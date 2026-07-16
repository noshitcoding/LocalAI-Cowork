use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use tauri::{AppHandle, Manager, Runtime, State};
use zip::ZipArchive;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[cfg(target_os = "windows")]
fn suppress_command_window(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
fn suppress_command_window(_command: &mut Command) {}

const EMBEDDED_WINDOWS_PYTHON_RELATIVE_PATH: &str = "python/windows/python.exe";
const EMBEDDED_WINDOWS_PYTHON_ARCHIVE_RELATIVE_PATH: &str = "python/windows.zip";
const EMBEDDED_RUNTIME_SCRIPT_DIR: &str = "python/crew_runtime";
const EMBEDDED_RUNTIME_WHEELS_ARCHIVE_RELATIVE_PATH: &str = "python/crew_runtime/wheels.zip";
const ENV_CREW_PYTHON: &str = "OPEN_COWORK_CREW_PYTHON";
const MANAGED_PYTHON_VERSION: &str = "3.12";
const UV_VERSION: &str = "0.11.7";
const UV_WINDOWS_DOWNLOAD_URL: &str =
    "https://github.com/astral-sh/uv/releases/download/0.11.7/uv-x86_64-pc-windows-msvc.zip";
const MIN_SUPPORTED_PYTHON_MINOR: u32 = 10;
const MAX_SUPPORTED_PYTHON_MINOR_EXCLUSIVE: u32 = 14;

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
    pub expected_crewai_version: Option<String>,
    pub tool_dependencies_installed: bool,
    pub runtime_compatible: bool,
    pub runtime_schema_version: Option<u64>,
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
    #[serde(default)]
    pub agent_name: Option<String>,
    #[serde(default)]
    pub source_agent: Option<String>,
    #[serde(default)]
    pub target_agent: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub task_title: Option<String>,
    #[serde(default)]
    pub phase: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub severity: Option<String>,
    #[serde(default)]
    pub provider_reasoning: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CrewRuntimeLogEvent {
    pub stream_id: Option<String>,
    pub run_id: Option<String>,
    pub log: CrewRuntimeExecutionLog,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CrewRuntimeProtocolEvent {
    #[serde(rename = "openCoworkEvent")]
    open_cowork_event: String,
    #[serde(default)]
    stream_id: Option<String>,
    #[serde(default)]
    run_id: Option<String>,
    payload: CrewRuntimeExecutionLog,
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
    active_runs: HashMap<String, u32>,
}

impl CrewPythonBridge {
    fn read_last_bootstrap_at(&self) -> Option<String> {
        self.metadata
            .lock()
            .ok()
            .and_then(|metadata| metadata.last_bootstrap_at.clone())
    }

    fn set_last_bootstrap_at(&self, value: Option<String>) {
        if let Ok(mut metadata) = self.metadata.lock() {
            metadata.last_bootstrap_at = value;
        }
    }

    fn set_active_run(&self, run_id: String, pid: u32) {
        if let Ok(mut metadata) = self.metadata.lock() {
            metadata.active_runs.insert(run_id, pid);
        }
    }

    fn clear_active_run(&self, run_id: &str) {
        if let Ok(mut metadata) = self.metadata.lock() {
            metadata.active_runs.remove(run_id);
        }
    }

    pub fn stop_active_run(&self, run_id: &str) -> Result<bool, String> {
        let pid = self
            .metadata
            .lock()
            .map_err(|_| "Crew runtime Metadaten gesperrt".to_string())?
            .active_runs
            .remove(run_id);

        let Some(pid) = pid else {
            return Ok(false);
        };

        #[cfg(target_os = "windows")]
        let status = {
            let mut command = Command::new("taskkill");
            command.args(["/PID", &pid.to_string(), "/T", "/F"]);
            suppress_command_window(&mut command);
            command
        }
        .status()
        .map_err(|error| format!("Crew runtime process could not be stopped: {}", error))?;

        #[cfg(not(target_os = "windows"))]
        let status = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()
            .map_err(|error| format!("Crew runtime process could not be stopped: {}", error))?;

        Ok(status.success())
    }
}

fn resolve_runtime_root<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|path| path.join("crew-runtime"))
        .map_err(|error| format!("Crew runtime root could not be resolved: {}", error))
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
    if let Ok(runtime_root) = resolve_runtime_root(app) {
        let extracted = runtime_root.join(EMBEDDED_WINDOWS_PYTHON_RELATIVE_PATH);
        if extracted.exists() {
            return Some(extracted);
        }
    }

    app.path()
        .resource_dir()
        .ok()
        .map(|path| path.join(EMBEDDED_WINDOWS_PYTHON_RELATIVE_PATH))
        .filter(|path| path.exists())
}

