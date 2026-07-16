use crate::credential_store::CredentialStore;
use crate::db::{Database, TerminalBackendRow};
use crate::process_control::{attach_process_tree, configure_process_tree, terminate_process_tree};
use crate::secure_config::{self, SecureConfigScope};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::process::Stdio;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[cfg(target_os = "windows")]
fn suppress_command_window(command: &mut std::process::Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
fn suppress_command_window(_command: &mut std::process::Command) {}

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MIN_TIMEOUT_MS: u64 = 100;
const MAX_TIMEOUT_MS: u64 = 600_000;
const MAX_CAPTURE_BYTES: usize = 4 * 1024 * 1024;
const WAIT_INTERVAL: Duration = Duration::from_millis(10);

struct CapturedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

fn read_limited(mut reader: impl Read) -> std::io::Result<CapturedOutput> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;

    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let remaining = MAX_CAPTURE_BYTES.saturating_sub(bytes.len());
        if remaining > 0 {
            bytes.extend_from_slice(&buffer[..count.min(remaining)]);
        }
        if count > remaining {
            truncated = true;
        }
    }

    Ok(CapturedOutput { bytes, truncated })
}

fn output_text(captured: CapturedOutput) -> String {
    let mut text = String::from_utf8_lossy(&captured.bytes).to_string();
    if captured.truncated {
        text.push_str(&format!(
            "\n[output truncated after {} bytes]\n",
            MAX_CAPTURE_BYTES
        ));
    }
    text
}

fn append_error(stderr: &mut String, message: impl AsRef<str>) {
    if !stderr.is_empty() && !stderr.ends_with('\n') {
        stderr.push('\n');
    }
    stderr.push_str(message.as_ref());
    if !stderr.ends_with('\n') {
        stderr.push('\n');
    }
}

