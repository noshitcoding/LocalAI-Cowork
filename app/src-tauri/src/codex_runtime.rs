use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

const CODEX_VERSION: &str = "0.147.0";
const PROTOCOL_SCHEMA: &str = "app-server-0.147.0";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const SECRET_ENV_VARS: &[&str] = &[
    "OPENAI_API_KEY",
    "CODEX_API_KEY",
    "AZURE_OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
];

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeManifest {
    version: String,
    protocol_schema: String,
    binary: String,
    sha256: String,
    license: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub available: bool,
    pub version: String,
    pub protocol_schema: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeEvent {
    profile_id: String,
    payload: Value,
}

struct CodexProcess {
    profile_id: String,
    profile_home: PathBuf,
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
    pending: Arc<Mutex<HashMap<u64, mpsc::Sender<Result<Value, String>>>>>,
    subscribers: Arc<Mutex<Vec<mpsc::Sender<Value>>>>,
    next_id: AtomicU64,
}

impl CodexProcess {
    fn send_line(&self, payload: &Value) -> Result<(), String> {
        let serialized = serde_json::to_string(payload).map_err(|error| error.to_string())?;
        let mut stdin = self
            .stdin
            .lock()
            .map_err(|_| "Codex input stream is unavailable".to_string())?;
        stdin
            .write_all(serialized.as_bytes())
            .and_then(|_| stdin.write_all(b"\n"))
            .and_then(|_| stdin.flush())
            .map_err(|error| format!("Could not write to the Codex App Server: {error}"))
    }

    fn request(&self, method: &str, params: Option<Value>) -> Result<Value, String> {
        self.reject_plaintext_auth_file()?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel();
        self.pending
            .lock()
            .map_err(|_| "Codex response registry is unavailable".to_string())?
            .insert(id, sender);

        let mut payload = json!({ "id": id, "method": method });
        if let Some(params) = params {
            payload["params"] = params;
        }
        if let Err(error) = self.send_line(&payload) {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&id);
            }
            return Err(error);
        }

        let result = receiver.recv_timeout(REQUEST_TIMEOUT).map_err(|error| {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&id);
            }
            match error {
                mpsc::RecvTimeoutError::Timeout => {
                    format!("Codex App Server timed out while handling {method}")
                }
                mpsc::RecvTimeoutError::Disconnected => {
                    "Codex App Server stopped before replying".to_string()
                }
            }
        })??;
        self.reject_plaintext_auth_file()?;
        Ok(result)
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.send_line(&json!({ "method": method, "params": params }))
    }

    fn respond(&self, id: u64, result: Value) -> Result<(), String> {
        self.send_line(&json!({ "id": id, "result": result }))
    }

    fn reject_plaintext_auth_file(&self) -> Result<(), String> {
        if self.profile_home.join("auth.json").exists() {
            return Err(format!(
                "Codex profile {} attempted to create auth.json. Sign-in was disabled because only operating-system key storage is allowed.",
                self.profile_id
            ));
        }
        Ok(())
    }

    fn stop(&self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub struct CodexRuntimeManager {
    resource_dir: PathBuf,
    profiles_dir: PathBuf,
    app: AppHandle,
    processes: Mutex<HashMap<String, Arc<CodexProcess>>>,
}

impl CodexRuntimeManager {
    pub fn new(resource_dir: PathBuf, app_data_dir: PathBuf, app: AppHandle) -> Self {
        Self {
            resource_dir,
            profiles_dir: app_data_dir.join("codex-profiles"),
            app,
            processes: Mutex::new(HashMap::new()),
        }
    }

    pub fn status(&self) -> RuntimeStatus {
        match self.verified_runtime() {
            Ok(_) => RuntimeStatus {
                available: true,
                version: CODEX_VERSION.to_string(),
                protocol_schema: PROTOCOL_SCHEMA.to_string(),
                error: None,
            },
            Err(error) => RuntimeStatus {
                available: false,
                version: CODEX_VERSION.to_string(),
                protocol_schema: PROTOCOL_SCHEMA.to_string(),
                error: Some(error),
            },
        }
    }

    pub fn request(
        &self,
        profile_id: &str,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, String> {
        self.process(profile_id)?.request(method, params)
    }

    pub fn respond(&self, profile_id: &str, id: u64, result: Value) -> Result<(), String> {
        self.process(profile_id)?.respond(id, result)
    }

    pub fn subscribe(&self, profile_id: &str) -> Result<mpsc::Receiver<Value>, String> {
        let process = self.process(profile_id)?;
        let (sender, receiver) = mpsc::channel();
        process
            .subscribers
            .lock()
            .map_err(|_| "Codex event subscriber registry is unavailable".to_string())?
            .push(sender);
        Ok(receiver)
    }

    pub fn stop_profile(&self, profile_id: &str) {
        if let Ok(mut processes) = self.processes.lock() {
            if let Some(process) = processes.remove(profile_id) {
                process.stop();
            }
        }
    }

    pub fn remove_profile_data(&self, profile_id: &str) -> Result<(), String> {
        self.stop_profile(profile_id);
        let path = self.profile_home(profile_id)?;
        if path.exists() {
            fs::remove_dir_all(&path)
                .map_err(|error| format!("Could not remove Codex profile data: {error}"))?;
        }
        Ok(())
    }

    fn process(&self, profile_id: &str) -> Result<Arc<CodexProcess>, String> {
        validate_profile_id(profile_id)?;
        let mut processes = self
            .processes
            .lock()
            .map_err(|_| "Codex process registry is unavailable".to_string())?;
        if let Some(process) = processes.get(profile_id).cloned() {
            return Ok(process);
        }

        let process = Arc::new(self.spawn(profile_id)?);
        processes.insert(profile_id.to_string(), process.clone());
        Ok(process)
    }

    fn spawn(&self, profile_id: &str) -> Result<CodexProcess, String> {
        let binary = self.verified_runtime()?;
        let profile_home = self.profile_home(profile_id)?;
        fs::create_dir_all(&profile_home)
            .map_err(|error| format!("Could not create isolated Codex profile: {error}"))?;
        write_profile_config(&profile_home)?;
        if profile_home.join("auth.json").exists() {
            return Err("Plaintext Codex credentials were found. Remove auth.json before signing in; OpenCowork only permits the operating-system keyring.".to_string());
        }

        verify_binary_version(&binary, &profile_home)?;

        let mut command = Command::new(&binary);
        command
            .arg("app-server")
            .arg("--listen")
            .arg("stdio://")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("CODEX_HOME", &profile_home);
        remove_secret_environment(&mut command);
        suppress_window(&mut command);

        let mut child = command
            .spawn()
            .map_err(|error| format!("Could not start the bundled Codex App Server: {error}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Codex App Server input stream is missing".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Codex App Server output stream is missing".to_string())?;
        let stderr = child.stderr.take();
        let pending: Arc<Mutex<HashMap<u64, mpsc::Sender<Result<Value, String>>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let reader_pending = pending.clone();
        let subscribers: Arc<Mutex<Vec<mpsc::Sender<Value>>>> = Arc::new(Mutex::new(Vec::new()));
        let reader_subscribers = subscribers.clone();
        let reader_profile = profile_id.to_string();
        let reader_app = self.app.clone();

        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let Ok(payload) = serde_json::from_str::<Value>(&line) else {
                    let _ = reader_app.emit(
                        "codex-runtime-event",
                        RuntimeEvent {
                            profile_id: reader_profile.clone(),
                            payload: json!({
                                "method": "runtime/protocolError",
                                "params": { "message": "Codex emitted invalid JSONL" }
                            }),
                        },
                    );
                    continue;
                };

                let response_id = payload
                    .get("id")
                    .and_then(Value::as_u64)
                    .filter(|_| payload.get("method").is_none());
                if let Some(id) = response_id {
                    if let Ok(mut pending) = reader_pending.lock() {
                        if let Some(sender) = pending.remove(&id) {
                            let result = match payload.get("error") {
                                Some(error) if !error.is_null() => Err(format_rpc_error(error)),
                                _ => Ok(payload.get("result").cloned().unwrap_or(Value::Null)),
                            };
                            let _ = sender.send(result);
                            continue;
                        }
                    }
                }

                if let Ok(mut subscribers) = reader_subscribers.lock() {
                    subscribers.retain(|subscriber| subscriber.send(payload.clone()).is_ok());
                }

                let _ = reader_app.emit(
                    "codex-runtime-event",
                    RuntimeEvent {
                        profile_id: reader_profile.clone(),
                        payload,
                    },
                );
            }

            if let Ok(mut pending) = reader_pending.lock() {
                for (_, sender) in pending.drain() {
                    let _ = sender.send(Err("Codex App Server stopped unexpectedly".to_string()));
                }
            }
            let _ = reader_app.emit(
                "codex-runtime-event",
                RuntimeEvent {
                    profile_id: reader_profile,
                    payload: json!({ "method": "runtime/stopped", "params": {} }),
                },
            );
        });

        if let Some(stderr) = stderr {
            let stderr_app = self.app.clone();
            let stderr_profile = profile_id.to_string();
            thread::spawn(move || {
                // Consume stderr so the child cannot block. Contents are intentionally not logged:
                // upstream diagnostics may contain user text or credentials.
                for line in BufReader::new(stderr).lines() {
                    if line.is_err() {
                        break;
                    }
                }
                let _ = stderr_app.emit(
                    "codex-runtime-event",
                    RuntimeEvent {
                        profile_id: stderr_profile,
                        payload: json!({ "method": "runtime/stderrClosed", "params": {} }),
                    },
                );
            });
        }

        let process = CodexProcess {
            profile_id: profile_id.to_string(),
            profile_home,
            stdin: Mutex::new(stdin),
            child: Mutex::new(child),
            pending,
            subscribers,
            next_id: AtomicU64::new(1),
        };
        let initialize_result = process.request(
            "initialize",
            Some(json!({
                "clientInfo": {
                    "name": "open_cowork",
                    "title": "OpenCowork",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {
                    "experimentalApi": true
                }
            })),
        )?;
        if initialize_result
            .get("userAgent")
            .and_then(Value::as_str)
            .is_none()
        {
            process.stop();
            return Err(
                "Codex App Server handshake did not match the pinned protocol schema".to_string(),
            );
        }
        process.notify("initialized", json!({}))?;
        Ok(process)
    }

    fn profile_home(&self, profile_id: &str) -> Result<PathBuf, String> {
        validate_profile_id(profile_id)?;
        Ok(self.profiles_dir.join(profile_id))
    }

    fn verified_runtime(&self) -> Result<PathBuf, String> {
        let runtime_dir = self.resource_dir.join("codex");
        let manifest_path = runtime_dir.join("runtime-bundle-manifest.json");
        let manifest_bytes = fs::read(&manifest_path).map_err(|_| {
            format!(
                "The bundled Codex {CODEX_VERSION} runtime manifest is missing. System Codex installations are never used."
            )
        })?;
        let manifest: RuntimeManifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|error| format!("The bundled Codex manifest is invalid: {error}"))?;
        if manifest.version != CODEX_VERSION || manifest.protocol_schema != PROTOCOL_SCHEMA {
            return Err(format!(
                "Codex bundle/schema mismatch: expected {CODEX_VERSION}/{PROTOCOL_SCHEMA}, found {}/{}",
                manifest.version, manifest.protocol_schema
            ));
        }
        validate_resource_path(&manifest.binary)?;
        validate_resource_path(&manifest.license)?;
        if !runtime_dir.join(&manifest.license).is_file() {
            return Err("The bundled Codex license file is missing".to_string());
        }
        let binary = runtime_dir.join(&manifest.binary);
        let bytes =
            fs::read(&binary).map_err(|_| "The bundled Codex executable is missing".to_string())?;
        let actual = Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        if !actual.eq_ignore_ascii_case(manifest.sha256.trim()) {
            return Err("The bundled Codex executable failed SHA-256 verification".to_string());
        }
        Ok(binary)
    }
}

impl Drop for CodexRuntimeManager {
    fn drop(&mut self) {
        if let Ok(processes) = self.processes.get_mut() {
            for (_, process) in processes.drain() {
                process.stop();
            }
        }
    }
}

fn validate_profile_id(profile_id: &str) -> Result<(), String> {
    if profile_id.is_empty()
        || profile_id.len() > 128
        || !profile_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err("Invalid Codex profile identifier".to_string());
    }
    Ok(())
}