#[cfg(target_os = "windows")]
fn resolve_venv_python_path(runtime_root: &Path) -> PathBuf {
    runtime_root.join("venv").join("Scripts").join("python.exe")
}

#[cfg(not(target_os = "windows"))]
fn resolve_venv_python_path(runtime_root: &Path) -> PathBuf {
    runtime_root.join("venv").join("bin").join("python")
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

fn ensure_compatible_base_python<R: Runtime>(app: &AppHandle<R>) -> Result<String, String> {
    if let Ok(path) = std::env::var(ENV_CREW_PYTHON) {
        let command = path.trim();
        if !command.is_empty() && command_available(command) {
            let version = read_python_version(command)
                .ok_or_else(|| format!("Python version for {} could not be determined", command))?;
            if python_version_supported(&version) {
                return Ok(command.to_string());
            }
            return Err(format!(
                "OPEN_COWORK_CREW_PYTHON zeigt auf Python {}, CrewAI benoetigt Python 3.10 bis 3.13.",
                version
            ));
        }
    }

    if let Some(command) = resolve_embedded_python_path(app)
        .map(|path| path.display().to_string())
        .filter(|command| command_available(command))
    {
        if let Some(version) = read_python_version(&command) {
            if python_version_supported(&version) {
                return Ok(command);
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        let runtime_root = resolve_runtime_root(app)?;
        let uv = ensure_managed_uv(app)?;
        install_managed_python(&uv, &runtime_root)?;
        Ok(uv)
    }

    #[cfg(not(target_os = "windows"))]
    {
        Err(
            "No compatible Python interpreter available. Python 3.10 through 3.13 is supported."
                .to_string(),
        )
    }
}

#[cfg(target_os = "windows")]
fn resolve_uv_path(runtime_root: &Path) -> PathBuf {
    runtime_root.join("tools").join("uv").join("uv.exe")
}

#[cfg(target_os = "windows")]
fn ensure_managed_uv<R: Runtime>(app: &AppHandle<R>) -> Result<String, String> {
    let runtime_root = resolve_runtime_root(app)?;
    let uv_exe = resolve_uv_path(&runtime_root);
    if uv_exe.exists() {
        return Ok(uv_exe.display().to_string());
    }

    let uv_dir = uv_exe
        .parent()
        .ok_or_else(|| "uv target folder could not be resolved".to_string())?;
    fs::create_dir_all(uv_dir)
        .map_err(|error| format!("uv target folder could not be created: {}", error))?;
    let downloads_dir = runtime_root.join("downloads");
    fs::create_dir_all(&downloads_dir)
        .map_err(|error| format!("download folder for uv could not be created: {}", error))?;
    let archive_path = downloads_dir.join(format!("uv-{}-x86_64-pc-windows-msvc.zip", UV_VERSION));

    if !archive_path.exists() {
        let response = reqwest::blocking::get(UV_WINDOWS_DOWNLOAD_URL)
            .map_err(|error| format!("uv {} could not be downloaded: {}", UV_VERSION, error))?;
        if !response.status().is_success() {
            return Err(format!(
                "uv {} Download fehlgeschlagen: HTTP {}",
                UV_VERSION,
                response.status()
            ));
        }
        let bytes = response
            .bytes()
            .map_err(|error| format!("uv download could not be read: {}", error))?;
        fs::write(&archive_path, bytes)
            .map_err(|error| format!("uv archive could not be saved: {}", error))?;
    }

    extract_file_from_zip(&archive_path, "uv.exe", &uv_exe)?;
    if !uv_exe.exists() {
        return Err(format!("uv was not found: {}", uv_exe.display()));
    }

    Ok(uv_exe.display().to_string())
}

#[cfg(target_os = "windows")]
fn install_managed_python(uv: &str, runtime_root: &Path) -> Result<(), String> {
    let python_install_dir = runtime_root.join("python").join("managed");
    fs::create_dir_all(&python_install_dir)
        .map_err(|error| format!("Python installation folder could not be created: {}", error))?;

    let mut command = Command::new(uv);
    command
        .args(["python", "install", MANAGED_PYTHON_VERSION])
        .env("UV_PYTHON_INSTALL_DIR", &python_install_dir)
        .env("UV_CACHE_DIR", runtime_root.join("cache").join("uv"));
    suppress_command_window(&mut command);
    let status = command.status().map_err(|error| {
        format!(
            "App-internal Python download could not be started: {}",
            error
        )
    })?;
    if !status.success() {
        return Err(format!(
            "App-internal Python download beendete sich mit {}",
            status
        ));
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn extract_file_from_zip(
    zip_path: &Path,
    file_name: &str,
    destination: &Path,
) -> Result<(), String> {
    let file = fs::File::open(zip_path).map_err(|error| {
        format!(
            "archive could not be opened ({}): {}",
            zip_path.display(),
            error
        )
    })?;
    let mut archive = ZipArchive::new(file).map_err(|error| {
        format!(
            "archive could not be read ({}): {}",
            zip_path.display(),
            error
        )
    })?;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| {
            format!(
                "archive entry could not be read ({}): {}",
                zip_path.display(),
                error
            )
        })?;
        let Some(entry_path) = entry.enclosed_name() else {
            continue;
        };
        if entry_path.file_name().and_then(|value| value.to_str()) != Some(file_name) {
            continue;
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "target folder could not be created ({}): {}",
                    parent.display(),
                    error
                )
            })?;
        }
        let mut output = fs::File::create(destination).map_err(|error| {
            format!(
                "file could not be written ({}): {}",
                destination.display(),
                error
            )
        })?;
        std::io::copy(&mut entry, &mut output).map_err(|error| {
            format!(
                "file could not be extracted ({}): {}",
                destination.display(),
                error
            )
        })?;
        return Ok(());
    }

    Err(format!(
        "{} was not found in archive {}",
        file_name,
        zip_path.display()
    ))
}

