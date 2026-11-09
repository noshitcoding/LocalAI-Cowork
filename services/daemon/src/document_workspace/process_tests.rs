//! Opt-in native factory/process regressions. The executable is an explicit,
//! separately built synthetic fixture, never a dependency Cargo-bin assumption.
use super::{Binding, SOURCE_MARKER, configure_at, read_marker};
use crate::authority_token_file;
use cowork_toolbox::{Config, Toolbox};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{Duration, Instant},
};

struct ChildWitness {
    #[cfg(windows)]
    original: fs::File,
    #[cfg(target_os = "linux")]
    original: std::os::fd::OwnedFd,
}

impl ChildWitness {
    fn open(pid: u32) -> io::Result<Option<Self>> {
        #[cfg(windows)]
        {
            use std::os::windows::io::FromRawHandle;
            use windows_sys::Win32::System::Threading::OpenProcess;
            // Retain this original process object, independently of PID reuse.
            let original = unsafe { OpenProcess(0x0010_0000, 0, pid) };
            if original.is_null() {
                let error = io::Error::last_os_error();
                return if error.raw_os_error() == Some(87) {
                    Ok(None)
                } else {
                    Err(error)
                };
            }
            Ok(Some(Self {
                original: unsafe { fs::File::from_raw_handle(original) },
            }))
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::{FromRawFd, OwnedFd};
            let original = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
            if original < 0 {
                let error = io::Error::last_os_error();
                return if error.raw_os_error() == Some(libc::ESRCH) {
                    Ok(None)
                } else {
                    Err(error)
                };
            }
            Ok(Some(Self {
                original: unsafe { OwnedFd::from_raw_fd(original as i32) },
            }))
        }
    }

    fn exited(&self) -> io::Result<bool> {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::System::Threading::WaitForSingleObject;
            match unsafe { WaitForSingleObject(self.original.as_raw_handle(), 0) } {
                0 => Ok(true),
                258 => Ok(false),
                _ => Err(io::Error::last_os_error()),
            }
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            let mut descriptor = libc::pollfd {
                fd: self.original.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let ready = unsafe { libc::poll(&mut descriptor, 1, 0) };
            if ready < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(ready > 0 && descriptor.revents & (libc::POLLIN | libc::POLLHUP) != 0)
        }
    }
}

fn contend(program: &Path, root: &Path) -> io::Result<Output> {
    Command::new(program).arg("--attempt").arg(root).output()
}

// Declared before Toolbox so unwinding drops the final Arc before this guard.
// Never remove scratch while child completion remains unproved.
struct Scratch {
    directory: Option<tempfile::TempDir>,
    control: PathBuf,
    source: PathBuf,
    workspace: Option<PathBuf>,
    program: PathBuf,
    child: Option<ChildWitness>,
    probe_requested: bool,
    cleanup_proven: bool,
}

impl Scratch {
    fn release(&self) -> io::Result<()> {
        fs::write(
            self.control.join("release"),
            b"release own native factory fixture",
        )
    }