fn validate_resource_path(name: &str) -> Result<(), String> {
    let path = Path::new(name);
    if name.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err("Codex bundle manifest contains an unsafe resource path".to_string());
    }
    Ok(())
}

fn write_profile_config(profile_home: &Path) -> Result<(), String> {
    let config = "cli_auth_credentials_store = \"keyring\"\ncheck_for_update_on_startup = false\n";
    fs::write(profile_home.join("config.toml"), config)
        .map_err(|error| format!("Could not enforce secure Codex credential storage: {error}"))
}

fn verify_binary_version(binary: &Path, profile_home: &Path) -> Result<(), String> {
    let mut command = Command::new(binary);
    command.arg("--version").env("CODEX_HOME", profile_home);
    remove_secret_environment(&mut command);
    suppress_window(&mut command);
    let output = command
        .output()
        .map_err(|error| format!("Could not inspect the bundled Codex runtime: {error}"))?;
    let version = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() || !version.contains(CODEX_VERSION) {
        return Err(format!(
            "Bundled Codex version mismatch: expected {CODEX_VERSION}"
        ));
    }
    Ok(())
}

fn remove_secret_environment(command: &mut Command) {
    for name in SECRET_ENV_VARS {
        command.env_remove(name);
    }
}

fn format_rpc_error(error: &Value) -> String {
    let code = error.get("code").and_then(Value::as_i64);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("Codex App Server request failed");
    match code {
        Some(code) => format!("Codex App Server error {code}: {message}"),
        None => message.to_string(),
    }
}

#[cfg(target_os = "windows")]
fn suppress_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x08000000);
}

#[cfg(not(target_os = "windows"))]
fn suppress_window(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_ids_cannot_escape_the_isolated_root() {
        assert!(validate_profile_id("account-01").is_ok());
        assert!(validate_profile_id("../account").is_err());
        assert!(validate_profile_id("account/profile").is_err());
    }

    #[test]
    fn manifest_resource_paths_must_be_single_file_names() {
        assert!(validate_resource_path("vendor/bin/codex.exe").is_ok());
        assert!(validate_resource_path("../codex.exe").is_err());
        assert!(validate_resource_path("/absolute/codex").is_err());
    }
}