fn command_available(command: &str) -> bool {
    let mut command = Command::new(command);
    command
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    suppress_command_window(&mut command);
    command
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn read_python_version(command: &str) -> Option<String> {
    let mut command = Command::new(command);
    command
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    suppress_command_window(&mut command);
    let output = command.output().ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let combined = if stdout.is_empty() { stderr } else { stdout };
    let version = combined.strip_prefix("Python ")?.trim().to_string();
    if version.is_empty() {
        return None;
    }

    Some(version)
}

fn python_version_supported(version: &str) -> bool {
    let mut parts = version.split('.');
    let major = parts.next().and_then(|value| value.parse::<u32>().ok());
    let minor = parts.next().and_then(|value| value.parse::<u32>().ok());

    matches!((major, minor), (Some(3), Some(minor)) if (MIN_SUPPORTED_PYTHON_MINOR..MAX_SUPPORTED_PYTHON_MINOR_EXCLUSIVE).contains(&minor))
}

fn resolve_local_wheels_path<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    if let Ok(runtime_root) = resolve_runtime_root(app) {
        let extracted = runtime_root
            .join("python")
            .join("crew_runtime")
            .join("wheels");
        if extracted.exists() {
            return extracted;
        }
    }

    resolve_runtime_scripts_path(app).join("wheels")
}

