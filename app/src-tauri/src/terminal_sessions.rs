use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Mutex;
use std::thread;
use tauri::Emitter;

#[derive(Default)]
pub struct TerminalSessionRegistry {
    sessions: Mutex<HashMap<String, ManagedTerminalSession>>,
}

struct ManagedTerminalSession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalCreateRequest {
    pub session_id: String,
    pub shell: Option<String>,
    pub cwd: Option<String>,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSessionRequest {
    pub session_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalWriteRequest {
    pub session_id: String,
    pub data: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalResizeRequest {
    pub session_id: String,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalCreateResponse {
    pub session_id: String,
    pub shell: String,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOutputPayload {
    pub session_id: String,
    pub stream: String,
    pub data: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalExitPayload {
    pub session_id: String,
    pub exit_code: Option<i32>,
    pub reason: String,
}

fn default_shell() -> String {
    if cfg!(target_os = "windows") {
        "powershell.exe".to_string()
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string())
    }
}

fn configure_shell_command(command: &mut CommandBuilder, shell: &str) {
    if cfg!(target_os = "windows") {
        let lower = shell.to_ascii_lowercase();
        if lower.contains("powershell") || lower.ends_with("pwsh") || lower.ends_with("pwsh.exe") {
            command.args(["-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass"]);
        } else if lower.ends_with("cmd") || lower.ends_with("cmd.exe") {
            command.args(["/Q"]);
        }
    } else {
        command.arg("-i");
        command.env("TERM", "xterm-256color");
    }
}

fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.max(1),
        cols: cols.max(1),
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn spawn_reader(app: tauri::AppHandle, session_id: String, mut reader: Box<dyn Read + Send>) {
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(size) => {
                    let payload = TerminalOutputPayload {
                        session_id: session_id.clone(),
                        stream: "stdout".to_string(),
                        data: String::from_utf8_lossy(&buffer[..size]).to_string(),
                    };
                    let _ = app.emit("terminal-output", payload);
                }
                Err(error) => {
                    let payload = TerminalOutputPayload {
                        session_id: session_id.clone(),
                        stream: "stderr".to_string(),
                        data: format!("terminal stream error: {}\n", error),
                    };
                    let _ = app.emit("terminal-output", payload);
                    break;
                }
            }
        }
    });
}

fn spawn_waiter(
    app: tauri::AppHandle,
    session_id: String,
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
) {
    thread::spawn(move || {
        let (exit_code, reason) = match child.wait() {
            Ok(status) => (Some(status.exit_code() as i32), "exited".to_string()),
            Err(error) => (None, format!("wait failed: {}", error)),
        };
        let payload = TerminalExitPayload {
            session_id,
            exit_code,
            reason,
        };
        let _ = app.emit("terminal-exit", payload);
    });
}

pub fn create_terminal_session(
    app: tauri::AppHandle,
    registry: tauri::State<'_, TerminalSessionRegistry>,
    request: TerminalCreateRequest,
) -> Result<TerminalCreateResponse, String> {
    let session_id = request.session_id.trim().to_string();
    if session_id.is_empty() {
        return Err("session_id must not be empty".to_string());
    }

    let shell = request
        .shell
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(default_shell);

    let cols = request.cols.unwrap_or(100);
    let rows = request.rows.unwrap_or(24);
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(pty_size(cols, rows))
        .map_err(|error| format!("Terminal PTY could not start: {}", error))?;

    let mut command = CommandBuilder::new(&shell);
    configure_shell_command(&mut command, &shell);
    if let Some(cwd) = request
        .cwd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        command.cwd(cwd);
    }

    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("Terminal shell could not start '{}': {}", shell, error))?;
    let killer = child.clone_killer();
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| format!("terminal reader unavailable: {}", error))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| format!("terminal writer unavailable: {}", error))?;

    spawn_reader(app.clone(), session_id.clone(), reader);
    spawn_waiter(app.clone(), session_id.clone(), child);

    {
        let mut sessions = registry
            .sessions
            .lock()
            .map_err(|error| error.to_string())?;
        if let Some(mut existing) = sessions.remove(&session_id) {
            let _ = existing.killer.kill();
        }
        sessions.insert(
            session_id.clone(),
            ManagedTerminalSession {
                master: pair.master,
                writer,
                killer,
            },
        );
    }

    let payload = TerminalOutputPayload {
        session_id: session_id.clone(),
        stream: "system".to_string(),
        data: format!(
            "Open Cowork terminal started: {}{} ({}x{})\n",
            shell,
            request
                .cwd
                .as_deref()
                .map(|cwd| format!(" in {}", cwd))
                .unwrap_or_default(),
            cols,
            rows,
        ),
    };
    let _ = app.emit("terminal-output", payload);

    Ok(TerminalCreateResponse {
        session_id,
        shell,
        cwd: request.cwd,
    })
}

pub fn write_terminal_session(
    registry: tauri::State<'_, TerminalSessionRegistry>,
    request: TerminalWriteRequest,
) -> Result<(), String> {
    let mut sessions = registry
        .sessions
        .lock()
        .map_err(|error| error.to_string())?;
    let session = sessions
        .get_mut(&request.session_id)
        .ok_or_else(|| format!("terminal session '{}' not found", request.session_id))?;
    session
        .writer
        .write_all(request.data.as_bytes())
        .map_err(|error| error.to_string())?;
    session.writer.flush().map_err(|error| error.to_string())
}

pub fn resize_terminal_session(
    registry: tauri::State<'_, TerminalSessionRegistry>,
    request: TerminalResizeRequest,
) -> Result<(), String> {
    let sessions = registry
        .sessions
        .lock()
        .map_err(|error| error.to_string())?;
    let session = sessions
        .get(&request.session_id)
        .ok_or_else(|| format!("terminal session '{}' not found", request.session_id))?;
    session
        .master
        .resize(pty_size(request.cols, request.rows))
        .map_err(|error| error.to_string())
}

pub fn interrupt_terminal_session(
    registry: tauri::State<'_, TerminalSessionRegistry>,
    request: TerminalSessionRequest,
) -> Result<(), String> {
    let mut sessions = registry
        .sessions
        .lock()
        .map_err(|error| error.to_string())?;
    let session = sessions
        .get_mut(&request.session_id)
        .ok_or_else(|| format!("terminal session '{}' not found", request.session_id))?;
    session
        .writer
        .write_all(b"\x03")
        .map_err(|error| error.to_string())?;
    session.writer.flush().map_err(|error| error.to_string())
}

pub fn kill_terminal_session(
    registry: tauri::State<'_, TerminalSessionRegistry>,
    request: TerminalSessionRequest,
) -> Result<(), String> {
    let mut sessions = registry
        .sessions
        .lock()
        .map_err(|error| error.to_string())?;
    let mut session = sessions
        .remove(&request.session_id)
        .ok_or_else(|| format!("terminal session '{}' not found", request.session_id))?;
    session.killer.kill().map_err(|error| error.to_string())
}

pub fn close_terminal_session(
    registry: tauri::State<'_, TerminalSessionRegistry>,
    request: TerminalSessionRequest,
) -> Result<(), String> {
    let mut sessions = registry
        .sessions
        .lock()
        .map_err(|error| error.to_string())?;
    if let Some(mut session) = sessions.remove(&request.session_id) {
        let _ = session.writer.write_all(b"exit\n");
        let _ = session.writer.flush();
        let _ = session.killer.kill();
    }
    Ok(())
}