// ── Backend config types ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalBackendConfig {
    pub shell: Option<String>,
    pub working_dir: Option<String>,
    pub env_vars: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerBackendConfig {
    pub image: String,
    pub container_name: Option<String>,
    pub mounts: Option<Vec<String>>,
    pub ports: Option<Vec<String>>,
    pub env_vars: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshBackendConfig {
    pub host: String,
    pub port: Option<u16>,
    pub user: String,
    pub key_path: Option<String>,
    pub working_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HpcBackendConfig {
    pub runtime: String, // singularity | apptainer
    pub image_path: String,
    pub bind_mounts: Option<Vec<String>>,
    pub gpu_flag: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerlessBackendConfig {
    pub provider: String, // modal | lambda | cloud-run
    pub function_name: String,
    pub region: Option<String>,
    pub env_vars: Option<HashMap<String, String>>,
}

// ── Request / Response types ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct BackendUpsertRequest {
    pub id: String,
    pub name: String,
    pub backend_type: String,
    pub config_json: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct BackendExecRequest {
    pub backend_id: String,
    pub command: String,
    pub working_dir: Option<String>,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendExecResponse {
    pub backend_id: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

// ── Backend validation ──────────────────────────────────────────────────────

pub fn validate_backend_type(backend_type: &str) -> Result<(), String> {
    match backend_type {
        "local" | "container" | "ssh" | "hpc" | "serverless" => Ok(()),
        other => Err(format!(
            "Unknown backend type '{}'. Allowed: local, container, ssh, hpc, serverless",
            other
        )),
    }
}

pub fn validate_backend_config(backend_type: &str, config_json: &str) -> Result<(), String> {
    match backend_type {
        "local" => {
            serde_json::from_str::<LocalBackendConfig>(config_json)
                .map_err(|e| format!("Invalid local configuration: {}", e))?;
        }
        "container" => {
            let cfg: ContainerBackendConfig = serde_json::from_str(config_json)
                .map_err(|e| format!("Invalid container configuration: {}", e))?;
            if cfg.image.is_empty() {
                return Err("Container image must not be empty".to_string());
            }
        }
        "ssh" => {
            let cfg: SshBackendConfig = serde_json::from_str(config_json)
                .map_err(|e| format!("Invalid SSH configuration: {}", e))?;
            if cfg.host.is_empty() || cfg.user.is_empty() {
                return Err("SSH host und user sind erforderlich".to_string());
            }
        }
        "hpc" => {
            let cfg: HpcBackendConfig = serde_json::from_str(config_json)
                .map_err(|e| format!("Invalid HPC configuration: {}", e))?;
            if cfg.image_path.is_empty() {
                return Err("HPC image_path must not be empty".to_string());
            }
        }
        "serverless" => {
            let cfg: ServerlessBackendConfig = serde_json::from_str(config_json)
                .map_err(|e| format!("Invalid serverless configuration: {}", e))?;
            if cfg.function_name.is_empty() {
                return Err("Serverless function_name must not be empty".to_string());
            }
        }
        _ => return Err(format!("Unbekannter Backend-Typ: {}", backend_type)),
    }
    Ok(())
}

/// Execute a command on the local backend
pub fn execute_local(
    config_json: &str,
    command: &str,
    working_dir_override: Option<&str>,
    timeout_ms: Option<u64>,
) -> BackendExecResponse {
    let config: LocalBackendConfig = match serde_json::from_str(config_json) {
        Ok(c) => c,
        Err(e) => {
            return BackendExecResponse {
                backend_id: String::new(),
                stdout: String::new(),
                stderr: format!("Config-Fehler: {}", e),
                exit_code: None,
                timed_out: false,
            };
        }
    };

    let shell = config.shell.unwrap_or_else(|| {
        if cfg!(target_os = "windows") {
            "powershell".to_string()
        } else {
            "sh".to_string()
        }
    });

    let cwd = working_dir_override
        .map(|s| s.to_string())
        .or(config.working_dir);

    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new(&shell);
        let shell_lower = shell.to_ascii_lowercase();
        if shell_lower.contains("powershell")
            || shell_lower.ends_with("pwsh")
            || shell_lower.ends_with("pwsh.exe")
        {
            c.args(["-NoProfile", "-NonInteractive", "-Command", command]);
        } else if shell_lower.ends_with("cmd") || shell_lower.ends_with("cmd.exe") {
            c.args(["/C", command]);
        } else {
            c.args(["-c", command]);
        }
        c
    } else {
        let mut c = std::process::Command::new(&shell);
        c.args(["-c", command]);
        c
    };

    if let Some(ref dir) = cwd {
        cmd.current_dir(dir);
    }
    suppress_command_window(&mut cmd);
    configure_process_tree(&mut cmd);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    if let Some(ref env_vars) = config.env_vars {
        for (k, v) in env_vars {
            cmd.env(k, v);
        }
    }

    let timeout_ms = timeout_ms
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .clamp(MIN_TIMEOUT_MS, MAX_TIMEOUT_MS);
    let timeout = Duration::from_millis(timeout_ms);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            return BackendExecResponse {
                backend_id: String::new(),
                stdout: String::new(),
                stderr: format!("Execution error: {error}"),
                exit_code: None,
                timed_out: false,
            };
        }
    };
    let process_tree = attach_process_tree(&child).ok();

    let stdout_handle = child
        .stdout
        .take()
        .map(|stdout| thread::spawn(move || read_limited(stdout)));
    let stderr_handle = child
        .stderr
        .take()
        .map(|stderr| thread::spawn(move || read_limited(stderr)));

    let started = Instant::now();
    let mut timed_out = false;
    let mut runtime_errors = Vec::new();
    let mut capture_complete = true;
    let exit_status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() >= timeout => {
                timed_out = true;
                if let Err(error) = terminate_process_tree(&mut child, process_tree.as_ref()) {
                    runtime_errors.push(format!("Failed to terminate timed-out process: {error}"));
                    capture_complete = false;
                }
                break None;
            }
            Ok(None) => thread::sleep(WAIT_INTERVAL),
            Err(error) => {
                runtime_errors.push(format!("Failed to wait for process: {error}"));
                if let Err(termination_error) =
                    terminate_process_tree(&mut child, process_tree.as_ref())
                {
                    runtime_errors.push(format!(
                        "Failed to terminate process after wait error: {termination_error}"
                    ));
                    capture_complete = false;
                }
                break None;
            }
        }
    };

    let mut stdout = String::new();
    let mut stderr = String::new();
    drop(process_tree);

    if capture_complete {
        match stdout_handle {
            Some(handle) => match handle.join() {
                Ok(Ok(captured)) => stdout = output_text(captured),
                Ok(Err(error)) => {
                    append_error(&mut stderr, format!("stdout capture failed: {error}"))
                }
                Err(_) => append_error(&mut stderr, "stdout capture thread failed"),
            },
            None => append_error(&mut stderr, "stdout pipe unavailable"),
        }

        match stderr_handle {
            Some(handle) => match handle.join() {
                Ok(Ok(captured)) => stderr.push_str(&output_text(captured)),
                Ok(Err(error)) => {
                    append_error(&mut stderr, format!("stderr capture failed: {error}"))
                }
                Err(_) => append_error(&mut stderr, "stderr capture thread failed"),
            },
            None => append_error(&mut stderr, "stderr pipe unavailable"),
        }
    } else {
        drop(stdout_handle);
        drop(stderr_handle);
        append_error(
            &mut stderr,
            "Output capture was detached because the process could not be confirmed stopped.",
        );
    }

    if timed_out {
        append_error(
            &mut stderr,
            format!("Command timed out after {timeout_ms} ms."),
        );
    }
    for error in runtime_errors {
        append_error(&mut stderr, error);
    }

    BackendExecResponse {
        backend_id: String::new(),
        stdout,
        stderr,
        exit_code: exit_status.and_then(|status| status.code()),
        timed_out,
    }
}