fn local_wheels_available(path: &Path) -> bool {
    std::fs::read_dir(path)
        .ok()
        .map(|entries| {
            entries.filter_map(Result::ok).any(|entry| {
                let file_path = entry.path();
                file_path.is_file()
                    && matches!(
                        file_path.extension().and_then(|value| value.to_str()),
                        Some("whl") | Some("zip")
                    )
            })
        })
        .unwrap_or(false)
}

fn ensure_bundled_runtime_assets<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let resource_dir = match app.path().resource_dir() {
        Ok(path) => path,
        Err(_) => return Ok(()),
    };

    let runtime_root = resolve_runtime_root(app)?;
    fs::create_dir_all(&runtime_root)
        .map_err(|error| format!("Crew runtime root could not be created: {}", error))?;

    let python_archive = resource_dir.join(EMBEDDED_WINDOWS_PYTHON_ARCHIVE_RELATIVE_PATH);
    if python_archive.exists() {
        extract_zip_if_needed(
            &python_archive,
            &runtime_root.join("python").join("windows"),
        )?;
    }

    let wheels_archive = resource_dir.join(EMBEDDED_RUNTIME_WHEELS_ARCHIVE_RELATIVE_PATH);
    if wheels_archive.exists() {
        extract_zip_if_needed(
            &wheels_archive,
            &runtime_root
                .join("python")
                .join("crew_runtime")
                .join("wheels"),
        )?;
    }

    Ok(())
}

fn extract_zip_if_needed(zip_path: &Path, destination: &Path) -> Result<(), String> {
    let marker = destination.join(".open_cowork_extract_complete");
    let zip_metadata = fs::metadata(zip_path).map_err(|error| {
        format!(
            "archive could not be read ({}): {}",
            zip_path.display(),
            error
        )
    })?;
    let zip_len = zip_metadata.len().to_string();
    let zip_modified = zip_metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|value| value.as_secs().to_string())
        .unwrap_or_else(|| "0".to_string());
    let expected_marker = format!("{}:{}", zip_len, zip_modified);

    if marker.exists() {
        if let Ok(current_marker) = fs::read_to_string(&marker) {
            if current_marker.trim() == expected_marker {
                return Ok(());
            }
        }
        fs::remove_dir_all(destination).map_err(|error| {
            format!(
                "Outdated archive data could not be removed ({}): {}",
                destination.display(),
                error
            )
        })?;
    }

    fs::create_dir_all(destination).map_err(|error| {
        format!(
            "target folder for archive could not be created ({}): {}",
            destination.display(),
            error
        )
    })?;

    let file = fs::File::open(zip_path).map_err(|error| {
        format!(
            "archive could not be opened ({}): {}",
            zip_path.display(),
            error
        )
    })?;
    let mut archive = ZipArchive::new(file).map_err(|error| {
        format!(
            "archive could not be read ({}): {}",
            zip_path.display(),
            error
        )
    })?;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| {
            format!(
                "archive entry could not be read ({}): {}",
                zip_path.display(),
                error
            )
        })?;
        let Some(entry_name) = entry.enclosed_name() else {
            continue;
        };
        let output_path = destination.join(entry_name);

        if entry.is_dir() {
            fs::create_dir_all(&output_path).map_err(|error| {
                format!(
                    "archive folder could not be created ({}): {}",
                    output_path.display(),
                    error
                )
            })?;
            continue;
        }

        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "archive target could not be created ({}): {}",
                    parent.display(),
                    error
                )
            })?;
        }

        let mut output = fs::File::create(&output_path).map_err(|error| {
            format!(
                "archive file could not be written ({}): {}",
                output_path.display(),
                error
            )
        })?;
        std::io::copy(&mut entry, &mut output).map_err(|error| {
            format!(
                "archive file could not be extracted ({}): {}",
                output_path.display(),
                error
            )
        })?;
    }

    fs::write(&marker, expected_marker).map_err(|error| {
        format!(
            "archive marker could not be written ({}): {}",
            marker.display(),
            error
        )
    })?;
    Ok(())
}

