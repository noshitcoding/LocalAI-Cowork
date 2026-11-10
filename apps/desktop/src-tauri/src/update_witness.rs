//! Independent exact-process observer. The OS handle survives loss of the GUI.
//! No PID lookup, port absence, process-name search or automatic installation.
use super::{daemon_process::DaemonProcess, update_journal as journal};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
};

const ARG: &str = "--cowork-update-exit-witness";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    format: u32,
    folder: PathBuf,
    attempt: String,
    attempt_sha256: String,
    process: usize,
}

pub fn spawn(
    process: &DaemonProcess,
    folder: &std::path::Path,
    attempt: &str,
    digest: &str,
) -> Result<Child, String> {
    let mut command =
        Command::new(std::env::current_exe().map_err(|_| "update_witness_executable_missing")?);
    command
        .arg(ARG)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        let fd = process.transfer_fd();
        // Only the child changes CLOEXEC; no descriptor in the parent is exposed.
        unsafe {
            command.pre_exec(move || {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = command.spawn().map_err(|_| "update_witness_start_failed")?;
    let transfer = (|| -> Result<(), String> {
        #[cfg(windows)]
        let handle = process.duplicate_into(&child)?;
        #[cfg(target_os = "linux")]
        let handle = process.transfer_fd() as usize;
        #[cfg(not(any(windows, target_os = "linux")))]
        let handle = {
            return Err("unsupported_update_platform".to_owned());
        };
        let request = Request {
            format: 1,
            folder: folder.into(),
            attempt: attempt.into(),
            attempt_sha256: digest.into(),
            process: handle,
        };
        let bytes = serde_json::to_vec(&request).map_err(|_| "update_witness_handoff_failed")?;
        child
            .stdin
            .take()
            .ok_or("update_witness_handoff_failed")?
            .write_all(&bytes)
            .map_err(|_| "update_witness_handoff_failed".into())
    })();
    if let Err(error) = transfer {
        // This exact newly spawned observer has no right to terminate the daemon.
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    Ok(child)
}

pub async fn ready(
    child: &mut Child,
    folder: &std::path::Path,
    id: &str,
    digest: &str,
) -> Result<(), String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if journal::has_phase(folder, id, digest, "witness-ready")? {
            return Ok(());
        }
        if child
            .try_wait()
            .map_err(|_| "update_witness_observation_failed")?
            .is_some()
        {
            return Err("update_witness_start_failed".into());
        }
        if std::time::Instant::now() >= deadline {
            return Err("update_witness_ready_unconfirmed".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
}
pub async fn stopped(
    mut child: Child,
    folder: &std::path::Path,
    id: &str,
    digest: &str,
) -> Result<(), String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|_| "update_witness_observation_failed")?
        {
            return if status.success() && journal::has_phase(folder, id, digest, "daemon-stopped")?
            {
                Ok(())
            } else {
                Err("daemon_stop_not_confirmed_update_not_installed".into())
            };
        }
        if std::time::Instant::now() >= deadline {
            // Keep the independent observer alive. It owns the original handle and
            // records the eventual exit; this task only reaps that exact child.
            tauri::async_runtime::spawn_blocking(move || child.wait());
            return Err("daemon_stop_not_confirmed_update_not_installed".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
}
fn run() -> Result<(), String> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| "invalid_update_witness_request")?;
    if bytes.len() > 65536 {
        return Err("invalid_update_witness_request".into());
    }
    let request: Request =
        serde_json::from_slice(&bytes).map_err(|_| "invalid_update_witness_request")?;
    if request.format != 1
        || !request.folder.is_absolute()
        || !journal::valid_id(&request.attempt)
        || !journal::valid_sha(&request.attempt_sha256)
        || request.folder.file_name().and_then(|n| n.to_str()) != Some("update-receipts")
        || request
            .folder
            .canonicalize()
            .map_err(|_| "invalid_update_witness_request")?
            != request.folder
    {
        return Err("invalid_update_witness_request".into());
    }
    let (_, digest) = journal::load(&request.folder, &request.attempt)?;
    if digest != request.attempt_sha256 {
        return Err("update_receipt_binding_changed".into());
    }
    let process = unsafe { DaemonProcess::from_transfer(request.process)? };
    // Validate that the transferred native object is observable before readiness.
    let mut exited = process.wait_for_exit(0)?;
    journal::phase(&request.folder, &request.attempt, &digest, "witness-ready")?;
    while !exited {
        exited = process.wait_for_exit(1000)?;
        // A known rejected/non-dispatched request can release its observer.
        if !exited
            && (journal::has_phase(
                &request.folder,
                &request.attempt,
                &digest,
                "shutdown-rejected",
            )? || journal::has_phase(
                &request.folder,
                &request.attempt,
                &digest,
                "shutdown-not-dispatched",
            )?)
        {
            return Ok(());
        }
    }
    // Read the immutable original bytes again before publishing positive proof.
    if journal::load(&request.folder, &request.attempt)?.1 != digest {
        return Err("update_receipt_binding_changed".into());
    }
    journal::phase(&request.folder, &request.attempt, &digest, "daemon-stopped")
}
pub fn early_mode() -> Option<Result<(), String>> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|arg| arg == ARG) {
        Some(if args.len() == 2 {
            run()
        } else {
            Err("invalid_update_witness_request".into())
        })
    } else {
        None
    }
}