/// Dispatch command execution to the correct backend
pub fn dispatch_exec(
    db: &Arc<Database>,
    credential_store: &CredentialStore,
    backend_id: &str,
    command: &str,
    working_dir: Option<&str>,
    timeout_ms: Option<u64>,
) -> Result<BackendExecResponse, String> {
    let backends = db.list_terminal_backends().map_err(|e| e.to_string())?;
    let backend = backends
        .into_iter()
        .find(|b| b.id == backend_id)
        .ok_or_else(|| format!("Backend '{}' not found", backend_id))?;

    if backend.status != "active" && backend.status != "connected" {
        return Err(format!(
            "Backend '{}' is not active (status: {})",
            backend.name, backend.status
        ));
    }

    let resolved_config = secure_config::resolve(
        credential_store,
        SecureConfigScope::TerminalBackend,
        &backend.id,
        &backend.config_json,
    )?;

    let mut result = match backend.backend_type.as_str() {
        "local" => execute_local(&resolved_config, command, working_dir, timeout_ms),
        "container" => {
            // Placeholder: would spawn docker exec or docker run
            BackendExecResponse {
                backend_id: backend_id.to_string(),
                stdout: String::new(),
                stderr: "Container backend is not implemented yet".to_string(),
                exit_code: None,
                timed_out: false,
            }
        }
        "ssh" => BackendExecResponse {
            backend_id: backend_id.to_string(),
            stdout: String::new(),
            stderr: "SSH backend is not implemented yet".to_string(),
            exit_code: None,
            timed_out: false,
        },
        "hpc" => BackendExecResponse {
            backend_id: backend_id.to_string(),
            stdout: String::new(),
            stderr: "HPC backend is not implemented yet".to_string(),
            exit_code: None,
            timed_out: false,
        },
        "serverless" => BackendExecResponse {
            backend_id: backend_id.to_string(),
            stdout: String::new(),
            stderr: "Serverless backend is not implemented yet".to_string(),
            exit_code: None,
            timed_out: false,
        },
        _ => return Err(format!("Unbekannter Backend-Typ: {}", backend.backend_type)),
    };

    result.backend_id = backend_id.to_string();

    // Touch last_connected
    let _ = db.update_terminal_backend_status(&backend.id, "active");

    Ok(result)
}