fn run_python_json_command(
    python: &Path,
    script: &Path,
    subcommand: &str,
    payload: Option<&Value>,
    active_run: Option<(&CrewPythonBridge, &str)>,
) -> Result<Value, String> {
    run_python_json_command_with_events(
        python,
        script,
        subcommand,
        payload,
        active_run,
        |_event| {},
    )
}

fn run_python_json_command_with_events<F>(
    python: &Path,
    script: &Path,
    subcommand: &str,
    payload: Option<&Value>,
    active_run: Option<(&CrewPythonBridge, &str)>,
    mut on_log_event: F,
) -> Result<Value, String>
where
    F: FnMut(CrewRuntimeLogEvent),
{
    let mut command = Command::new(python);
    command
        .arg(script)
        .arg(subcommand)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    suppress_command_window(&mut command);

    let mut child = command
        .spawn()
        .map_err(|error| format!("Crew runtime process could not be started: {}", error))?;
    let active_key = active_run.map(|(bridge, run_id)| {
        bridge.set_active_run(run_id.to_string(), child.id());
        (bridge, run_id.to_string())
    });

    if let Some(input) = payload {
        let input_json = serde_json::to_vec(input).map_err(|error| error.to_string())?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(&input_json).map_err(|error| {
                if let Some((bridge, run_id)) = &active_key {
                    bridge.clear_active_run(run_id);
                }
                let _ = child.kill();
                format!("Crew runtime stdin fehlgeschlagen: {}", error)
            })?;
        }
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Crew runtime stdout could not be read".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Crew runtime stderr could not be read".to_string())?;

    let stderr_handle = thread::spawn(move || {
        let mut output = String::new();
        for line in read_lossy_lines(stderr) {
            output.push_str(&line);
            output.push('\n');
        }
        output
    });

    let mut stdout_lines = Vec::new();
    for line in read_lossy_lines(stdout) {
        if let Some(event) = parse_runtime_protocol_event(&line) {
            on_log_event(event);
        } else {
            stdout_lines.push(line);
        }
    }

    let status = child
        .wait()
        .map_err(|error| format!("Crew runtime processfehler: {}", error))?;
    if let Some((bridge, run_id)) = &active_key {
        bridge.clear_active_run(run_id);
    }
    let stdout = stdout_lines.join("\n").trim().to_string();
    let stderr = stderr_handle
        .join()
        .unwrap_or_else(|_| "Crew runtime stderr Thread fehlgeschlagen".to_string())
        .trim()
        .to_string();

    if !status.success() {
        let message = if stderr.is_empty() {
            format!("Crew runtime beendete sich mit {}", status)
        } else {
            stderr
        };
        return Err(message);
    }

    parse_python_json_stdout(&stdout, &stderr)
}

fn read_lossy_lines<R: std::io::Read>(reader: R) -> impl Iterator<Item = String> {
    BufReader::new(reader)
        .split(b'\n')
        .filter_map(|line| line.ok())
        .map(|mut bytes| {
            if bytes.ends_with(b"\r") {
                bytes.pop();
            }
            String::from_utf8_lossy(&bytes).into_owned()
        })
}

fn parse_runtime_protocol_event(line: &str) -> Option<CrewRuntimeLogEvent> {
    let normalized = line.trim();
    if normalized.is_empty() || !normalized.contains("\"openCoworkEvent\"") {
        return None;
    }

    let event = serde_json::from_str::<CrewRuntimeProtocolEvent>(normalized).ok()?;
    if event.open_cowork_event != "crew_log" {
        return None;
    }

    Some(CrewRuntimeLogEvent {
        stream_id: event.stream_id,
        run_id: event.run_id,
        log: event.payload,
    })
}

