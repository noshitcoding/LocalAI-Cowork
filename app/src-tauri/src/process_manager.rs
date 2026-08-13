use crate::db::Database;
use crate::process_control::{attach_process_tree, configure_process_tree, terminate_process_tree};
use serde::{Deserialize, Serialize};

use std::sync::Arc;

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

// ── Request types ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessStartRequest {
    pub label: String,
    pub command: String,
    pub backend_id: Option<String>,
    pub requires_admin: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct AdminApprovalRequest {
    pub process_id: String,
    pub approved: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessStartResult {
    pub process_id: String,
    pub pid: Option<u32>,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessStatusResult {
    pub process_id: String,
    pub label: String,
    pub command: String,
    pub status: String,
    pub pid: Option<i64>,
    pub exit_code: Option<i32>,
    pub requires_admin: bool,
    pub admin_approved: bool,
}

// ── Admin / sudo detection ──────────────────────────────────────────────────

/// Detects whether a command may require elevated privileges
pub fn detect_admin_requirement(command: &str) -> bool {
    let lower = command.to_lowercase();
    let admin_patterns = [
        "sudo ",
        "runas",
        "start-process.*-verb.*runas",
        "net start",
        "net stop",
        "sc.exe ",
        "bcdedit",
        "dism ",
        "sfc /",
        "chown ",
        "chmod ",
        "systemctl ",
        "service ",
        "iptables ",
        "ufw ",
        "netsh ",
        "reg add",
        "reg delete",
        "regedit",
    ];

    admin_patterns.iter().any(|p| lower.contains(p))
}

/// Start a background process. If it requires admin, it must be approved first.
pub fn start_process(db: &Arc<Database>, request: &ProcessStartRequest) -> ProcessStartResult {
    let id = uuid::Uuid::new_v4().to_string();
    let needs_admin = request.requires_admin || detect_admin_requirement(&request.command);

    // Record in DB
    if let Err(e) = db.insert_managed_process(
        &id,
        &request.label,
        &request.command,
        request.backend_id.as_deref(),
        needs_admin,
    ) {
        return ProcessStartResult {
            process_id: id,
            pid: None,
            status: "error".to_string(),
            message: format!("DB-Fehler: {}", e),
        };
    }

    if needs_admin {
        return ProcessStartResult {
            process_id: id,
            pid: None,
            status: "pending_approval".to_string(),
            message: "Process requires admin approval".to_string(),
        };
    }

    // Actually spawn (local only for now)
    spawn_local_process(db, &id, &request.command)
}

/// Approve an admin-required process and optionally start it
pub fn approve_and_start(
    db: &Arc<Database>,
    process_id: &str,
    approved: bool,
) -> ProcessStartResult {
    if !approved {
        let _ = db.update_process_status(process_id, "rejected", None, None);
        return ProcessStartResult {
            process_id: process_id.to_string(),
            pid: None,
            status: "rejected".to_string(),
            message: "Admin approval rejected".to_string(),
        };
    }

    let _ = db.approve_process_admin(process_id);

    let processes = match db.list_managed_processes() {
        Ok(p) => p,
        Err(e) => {
            return ProcessStartResult {
                process_id: process_id.to_string(),
                pid: None,
                status: "error".to_string(),
                message: format!("DB-Fehler: {}", e),
            };
        }
    };

    let proc = match processes.into_iter().find(|p| p.id == process_id) {
        Some(p) => p,
        None => {
            return ProcessStartResult {
                process_id: process_id.to_string(),
                pid: None,
                status: "error".to_string(),
                message: "Process not found".to_string(),
            };
        }
    };

    spawn_local_process(db, process_id, &proc.command)
}

/// Stop a running process by PID
pub fn stop_process(db: &Arc<Database>, process_id: &str) -> Result<(), String> {
    let processes = db.list_managed_processes().map_err(|e| e.to_string())?;
    let proc = processes
        .into_iter()
        .find(|p| p.id == process_id)
        .ok_or_else(|| "Process not found".to_string())?;

    if let Some(pid) = proc.pid {
        #[cfg(target_os = "windows")]
        {
            let mut command = std::process::Command::new("taskkill");
            command.args(["/PID", &pid.to_string(), "/T", "/F"]);
            suppress_command_window(&mut command);
            let _ = command.output();
        }
        #[cfg(not(target_os = "windows"))]
        {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
        }
    }

    db.update_process_status(process_id, "stopped", None, None)
        .map_err(|e| e.to_string())
}

/// Get status of all managed processes
pub fn list_process_statuses(db: &Arc<Database>) -> Result<Vec<ProcessStatusResult>, String> {
    let processes = db.list_managed_processes().map_err(|e| e.to_string())?;
    Ok(processes
        .into_iter()
        .map(|p| ProcessStatusResult {
            process_id: p.id,
            label: p.label,
            command: p.command,
            status: p.status,
            pid: p.pid,
            exit_code: p.exit_code,
            requires_admin: p.requires_admin,
            admin_approved: p.admin_approved,
        })
        .collect())
}

// ── Internal ────────────────────────────────────────────────────────────────

fn spawn_local_process(db: &Arc<Database>, process_id: &str, command: &str) -> ProcessStartResult {
    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("powershell");
        c.args(["-NoProfile", "-NonInteractive", "-Command", command]);
        c
    } else {
        let mut c = std::process::Command::new("sh");
        c.args(["-c", command]);
        c
    };

    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    suppress_command_window(&mut cmd);
    configure_process_tree(&mut cmd);

    match cmd.spawn() {
        Ok(mut child) => {
            let pid = child.id();
            let process_tree = match attach_process_tree(&child) {
                Ok(process_tree) => process_tree,
                Err(error) => {
                    let _ = terminate_process_tree(&mut child, None);
                    let _ = db.update_process_status(process_id, "failed", None, None);
                    return ProcessStartResult {
                        process_id: process_id.to_string(),
                        pid: None,
                        status: "error".to_string(),
                        message: format!("Process isolation failed: {error}"),
                    };
                }
            };
            if let Err(error) =
                db.update_process_status(process_id, "running", Some(pid as i64), None)
            {
                let _ = terminate_process_tree(&mut child, Some(&process_tree));
                return ProcessStartResult {
                    process_id: process_id.to_string(),
                    pid: None,
                    status: "error".to_string(),
                    message: format!("Process state could not be persisted: {error}"),
                };
            }

            let monitor_db = Arc::clone(db);
            let monitor_process_id = process_id.to_string();
            let monitor = std::thread::Builder::new()
                .name(format!("managed-process-{pid}"))
                .spawn(move || {
                    let exit = child.wait();
                    drop(process_tree);
                    let (status, exit_code) = match exit {
                        Ok(exit_status) => (
                            if exit_status.success() {
                                "stopped"
                            } else {
                                "failed"
                            },
                            exit_status.code(),
                        ),
                        Err(_) => ("failed", None),
                    };
                    let _ = monitor_db.update_process_exit_if_running(
                        &monitor_process_id,
                        status,
                        exit_code,
                    );
                });
            if let Err(error) = monitor {
                let _ = db.update_process_status(process_id, "failed", None, None);
                return ProcessStartResult {
                    process_id: process_id.to_string(),
                    pid: None,
                    status: "error".to_string(),
                    message: format!("Process monitor could not start: {error}"),
                };
            }

            ProcessStartResult {
                process_id: process_id.to_string(),
                pid: Some(pid),
                status: "running".to_string(),
                message: format!("Process started (PID {})", pid),
            }
        }
        Err(e) => {
            let _ = db.update_process_status(process_id, "error", None, None);
            ProcessStartResult {
                process_id: process_id.to_string(),
                pid: None,
                status: "error".to_string(),
                message: format!("Start-Fehler: {}", e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait_for_status(db: &Arc<Database>, id: &str, expected: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let status = db
                .list_managed_processes()
                .unwrap()
                .into_iter()
                .find(|process| process.id == id)
                .map(|process| process.status)
                .unwrap();
            if status == expected {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "process status remained {status}"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    #[test]
    fn managed_process_monitor_persists_normal_exit() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let result = start_process(
            &db,
            &ProcessStartRequest {
                label: "short process".to_string(),
                command: "exit 0".to_string(),
                backend_id: None,
                requires_admin: false,
            },
        );

        assert_eq!(result.status, "running");
        wait_for_status(&db, &result.process_id, "stopped");
        let process = db
            .list_managed_processes()
            .unwrap()
            .into_iter()
            .find(|process| process.id == result.process_id)
            .unwrap();
        assert_eq!(process.exit_code, Some(0));
        assert!(process.pid.is_none());
        assert!(process.stopped_at.is_some());
    }

    #[test]
    fn explicit_stop_is_not_overwritten_by_exit_monitor() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let command = if cfg!(target_os = "windows") {
            "Start-Sleep -Seconds 30"
        } else {
            "sleep 30"
        };
        let result = start_process(
            &db,
            &ProcessStartRequest {
                label: "long process".to_string(),
                command: command.to_string(),
                backend_id: None,
                requires_admin: false,
            },
        );

        assert_eq!(result.status, "running");
        stop_process(&db, &result.process_id).unwrap();
        wait_for_status(&db, &result.process_id, "stopped");
        std::thread::sleep(Duration::from_millis(100));
        wait_for_status(&db, &result.process_id, "stopped");
    }
}