/// Get the default local backend, creating it if needed
pub fn ensure_default_local_backend(
    db: &Arc<Database>,
    credential_store: &CredentialStore,
) -> Result<TerminalBackendRow, String> {
    let backends = db.list_terminal_backends().map_err(|e| e.to_string())?;
    if let Some(local) = backends.into_iter().find(|b| b.backend_type == "local") {
        return Ok(local);
    }

    let id = uuid::Uuid::new_v4().to_string();
    let config = serde_json::to_string(&LocalBackendConfig {
        shell: None,
        working_dir: None,
        env_vars: None,
    })
    .unwrap_or_else(|_| "{}".to_string());

    secure_config::replace(
        credential_store,
        SecureConfigScope::TerminalBackend,
        &id,
        &config,
        None,
        |marker| {
            db.upsert_terminal_backend(&id, "Lokal", "local", marker)
                .map_err(|error| error.to_string())
        },
    )?;
    db.update_terminal_backend_status(&id, "active")
        .map_err(|e| e.to_string())?;

    db.list_terminal_backends()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|b| b.id == id)
        .ok_or_else(|| "Default backend could not be created".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Cursor;

    fn local_config() -> String {
        serde_json::to_string(&LocalBackendConfig {
            shell: None,
            working_dir: None,
            env_vars: None,
        })
        .expect("local backend config should serialize")
    }

    #[test]
    fn execute_local_captures_output_and_exit_code() {
        let command = if cfg!(target_os = "windows") {
            "[Console]::Out.WriteLine('hello'); [Console]::Error.WriteLine('warning'); exit 7"
        } else {
            "printf 'hello\\n'; printf 'warning\\n' >&2; exit 7"
        };

        let result = execute_local(&local_config(), command, None, Some(5_000));

        assert!(!result.timed_out);
        assert_eq!(result.exit_code, Some(7));
        assert!(result.stdout.contains("hello"));
        assert!(result.stderr.contains("warning"));
    }

    #[test]
    fn execute_local_reports_invalid_configuration_without_spawning() {
        let result = execute_local("{", "echo should-not-run", None, Some(1_000));

        assert!(!result.timed_out);
        assert_eq!(result.exit_code, None);
        assert!(result.stderr.contains("Config-Fehler"));
    }

    #[test]
    fn output_capture_is_bounded_and_reports_truncation() {
        let captured = read_limited(Cursor::new(vec![b'x'; MAX_CAPTURE_BYTES + 17]))
            .expect("in-memory output should be readable");

        assert_eq!(captured.bytes.len(), MAX_CAPTURE_BYTES);
        assert!(captured.truncated);
        assert!(output_text(captured).contains("output truncated"));
    }

    #[cfg(any(target_os = "windows", unix))]
    #[test]
    fn execute_local_closes_background_descendants_after_parent_exit() {
        let test_dir = std::env::temp_dir().join(format!(
            "open-cowork-terminal-background-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&test_dir).expect("test directory should be created");
        let marker = test_dir.join("background-survived.txt");

        #[cfg(target_os = "windows")]
        let command = {
            let child_script = test_dir.join("background-write.ps1");
            let marker_literal = marker.to_string_lossy().replace('\'', "''");
            fs::write(
                &child_script,
                format!(
                    "Start-Sleep -Milliseconds 1000\nSet-Content -LiteralPath '{}' -Value 'survived'\n",
                    marker_literal
                ),
            )
            .expect("child script should be written");
            let child_literal = child_script.to_string_lossy().replace('\'', "''");
            format!(
                "Start-Process -WindowStyle Hidden -FilePath 'powershell.exe' -ArgumentList @('-NoProfile','-NonInteractive','-File','{}') | Out-Null",
                child_literal
            )
        };

        #[cfg(unix)]
        let command = {
            let marker_literal = marker.to_string_lossy().replace('\'', "'\"'\"'");
            format!("(sleep 1; printf survived > '{}') &", marker_literal)
        };

        let result = execute_local(&local_config(), &command, None, Some(5_000));

        assert!(!result.timed_out);
        assert_eq!(result.exit_code, Some(0));
        thread::sleep(Duration::from_millis(1_300));
        assert!(
            !marker.exists(),
            "background descendant survived and wrote {}",
            marker.display()
        );
        let _ = fs::remove_dir_all(test_dir);
    }

    #[test]
    fn execute_local_timeout_terminates_descendant_processes() {
        let test_dir = std::env::temp_dir().join(format!(
            "open-cowork-terminal-timeout-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&test_dir).expect("test directory should be created");
        let marker = test_dir.join("descendant-survived.txt");

        #[cfg(target_os = "windows")]
        let command = {
            let child_script = test_dir.join("delayed-write.ps1");
            let marker_literal = marker.to_string_lossy().replace('\'', "''");
            fs::write(
                &child_script,
                format!(
                    "Start-Sleep -Milliseconds 1200\nSet-Content -LiteralPath '{}' -Value 'survived'\n",
                    marker_literal
                ),
            )
            .expect("child script should be written");
            let child_literal = child_script.to_string_lossy().replace('\'', "''");
            format!(
                "$child = Start-Process -PassThru -WindowStyle Hidden -FilePath 'powershell.exe' -ArgumentList @('-NoProfile','-NonInteractive','-File','{}'); Start-Sleep -Seconds 5",
                child_literal
            )
        };

        #[cfg(unix)]
        let command = {
            let marker_literal = marker.to_string_lossy().replace('\'', "'\"'\"'");
            format!(
                "(sleep 1; printf survived > '{}') & sleep 5",
                marker_literal
            )
        };

        #[cfg(not(any(target_os = "windows", unix)))]
        let command = "sleep 5".to_string();

        let started = Instant::now();
        let result = execute_local(&local_config(), &command, None, Some(200));

        assert!(result.timed_out);
        assert_eq!(result.exit_code, None);
        assert!(result.stderr.contains("Command timed out after 200 ms."));
        assert!(started.elapsed() < Duration::from_secs(2));

        thread::sleep(Duration::from_millis(1_500));
        assert!(
            !marker.exists(),
            "descendant process survived the timeout and wrote {}",
            marker.display()
        );
        let _ = fs::remove_dir_all(test_dir);
    }
}