fn parse_python_json_stdout(stdout: &str, stderr: &str) -> Result<Value, String> {
    let trimmed = stdout.trim();
    let mut deserializer = serde_json::Deserializer::from_str(trimmed);
    if let Ok(value) = Value::deserialize(&mut deserializer) {
        return Ok(value);
    }

    for line in trimmed.lines().rev() {
        let candidate = line.trim();
        if candidate.is_empty() || candidate.contains("\"openCoworkEvent\"") {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(candidate) {
            return Ok(value);
        }
    }

    let mut deserializer = serde_json::Deserializer::from_str(trimmed);
    Value::deserialize(&mut deserializer).map_err(|error| {
        format!(
            "Crew runtime response could not be read: {}. Stdout: {}. Stderr: {}",
            error, stdout, stderr
        )
    })
}

fn build_status_from_json<R: Runtime>(
    app: &AppHandle<R>,
    bridge: &CrewPythonBridge,
    runtime_root: &Path,
    detected_python_path: Option<String>,
    detected_python_version: Option<String>,
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
        .map(ToString::to_string)
        .or(detected_python_version);
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
    let expected_crewai_version = json
        .as_ref()
        .and_then(|value| value.get("expectedCrewaiVersion"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let tool_dependencies_installed = json
        .as_ref()
        .and_then(|value| value.get("toolDependenciesInstalled"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let runtime_compatible = json
        .as_ref()
        .and_then(|value| value.get("runtimeCompatible"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let runtime_schema_version = json
        .as_ref()
        .and_then(|value| value.get("runtimeSchemaVersion"))
        .and_then(Value::as_u64);
    let runtime_message = json
        .as_ref()
        .and_then(|value| value.get("runtimeMessage"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or(message);
    let ready = venv_exists && crewai_installed && runtime_compatible;

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
        venv_python_path: if venv_exists {
            Some(venv_python_path.display().to_string())
        } else {
            None
        },
        python_version,
        crewai_version,
        expected_crewai_version,
        tool_dependencies_installed,
        runtime_compatible,
        runtime_schema_version,
        last_bootstrap_at: bridge.read_last_bootstrap_at(),
        message: runtime_message,
    }
}

fn crew_runtime_status_internal<R: Runtime>(
    app: &AppHandle<R>,
    bridge: &CrewPythonBridge,
) -> Result<CrewRuntimeStatusResponse, String> {
    ensure_bundled_runtime_assets(app)?;
    let runtime_root = resolve_runtime_root(app)?;
    if !runtime_root.exists() {
        fs::create_dir_all(&runtime_root)
            .map_err(|error| format!("Crew runtime root could not be created: {}", error))?;
    }

    let scripts_path = resolve_runtime_scripts_path(app);
    let main_script = scripts_path.join("main.py");
    let venv_python = resolve_venv_python_path(&runtime_root);
    let venv_exists = venv_python.exists();
    let base_python = if venv_exists {
        Some(venv_python.display().to_string())
    } else {
        detect_base_python_command(app)
            .filter(|command| command != "python")
            .filter(|command| command_available(command))
    };
    let detected_python_path = base_python.clone();
    let detected_python_version = detected_python_path
        .as_ref()
        .and_then(|command| read_python_version(command));
    let python_compatible = detected_python_version
        .as_deref()
        .map(python_version_supported)
        .unwrap_or(false);

    if !main_script.exists() {
        return Ok(build_status_from_json(
            app,
            bridge,
            &runtime_root,
            detected_python_path,
            detected_python_version,
            None,
            "Crew runtime Skript fehlt".to_string(),
        ));
    }

    let preferred_python = if venv_python.exists() {
        Some(venv_python)
    } else if python_compatible {
        detected_python_path.as_ref().map(PathBuf::from)
    } else {
        None
    };

    let status_json = preferred_python.as_ref().and_then(|python| {
        run_python_json_command(python, &main_script, "status", None, None).ok()
    });

    let message = if status_json.is_some() {
        "Crew runtime status loaded successfully".to_string()
    } else if detected_python_path.is_some() && !python_compatible {
        format!(
            "Detected Python interpreter ({}) is not compatible with CrewAI. The app-internal runtime will be prepared with Python 3.12 during initialization.",
            detected_python_version
                .clone()
                .unwrap_or_else(|| "unbekannt".to_string())
        )
    } else if preferred_python.is_none() {
        "Crew runtime must be initialized. Python 3.12 and CrewAI will be downloaded in isolation into the app data folder.".to_string()
    } else {
        "Crew runtime exists but is not prepared yet".to_string()
    };

    Ok(build_status_from_json(
        app,
        bridge,
        &runtime_root,
        detected_python_path,
        detected_python_version,
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
    ensure_bundled_runtime_assets(&app)?;
    let runtime_root = resolve_runtime_root(&app)?;
    fs::create_dir_all(&runtime_root)
        .map_err(|error| format!("Crew runtime root could not be created: {}", error))?;
    let venv_root = runtime_root.join("venv");
    let venv_python = resolve_venv_python_path(&runtime_root);
    let requirements_path = resolve_requirements_path(&app);
    let wheels_path = resolve_local_wheels_path(&app);
    let scripts_path = resolve_runtime_scripts_path(&app);
    let main_script = scripts_path.join("main.py");

    if !main_script.exists() {
        return Err(format!(
            "Crew runtime Skript fehlt: {}",
            main_script.display()
        ));
    }

    let base_python = ensure_compatible_base_python(&app)?;
    let use_local_wheels = local_wheels_available(&wheels_path);

    let force_reinstall = request
        .as_ref()
        .map(|value| value.force_reinstall)
        .unwrap_or(false);
    let venv_python_supported = if venv_python.exists() {
        read_python_version(venv_python.to_string_lossy().as_ref())
            .as_deref()
            .map(python_version_supported)
            .unwrap_or(false)
    } else {
        true
    };
    if venv_root.exists() && (force_reinstall || !venv_python_supported) {
        fs::remove_dir_all(&venv_root)
            .map_err(|error| format!("Existing crew runtime could not be removed: {}", error))?;
    }

    if !venv_python.exists() {
        let mut command = Command::new(&base_python);
        if base_python.ends_with("uv.exe") {
            command
                .args([
                    "venv",
                    "--python",
                    MANAGED_PYTHON_VERSION,
                    venv_root.to_string_lossy().as_ref(),
                ])
                .env(
                    "UV_PYTHON_INSTALL_DIR",
                    runtime_root.join("python").join("managed"),
                )
                .env("UV_CACHE_DIR", runtime_root.join("cache").join("uv"));
        } else {
            command.args(["-m", "venv", venv_root.to_string_lossy().as_ref()]);
        }
        suppress_command_window(&mut command);
        let status = command
            .status()
            .map_err(|error| format!("Crew runtime venv could not be created: {}", error))?;
        if !status.success() {
            return Err("Crew runtime venv-Erstellung fehlgeschlagen".to_string());
        }
    }

    if !use_local_wheels && !base_python.ends_with("uv.exe") {
        let mut pip_upgrade_command = Command::new(&venv_python);
        pip_upgrade_command.args(["-m", "pip", "install", "--upgrade", "pip"]);
        suppress_command_window(&mut pip_upgrade_command);
        let pip_upgrade = pip_upgrade_command
            .status()
            .map_err(|error| format!("pip upgrade for crew runtime failed: {}", error))?;
        if !pip_upgrade.success() {
            return Err("pip upgrade for crew runtime failed".to_string());
        }
    }

    let requirements_path_arg = requirements_path.to_string_lossy().to_string();
    let wheels_path_arg = wheels_path.to_string_lossy().to_string();
    let mut install_requirements_command = if base_python.ends_with("uv.exe") {
        let mut command = Command::new(&base_python);
        command
            .args([
                "pip",
                "install",
                "--python",
                venv_python.to_string_lossy().as_ref(),
            ])
            .env(
                "UV_PYTHON_INSTALL_DIR",
                runtime_root.join("python").join("managed"),
            )
            .env("UV_CACHE_DIR", runtime_root.join("cache").join("uv"));
        command
    } else {
        let mut command = Command::new(&venv_python);
        command.args(["-m", "pip", "install"]);
        command
    };
    if use_local_wheels {
        install_requirements_command.args(["--no-index", "--find-links", wheels_path_arg.as_str()]);
    }
    install_requirements_command.args(["-r", requirements_path_arg.as_str()]);
    suppress_command_window(&mut install_requirements_command);

    let install_requirements = install_requirements_command.status().map_err(|error| {
        format!(
            "Crew runtime requirements could not be installed: {}",
            error
        )
    })?;
    if !install_requirements.success() {
        return Err("Crew runtime requirements could not be installed".to_string());
    }

    bridge.set_last_bootstrap_at(Some(chrono::Utc::now().to_rfc3339()));
    let status = crew_runtime_status_internal(&app, bridge.inner())?;

    Ok(CrewRuntimeBootstrapResponse {
        ok: status.ready,
        runtime_root: runtime_root.display().to_string(),
        venv_python_path: status.venv_python_path.clone(),
        installed_requirements: true,
        message: if status.ready {
            "Crew runtime prepared successfully".to_string()
        } else {
            status.message.clone()
        },
        status,
    })
}

pub fn crew_runtime_execute_request<R: Runtime, F>(
    app: &AppHandle<R>,
    bridge: &CrewPythonBridge,
    payload: &Value,
    on_log_event: F,
) -> Result<CrewRuntimeExecuteResponse, String>
where
    F: FnMut(CrewRuntimeLogEvent),
{
    let status = crew_runtime_status_internal(app, bridge)?;
    if !status.ready {
        return Err("Crew runtime is not prepared. Run runtime initialization first.".to_string());
    }

    let runtime_root = resolve_runtime_root(app)?;
    let venv_python = resolve_venv_python_path(&runtime_root);
    if !venv_python.exists() {
        return Err("Crew runtime is not prepared. Run runtime initialization first.".to_string());
    }

    let scripts_path = resolve_runtime_scripts_path(app);
    let main_script = scripts_path.join("main.py");
    if !main_script.exists() {
        return Err(format!(
            "Crew runtime Skript fehlt: {}",
            main_script.display()
        ));
    }

    let run_id = payload
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("runtime-crew")
        .to_string();
    let result = run_python_json_command_with_events(
        &venv_python,
        &main_script,
        "execute",
        Some(payload),
        Some((bridge, &run_id)),
        on_log_event,
    );

    let response = result?;
    serde_json::from_value::<CrewRuntimeExecuteResponse>(response)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn crew_runtime_validate_definition(
    app: AppHandle,
    request: CrewRuntimeValidateRequest,
) -> Result<CrewRuntimeValidateResponse, String> {
    let runtime_root = resolve_runtime_root(&app)?;
    let venv_python = resolve_venv_python_path(&runtime_root);
    if !venv_python.exists() {
        return Err("Crew runtime is not prepared. Run runtime initialization first.".to_string());
    }

    let scripts_path = resolve_runtime_scripts_path(&app);
    let main_script = scripts_path.join("main.py");
    if !main_script.exists() {
        return Err(format!(
            "Crew runtime Skript fehlt: {}",
            main_script.display()
        ));
    }

    let result = run_python_json_command(
        &venv_python,
        &main_script,
        "validate",
        Some(&request.payload),
        None,
    )?;
    serde_json::from_value::<CrewRuntimeValidateResponse>(result).map_err(|error| error.to_string())
}
