use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use tauri::{AppHandle, Manager, Runtime, State};

const EMBEDDED_WINDOWS_PYTHON_RELATIVE_PATH: &str = "python/windows/python.exe";
const EMBEDDED_RUNTIME_SCRIPT_DIR: &str = "python/crew_runtime";
const ENV_CREW_PYTHON: &str = "OPEN_COWORK_CREW_PYTHON";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrewRuntimeStatusResponse {
    pub ready: bool,
    pub bootstrap_required: bool,
    pub embedded_python_available: bool,
    pub crewai_installed: bool,
    pub runtime_root: String,
    pub runtime_scripts_path: String,
    pub requirements_path: String,
    pub embedded_python_path: Option<String>,
    pub detected_python_path: Option<String>,
    pub venv_python_path: Option<String>,
    pub python_version: Option<String>,
    pub crewai_version: Option<String>,
    pub last_bootstrap_at: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrewRuntimeBootstrapRequest {
    #[serde(default)]
    pub force_reinstall: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrewRuntimeBootstrapResponse {
    pub ok: bool,
    pub runtime_root: String,
    pub venv_python_path: Option<String>,
    pub installed_requirements: bool,
    pub message: String,
    pub status: CrewRuntimeStatusResponse,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrewRuntimeValidateRequest {
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrewRuntimeTaskExecutionResult {
    pub task_id: String,
    pub agent_id: String,
    pub status: String,
    pub output: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrewRuntimeExecutionLog {
    pub id: String,
    pub crew_id: String,
    pub agent_id: String,
    pub task_id: String,
    pub action: String,
    pub result: String,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrewRuntimeExecuteResponse {
    pub crew_id: String,
    pub status: String,
    pub task_results: Vec<CrewRuntimeTaskExecutionResult>,
    pub logs: Vec<CrewRuntimeExecutionLog>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrewRuntimeValidateResponse {
    pub valid: bool,
    pub issues: Vec<String>,
    pub normalized: Option<Value>,
}

#[derive(Debug, Default)]
pub struct CrewPythonBridge {
    metadata: Mutex<CrewPythonBridgeMetadata>,
}

#[derive(Debug, Default)]
struct CrewPythonBridgeMetadata {
    last_bootstrap_at: Option<String>,
    active_runs: HashMap<String, String>,
}

impl CrewPythonBridge {
    fn read_last_bootstrap_at(&self) -> Option<String> {
        self.metadata.lock().ok().and_then(|metadata| metadata.last_bootstrap_at.clone())
    }

    fn set_last_bootstrap_at(&self, value: Option<String>) {
        if let Ok(mut metadata) = self.metadata.lock() {
            metadata.last_bootstrap_at = value;
        }
    }

    #[allow(dead_code)]
    fn set_active_run(&self, run_id: String, value: String) {
        if let Ok(mut metadata) = self.metadata.lock() {
            metadata.active_runs.insert(run_id, value);
        }
    }

    fn clear_active_run(&self, run_id: &str) {
        if let Ok(mut metadata) = self.metadata.lock() {
            metadata.active_runs.remove(run_id);
        }
    }
}

fn resolve_runtime_root<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|path| path.join("crew-runtime"))
        .map_err(|error| format!("Crew runtime root konnte nicht aufgeloest werden: {}", error))
}

fn dev_script_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("python")
        .join("crew_runtime")
}

fn resolve_runtime_scripts_path<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    let bundled = app
        .path()
        .resource_dir()
        .ok()
        .map(|path| path.join(EMBEDDED_RUNTIME_SCRIPT_DIR));

    bundled
        .filter(|path| path.exists())
        .unwrap_or_else(dev_script_dir)
}

fn resolve_requirements_path<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    resolve_runtime_scripts_path(app).join("requirements.txt")
}

fn resolve_embedded_python_path<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    app.path()
        .resource_dir()
        .ok()
        .map(|path| path.join(EMBEDDED_WINDOWS_PYTHON_RELATIVE_PATH))
        .filter(|path| path.exists())
}

fn resolve_venv_python_path(runtime_root: &Path) -> PathBuf {
    runtime_root.join("venv").join("Scripts").join("python.exe")
}

fn detect_base_python_command<R: Runtime>(app: &AppHandle<R>) -> Option<String> {
    if let Some(path) = resolve_embedded_python_path(app) {
        return Some(path.display().to_string());
    }

    if let Ok(path) = std::env::var(ENV_CREW_PYTHON) {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    Some("python".to_string())
}

fn command_available(command: &str) -> bool {
    Command::new(command)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn run_python_json_command(python: &Path, script: &Path, subcommand: &str, payload: Option<&Value>) -> Result<Value, String> {
    let mut command = Command::new(python);
    command
        .arg(script)
        .arg(subcommand)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().map_err(|error| format!("Crew runtime Prozess konnte nicht gestartet werden: {}", error))?;

    if let Some(input) = payload {
        let input_json = serde_json::to_vec(input).map_err(|error| error.to_string())?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(&input_json).map_err(|error| format!("Crew runtime stdin fehlgeschlagen: {}", error))?;
        }
    }

    let output = child.wait_with_output().map_err(|error| format!("Crew runtime Prozessfehler: {}", error))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if !output.status.success() {
        let message = if stderr.is_empty() {
            format!("Crew runtime beendete sich mit {}", output.status)
        } else {
            stderr
        };
        return Err(message);
    }

    serde_json::from_str::<Value>(&stdout).map_err(|error| {
        format!(
            "Crew runtime Antwort konnte nicht gelesen werden: {}. Stdout: {}. Stderr: {}",
            error,
            stdout,
            stderr
        )
    })
}

fn build_status_from_json<R: Runtime>(
    app: &AppHandle<R>,
    bridge: &CrewPythonBridge,
    runtime_root: &Path,
    detected_python_path: Option<String>,
    json: Option<Value>,
    message: String,
) -> CrewRuntimeStatusResponse {
    let runtime_scripts_path = resolve_runtime_scripts_path(app);
    let requirements_path = resolve_requirements_path(app);
    let venv_python_path = resolve_venv_python_path(runtime_root);
    let embedded_python_path = resolve_embedded_python_path(app);
    let embedded_python_available = embedded_python_path.is_some();
    let venv_exists = venv_python_path.exists();

    let python_version = json
        .as_ref()
        .and_then(|value| value.get("pythonVersion"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let crewai_version = json
        .as_ref()
        .and_then(|value| value.get("crewaiVersion"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let crewai_installed = json
        .as_ref()
        .and_then(|value| value.get("crewaiInstalled"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let ready = venv_exists && crewai_installed;

    CrewRuntimeStatusResponse {
        ready,
        bootstrap_required: !ready,
        embedded_python_available,
        crewai_installed,
        runtime_root: runtime_root.display().to_string(),
        runtime_scripts_path: runtime_scripts_path.display().to_string(),
        requirements_path: requirements_path.display().to_string(),
        embedded_python_path: embedded_python_path.map(|path| path.display().to_string()),
        detected_python_path,
        venv_python_path: if venv_exists { Some(venv_python_path.display().to_string()) } else { None },
        python_version,
        crewai_version,
        last_bootstrap_at: bridge.read_last_bootstrap_at(),
        message,
    }
}

fn crew_runtime_status_internal<R: Runtime>(
    app: &AppHandle<R>,
    bridge: &CrewPythonBridge,
) -> Result<CrewRuntimeStatusResponse, String> {
    let runtime_root = resolve_runtime_root(&app)?;
    if !runtime_root.exists() {
        fs::create_dir_all(&runtime_root).map_err(|error| format!("Crew runtime root konnte nicht erstellt werden: {}", error))?;
    }

    let scripts_path = resolve_runtime_scripts_path(&app);
    let main_script = scripts_path.join("main.py");
    let base_python = detect_base_python_command(&app);
    let detected_python_path = base_python.clone().filter(|command| command_available(command));

    if !main_script.exists() {
        return Ok(build_status_from_json(
            &app,
            bridge,
            &runtime_root,
            detected_python_path,
            None,
            "Crew runtime Skript fehlt".to_string(),
        ));
    }

    let venv_python = resolve_venv_python_path(&runtime_root);
    let preferred_python = if venv_python.exists() {
        Some(venv_python)
    } else {
        detected_python_path.as_ref().map(PathBuf::from)
    };

    let status_json = preferred_python
        .as_ref()
        .and_then(|python| run_python_json_command(python, &main_script, "status", None).ok());

    let message = if status_json.is_some() {
        "Crew runtime Status erfolgreich geladen".to_string()
    } else if preferred_python.is_none() {
        "Kein Python-Interpreter verfuegbar. Fuer produktive Builds wird die eingebettete Runtime erwartet.".to_string()
    } else {
        "Crew runtime vorhanden, aber noch nicht vorbereitet".to_string()
    };

    Ok(build_status_from_json(
        &app,
        bridge,
        &runtime_root,
        detected_python_path,
        status_json,
        message,
    ))
}

#[tauri::command]
pub fn crew_runtime_status(
    app: AppHandle,
    bridge: State<'_, CrewPythonBridge>,
) -> Result<CrewRuntimeStatusResponse, String> {
    crew_runtime_status_internal(&app, bridge.inner())
}

#[tauri::command]
pub fn crew_runtime_bootstrap(
    app: AppHandle,
    bridge: State<'_, CrewPythonBridge>,
    request: Option<CrewRuntimeBootstrapRequest>,
) -> Result<CrewRuntimeBootstrapResponse, String> {
    let runtime_root = resolve_runtime_root(&app)?;
    fs::create_dir_all(&runtime_root).map_err(|error| format!("Crew runtime root konnte nicht erstellt werden: {}", error))?;
    let venv_root = runtime_root.join("venv");
    let venv_python = resolve_venv_python_path(&runtime_root);
    let requirements_path = resolve_requirements_path(&app);
    let scripts_path = resolve_runtime_scripts_path(&app);
    let main_script = scripts_path.join("main.py");

    if !main_script.exists() {
        return Err(format!("Crew runtime Skript fehlt: {}", main_script.display()));
    }

    let base_python = detect_base_python_command(&app)
        .filter(|command| command_available(command))
        .ok_or_else(|| "Kein Python-Interpreter verfuegbar. Lege fuer Windows-Builds eine eingebettete Runtime unter resources/python/windows ab oder setze OPEN_COWORK_CREW_PYTHON.".to_string())?;

    let force_reinstall = request.as_ref().map(|value| value.force_reinstall).unwrap_or(false);
    if force_reinstall && venv_root.exists() {
        fs::remove_dir_all(&venv_root).map_err(|error| format!("Bestehende Crew runtime konnte nicht entfernt werden: {}", error))?;
    }

    if !venv_python.exists() {
        let status = Command::new(&base_python)
            .args(["-m", "venv", venv_root.to_string_lossy().as_ref()])
            .status()
            .map_err(|error| format!("Crew runtime venv konnte nicht erstellt werden: {}", error))?;
        if !status.success() {
            return Err("Crew runtime venv-Erstellung fehlgeschlagen".to_string());
        }
    }

    let pip_upgrade = Command::new(&venv_python)
        .args(["-m", "pip", "install", "--upgrade", "pip"])
        .status()
        .map_err(|error| format!("pip Upgrade fuer Crew runtime fehlgeschlagen: {}", error))?;
    if !pip_upgrade.success() {
        return Err("pip Upgrade fuer Crew runtime fehlgeschlagen".to_string());
    }

    let install_requirements = Command::new(&venv_python)
        .args([
            "-m",
            "pip",
            "install",
            "-r",
            requirements_path.to_string_lossy().as_ref(),
        ])
        .status()
        .map_err(|error| format!("Crew runtime Requirements konnten nicht installiert werden: {}", error))?;
    if !install_requirements.success() {
        return Err("Crew runtime Requirements konnten nicht installiert werden".to_string());
    }

    bridge.set_last_bootstrap_at(Some(chrono::Utc::now().to_rfc3339()));
    let status = crew_runtime_status_internal(&app, bridge.inner())?;

    Ok(CrewRuntimeBootstrapResponse {
        ok: status.ready,
        runtime_root: runtime_root.display().to_string(),
        venv_python_path: status.venv_python_path.clone(),
        installed_requirements: true,
        message: if status.ready {
            "Crew runtime erfolgreich vorbereitet".to_string()
        } else {
            status.message.clone()
        },
        status,
    })
}

pub fn crew_runtime_execute_request<R: Runtime>(
    app: &AppHandle<R>,
    bridge: &CrewPythonBridge,
    payload: &Value,
) -> Result<CrewRuntimeExecuteResponse, String> {
    let status = crew_runtime_status_internal(app, bridge)?;
    if !status.ready {
        return Err("Crew runtime ist nicht vorbereitet. Fuehre zuerst die Runtime-Initialisierung aus.".to_string());
    }

    let runtime_root = resolve_runtime_root(app)?;
    let venv_python = resolve_venv_python_path(&runtime_root);
    if !venv_python.exists() {
        return Err("Crew runtime ist nicht vorbereitet. Fuehre zuerst die Runtime-Initialisierung aus.".to_string());
    }

    let scripts_path = resolve_runtime_scripts_path(app);
    let main_script = scripts_path.join("main.py");
    if !main_script.exists() {
        return Err(format!("Crew runtime Skript fehlt: {}", main_script.display()));
    }

    let run_id = payload
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("runtime-crew")
        .to_string();
    bridge.set_active_run(run_id.clone(), chrono::Utc::now().to_rfc3339());

    let result = run_python_json_command(&venv_python, &main_script, "execute", Some(payload));
    bridge.clear_active_run(&run_id);

    let response = result?;
    serde_json::from_value::<CrewRuntimeExecuteResponse>(response).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn crew_runtime_validate_definition(
    app: AppHandle,
    request: CrewRuntimeValidateRequest,
) -> Result<CrewRuntimeValidateResponse, String> {
    let runtime_root = resolve_runtime_root(&app)?;
    let venv_python = resolve_venv_python_path(&runtime_root);
    if !venv_python.exists() {
        return Err("Crew runtime ist nicht vorbereitet. Fuehre zuerst die Runtime-Initialisierung aus.".to_string());
    }

    let scripts_path = resolve_runtime_scripts_path(&app);
    let main_script = scripts_path.join("main.py");
    if !main_script.exists() {
        return Err(format!("Crew runtime Skript fehlt: {}", main_script.display()));
    }

    let result = run_python_json_command(&venv_python, &main_script, "validate", Some(&request.payload))?;
    serde_json::from_value::<CrewRuntimeValidateResponse>(result).map_err(|error| error.to_string())
}