    fn child_exited(&self) -> io::Result<bool> {
        self.child.as_ref().map_or(Ok(false), ChildWitness::exited)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if !self.probe_requested {
            self.cleanup_proven = true;
        }
        if !self.cleanup_proven {
            let _ = self.release();
            let until = Instant::now() + Duration::from_secs(25);
            while Instant::now() < until {
                let mut absent = false;
                if self.child.is_none()
                    && let Ok(bytes) = fs::read(self.control.join("started.json"))
                    && let Ok(started) = serde_json::from_slice::<Value>(&bytes)
                    && let Some(pid) = started["pid"]
                        .as_u64()
                        .and_then(|pid| u32::try_from(pid).ok())
                {
                    match ChildWitness::open(pid) {
                        Ok(Some(child)) => self.child = Some(child),
                        Ok(None) => absent = true,
                        Err(_) => {}
                    }
                }
                if self.control.join("finished.json").is_file()
                    && (absent || self.child_exited().unwrap_or(false))
                {
                    let source_open = contend(&self.program, &self.source)
                        .is_ok_and(|output| output.status.success());
                    let workspace_open = self.workspace.as_ref().is_none_or(|path| {
                        contend(&self.program, path).is_ok_and(|output| output.status.success())
                    });
                    if source_open && workspace_open {
                        self.cleanup_proven = true;
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        if !self.cleanup_proven
            && let Some(directory) = self.directory.take()
        {
            let preserved = directory.keep();
            eprintln!(
                "Native factory probe completion was not proved; preserved own scratch at {}",
                preserved.display()
            );
        }
    }
}

fn wait_json(path: &Path) -> Value {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(bytes) = fs::read(path)
            && let Ok(value) = serde_json::from_slice(&bytes)
        {
            return value;
        }
        assert!(
            Instant::now() < until,
            "own probe did not publish complete JSON at {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_reopened(program: &Path, root: &Path) -> Output {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let output = contend(program, root).unwrap();
        if output.status.success() {
            return output;
        }
        assert_eq!(
            output.status.code(),
            Some(2),
            "unexpected contender failure: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            Instant::now() < until,
            "native factory probe did not release an original root lease"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn check(phase: &str, exit_code: i32) {
    let fixture = fs::canonicalize(
        std::env::var_os("COWORK_TEST_DOCUMENT_PROBE_PROGRAM").expect(
            "set COWORK_TEST_DOCUMENT_PROBE_PROGRAM to the separately built synthetic fixture",
        ),
    )
    .unwrap();
    assert!(fixture.is_file());
    let directory = tempfile::Builder::new()
        .prefix("cw-workspace-probe-")
        .tempdir()
        .unwrap();
    let data = directory.path().join("data");
    let native = directory.path().join("native");
    let control = directory.path().join("control");
    for path in [&data, &native, &control] {
        drop(authority_token_file::private_directory(path, None).unwrap());
    }
    let source = data.join("toolbox");
    let base = native.join("LocalAI-Cowork/doc-work");
    let mut scratch = Scratch {
        directory: Some(directory),
        control,
        source: source.clone(),
        workspace: None,
        program: fixture.clone(),
        child: None,
        probe_requested: false,
        cleanup_proven: false,
    };
    let mut programs = BTreeMap::from([("python".into(), fixture.clone())]);
    if phase == "office" {
        programs.insert("libreoffice".into(), fixture);
    }
    let mut environment: BTreeMap<String, String> = ["SystemRoot", "WINDIR", "TEMP", "TMP"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok().map(|value| (name.into(), value)))
        .collect();
    for (key, value) in [
        (
            "COWORK_PROBE_FIXTURE_CONTROL",
            scratch.control.to_string_lossy().into_owned(),
        ),
        (
            "COWORK_PROBE_FIXTURE_ROOT",
            source.to_string_lossy().into_owned(),
        ),
        ("COWORK_PROBE_FIXTURE_USE_ADAPTER_ROOT", "1".into()),
        ("COWORK_PROBE_FIXTURE_PHASE", phase.into()),
        ("COWORK_PROBE_FIXTURE_EXIT", exit_code.to_string()),
    ] {
        environment.insert(key.into(), value);
    }
    let source_guard = authority_token_file::private_directory(&source, None).unwrap();
    let mut owner = Toolbox::open(Config {
        root: source.clone(),
        programs,
        environment,
        github_token: None,
    })
    .unwrap();
    configure_at(&data, &source, None, &base, &mut owner).unwrap();
    drop(source_guard);
    let workspace = {
        let source_directory = authority_token_file::private_directory(&source, None).unwrap();
        let (binding, held_marker) =
            read_marker::<Binding>(&source_directory, SOURCE_MARKER).unwrap();
        assert_eq!(binding.format, "cowork-document-source/1");
        let workspace = fs::canonicalize(base.join(binding.locator)).unwrap();
        drop(held_marker);
        drop(source_directory);
        workspace
    };
    scratch.workspace = Some(workspace.clone());
    scratch.probe_requested = true;
    assert_eq!(owner.availability().document_status, "probing");
    let started = wait_json(&scratch.control.join("started.json"));
    let pid = u32::try_from(started["pid"].as_u64().unwrap()).unwrap();
    scratch.child = ChildWitness::open(pid).unwrap();
    assert!(
        scratch.child.is_some(),
        "the waiting original fixture process must still exist"
    );
    let input = PathBuf::from(started["input"].as_str().unwrap());
    assert!(
        fs::canonicalize(&input).unwrap().starts_with(&workspace),
        "the actual probe must use the selected native slot"
    );
    let input_bytes = fs::read(&input).unwrap();
    let profile = started["profile"].as_str().map(PathBuf::from);
    if let Some(profile) = &profile {
        assert!(fs::canonicalize(profile).unwrap().starts_with(&workspace));
        fs::write(
            profile.join("factory-owner-sentinel.txt"),
            b"original probe profile",
        )
        .unwrap();
    }
    assert_eq!(profile.is_some(), phase == "office");
    assert!(!scratch.control.join("finished.json").exists());
    // Only the actual availability thread can retain Toolbox's original lease
    // and the factory binding's separate workspace lease from this point onward.
    drop(owner);
    let source_attempt = contend(&scratch.program, &source).unwrap();
    let workspace_attempt = contend(&scratch.program, &workspace).unwrap();
    let input_preserved_while_running = fs::read(&input).is_ok_and(|bytes| bytes == input_bytes);
    let profile_preserved_while_running = profile.as_ref().is_none_or(|path| {
        path.is_dir()
            && fs::read(path.join("factory-owner-sentinel.txt"))
                .is_ok_and(|bytes| bytes == b"original probe profile")
    });
    let child_still_waiting =
        !scratch.control.join("finished.json").exists() && !scratch.child_exited().unwrap();
    scratch.release().unwrap();
    let finished = wait_json(&scratch.control.join("finished.json"));
    let until = Instant::now() + Duration::from_secs(10);
    while !scratch.child_exited().unwrap() {
        assert!(
            Instant::now() < until,
            "released original fixture process did not exit"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let source_reopened = wait_reopened(&scratch.program, &source);
    let workspace_reopened = wait_reopened(&scratch.program, &workspace);
    scratch.cleanup_proven = true;
    println!(
        "{}",
        serde_json::json!({
            "format":"cowork-document-workspace-probe-owner/1", "phase":phase, "child_pid":pid,
            "probe_exit_requested":exit_code, "synthetic_python_failure_exit":if phase == "python" { Some(7) } else { None },
            "last_toolbox_arc_dropped":true,
            "source_second_exit":source_attempt.status.code(), "source_second_stdout":String::from_utf8_lossy(&source_attempt.stdout).trim(),
            "workspace_second_exit":workspace_attempt.status.code(), "workspace_second_stdout":String::from_utf8_lossy(&workspace_attempt.stdout).trim(),
            "input_preserved_while_running":input_preserved_while_running,
            "profile_preserved_while_running":profile_preserved_while_running,
            "child_still_waiting":child_still_waiting, "child_exited":true, "finished":finished,
            "source_reopened_after_probe":source_reopened.status.success(),
            "workspace_reopened_after_probe":workspace_reopened.status.success()
        })
    );
    for (name, attempt) in [("source", source_attempt), ("workspace", workspace_attempt)] {
        assert_eq!(
            attempt.status.code(),
            Some(2),
            "separate process acquired {name} while its original probe was live"
        );
        assert_eq!(String::from_utf8_lossy(&attempt.stdout).trim(), "busy");
    }
    assert!(
        child_still_waiting && input_preserved_while_running && profile_preserved_while_running
    );
    assert_eq!(finished["original_input_preserved"], true);
    assert_eq!(finished["original_profile_preserved"], true);
    assert_eq!(finished["requested_exit"], exit_code);
    assert_eq!(
        String::from_utf8_lossy(&source_reopened.stdout).trim(),
        "opened"
    );
    assert_eq!(
        String::from_utf8_lossy(&workspace_reopened.stdout).trim(),
        "opened"
    );
}

#[test]
#[ignore = "requires explicit COWORK_TEST_DOCUMENT_PROBE_PROGRAM synthetic fixture"]
fn native_workspace_python_probe_keeps_both_original_leases_after_last_toolbox_arc() {
    check("python", 0);
}

#[test]
#[ignore = "requires explicit COWORK_TEST_DOCUMENT_PROBE_PROGRAM synthetic fixture"]
fn native_workspace_office_probe_keeps_both_original_leases_after_last_toolbox_arc() {
    check("office", 0);
}

#[test]
#[ignore = "requires explicit COWORK_TEST_DOCUMENT_PROBE_PROGRAM synthetic fixture"]
fn native_workspace_failing_office_probe_releases_both_leases_after_completion() {
    check("office", 9);
}
