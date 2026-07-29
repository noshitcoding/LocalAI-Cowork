use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
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
const EMBEDDED_RUNTIME_MANIFEST_RELATIVE_PATH: &str =
    "python/crew_runtime/runtime-bundle-manifest.json";
const ENV_CREW_PYTHON: &str = "LOCALAI_COWORK_CREW_PYTHON";
const LEGACY_ENV_CREW_PYTHON: &str = "OPEN_COWORK_CREW_PYTHON";
const MANAGED_PYTHON_VERSION: &str = "3.12";
const EXPECTED_BUNDLED_PYTHON_VERSION: &str = "3.12.10";
const EXPECTED_RUNTIME_BUNDLE_SCHEMA_VERSION: u64 = 1;
const EXPECTED_RUNTIME_SCHEMA_VERSION: u64 = 2;
const UV_VERSION: &str = "0.11.7";
const UV_WINDOWS_DOWNLOAD_URL: &str =
    "https://github.com/astral-sh/uv/releases/download/0.11.7/uv-x86_64-pc-windows-msvc.zip";
const MIN_SUPPORTED_PYTHON_MINOR: u32 = 10;
const MAX_SUPPORTED_PYTHON_MINOR_EXCLUSIVE: u32 = 14;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CrewRuntimeBundleManifest {
    schema_version: u64,
    python: CrewRuntimeBundlePython,
    wheelhouse: CrewRuntimeBundleWheelhouse,
    #[serde(default)]
    packages: Vec<CrewRuntimeBundlePackage>,
    smoke: CrewRuntimeBundleSmoke,
}

#[derive(Debug, Clone, Deserialize)]
struct CrewRuntimeBundlePython {
    version: String,
    archive: CrewRuntimeBundleArchive,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CrewRuntimeBundleWheelhouse {
    requirements_sha256: String,
    lock_sha256: String,
    archive: CrewRuntimeBundleArchive,
}

#[derive(Debug, Clone, Deserialize)]
struct CrewRuntimeBundleArchive {
    bytes: u64,
    sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
struct CrewRuntimeBundlePackage {
    name: String,
    version: String,
    filename: String,
    sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CrewRuntimeBundleSmoke {
    verified: bool,
    offline: bool,
    tests_passed: bool,
    python_version: String,
    crewai_version: String,
    runtime_compatible: bool,
    tool_dependencies_installed: bool,
    runtime_schema_version: u64,
}

#[derive(Debug, Clone)]
struct ValidatedCrewRuntimeBundle {
    python_version: String,
    crewai_version: String,
    python_archive_sha256: String,
    wheels_archive_sha256: String,
    packages: Vec<CrewRuntimeBundlePackage>,
}

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
    #[serde(rename = "localAiCoworkEvent")]
    localai_cowork_event: String,
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

    if let Some(path) = bundled.as_ref().filter(|path| path.exists()) {
        return path.clone();
    }
    if cfg!(debug_assertions) {
        return dev_script_dir();
    }

    bundled.unwrap_or_else(|| PathBuf::from(EMBEDDED_RUNTIME_SCRIPT_DIR))
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

fn configured_crew_python() -> Option<String> {
    [ENV_CREW_PYTHON, LEGACY_ENV_CREW_PYTHON]
        .iter()
        .find_map(|name| {
            std::env::var(name).ok().and_then(|path| {
                let trimmed = path.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            })
        })
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

    if let Some(path) = configured_crew_python() {
        return Some(path);
    }

    Some("python".to_string())
}

fn python_version_matches(version: &str, expected_exact_version: Option<&str>) -> bool {
    expected_exact_version
        .map(|expected| version == expected)
        .unwrap_or_else(|| python_version_supported(version))
}

fn select_exact_python_command(
    expected_version: &str,
    embedded: Option<(String, String)>,
    configured: Option<(String, String)>,
) -> Option<String> {
    [embedded, configured]
        .into_iter()
        .flatten()
        .find_map(|(command, version)| (version == expected_version).then_some(command))
}

fn detect_python_candidate(command: Option<String>) -> Option<(String, String)> {
    let command = command?;
    if !command_available(&command) {
        return None;
    }
    read_python_version(&command).map(|version| (command, version))
}

fn ensure_compatible_base_python<R: Runtime>(
    app: &AppHandle<R>,
    expected_exact_version: Option<&str>,
) -> Result<String, String> {
    if let Some(expected_version) = expected_exact_version {
        let embedded = detect_python_candidate(
            resolve_embedded_python_path(app).map(|path| path.display().to_string()),
        );
        let configured = detect_python_candidate(configured_crew_python());
        if let Some(command) = select_exact_python_command(expected_version, embedded, configured) {
            return Ok(command);
        }
        return Err(format!(
            "The verified CrewAI release bundle requires its embedded Python {}, but that interpreter is unavailable or incompatible. Network fallback is disabled.",
            expected_version
        ));
    }

    if let Some(command) = configured_crew_python() {
        if command_available(&command) {
            let version = read_python_version(&command)
                .ok_or_else(|| format!("Python version for {} could not be determined", command))?;
            if python_version_matches(&version, None) {
                return Ok(command);
            }
            return Err(format!(
                "LOCALAI_COWORK_CREW_PYTHON points to Python {}, but CrewAI requires Python 3.10 through 3.13.",
                version
            ));
        }
    }

    if let Some(command) = resolve_embedded_python_path(app)
        .map(|path| path.display().to_string())
        .filter(|command| command_available(command))
    {
        if let Some(version) = read_python_version(&command) {
            if python_version_matches(&version, None) {
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

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path)
        .map_err(|error| format!("{} could not be opened: {}", path.display(), error))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];

    loop {
        let bytes_read = file
            .read(&mut buffer)
            .map_err(|error| format!("{} could not be read: {}", path.display(), error))?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(digest.len() * 2);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in digest {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Ok(encoded)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn normalized_distribution_name(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace(['_', '.'], "-")
}

fn parse_exact_crewai_requirement(path: &Path) -> Result<String, String> {
    let requirements = fs::read_to_string(path).map_err(|error| {
        format!(
            "Crew runtime requirements could not be read ({}): {}",
            path.display(),
            error
        )
    })?;
    let mut versions = Vec::new();

    for line in requirements.lines() {
        let requirement = line
            .split_once('#')
            .map(|(value, _)| value)
            .unwrap_or(line)
            .trim();
        let Some((name, version)) = requirement.split_once("==") else {
            continue;
        };
        let distribution = name.split_once('[').map(|(value, _)| value).unwrap_or(name);
        if normalized_distribution_name(distribution) == "crewai" {
            let version = version
                .split_once(';')
                .map(|(value, _)| value)
                .unwrap_or(version)
                .trim();
            if version.is_empty() {
                return Err("CrewAI must be pinned to an exact non-empty version".to_string());
            }
            versions.push(version.to_string());
        }
    }

    if versions.len() != 1 {
        return Err(format!(
            "requirements.txt must contain exactly one CrewAI == pin, found {}",
            versions.len()
        ));
    }

    Ok(versions.remove(0))
}

fn parse_runtime_script_crewai_version(path: &Path) -> Result<String, String> {
    let script = fs::read_to_string(path).map_err(|error| {
        format!(
            "Crew runtime script could not be read ({}): {}",
            path.display(),
            error
        )
    })?;

    for line in script.lines() {
        let trimmed = line.trim();
        let Some((name, value)) = trimmed.split_once('=') else {
            continue;
        };
        if name.trim() != "EXPECTED_CREWAI_VERSION" {
            continue;
        }
        let version = value
            .trim()
            .trim_matches(|character| character == '"' || character == '\'');
        if !version.is_empty() {
            return Ok(version.to_string());
        }
    }

    Err(format!(
        "{} does not declare EXPECTED_CREWAI_VERSION",
        path.display()
    ))
}

fn validate_bundle_archive(
    path: &Path,
    descriptor: &CrewRuntimeBundleArchive,
    label: &str,
) -> Result<String, String> {
    if !valid_sha256(&descriptor.sha256) {
        return Err(format!(
            "CrewAI release bundle manifest contains an invalid {} SHA-256",
            label
        ));
    }
    let metadata = fs::metadata(path).map_err(|error| {
        format!(
            "CrewAI release bundle is incomplete: {} is missing ({}). Network fallback is disabled.",
            path.display(),
            error
        )
    })?;
    if metadata.len() != descriptor.bytes {
        return Err(format!(
            "CrewAI release bundle {} size mismatch: expected {}, found {}",
            label,
            descriptor.bytes,
            metadata.len()
        ));
    }

    let expected = descriptor.sha256.to_ascii_lowercase();
    let actual = sha256_file(path)?;
    if actual != expected {
        return Err(format!(
            "CrewAI release bundle {} SHA-256 mismatch: expected {}, found {}",
            label, expected, actual
        ));
    }
    Ok(expected)
}

fn bundled_runtime_indicated(resource_dir: &Path) -> bool {
    [
        resource_dir.join(EMBEDDED_RUNTIME_MANIFEST_RELATIVE_PATH),
        resource_dir.join(EMBEDDED_WINDOWS_PYTHON_ARCHIVE_RELATIVE_PATH),
        resource_dir.join(EMBEDDED_RUNTIME_WHEELS_ARCHIVE_RELATIVE_PATH),
        resource_dir
            .join(EMBEDDED_RUNTIME_SCRIPT_DIR)
            .join("main.py"),
        resource_dir
            .join(EMBEDDED_RUNTIME_SCRIPT_DIR)
            .join("requirements.txt"),
        resource_dir
            .join(EMBEDDED_RUNTIME_SCRIPT_DIR)
            .join("requirements.lock"),
    ]
    .iter()
    .any(|path| path.exists())
}

fn validate_bundled_runtime(
    resource_dir: &Path,
) -> Result<Option<ValidatedCrewRuntimeBundle>, String> {
    let python_archive = resource_dir.join(EMBEDDED_WINDOWS_PYTHON_ARCHIVE_RELATIVE_PATH);
    let wheels_archive = resource_dir.join(EMBEDDED_RUNTIME_WHEELS_ARCHIVE_RELATIVE_PATH);
    validate_bundled_runtime_with_archives(resource_dir, &python_archive, &wheels_archive)
}

fn validate_bundled_runtime_with_archives(
    resource_dir: &Path,
    python_archive: &Path,
    wheels_archive: &Path,
) -> Result<Option<ValidatedCrewRuntimeBundle>, String> {
    if !bundled_runtime_indicated(resource_dir) {
        if cfg!(debug_assertions) {
            return Ok(None);
        }
        return Err(
            "CrewAI release bundle is missing completely. Network fallback is disabled."
                .to_string(),
        );
    }

    let manifest_path = resource_dir.join(EMBEDDED_RUNTIME_MANIFEST_RELATIVE_PATH);
    let manifest_json = fs::read_to_string(&manifest_path).map_err(|error| {
        format!(
            "CrewAI release bundle is incomplete: {} is missing ({}). Network fallback is disabled.",
            manifest_path.display(),
            error
        )
    })?;
    let manifest: CrewRuntimeBundleManifest =
        serde_json::from_str(manifest_json.trim_start_matches('\u{feff}')).map_err(|error| {
            format!(
                "CrewAI release bundle manifest is invalid ({}): {}. Network fallback is disabled.",
                manifest_path.display(),
                error
            )
        })?;

    if manifest.schema_version != EXPECTED_RUNTIME_BUNDLE_SCHEMA_VERSION {
        return Err(format!(
            "Unsupported CrewAI release bundle schema {} (expected {})",
            manifest.schema_version, EXPECTED_RUNTIME_BUNDLE_SCHEMA_VERSION
        ));
    }
    if manifest.python.version != EXPECTED_BUNDLED_PYTHON_VERSION {
        return Err(format!(
            "CrewAI release bundle requires CPython {}, but this app expects {}",
            manifest.python.version, EXPECTED_BUNDLED_PYTHON_VERSION
        ));
    }
    if !manifest.smoke.verified
        || !manifest.smoke.offline
        || !manifest.smoke.tests_passed
        || !manifest.smoke.runtime_compatible
        || !manifest.smoke.tool_dependencies_installed
    {
        return Err(
            "CrewAI release bundle has no successful offline compatibility and test-suite result"
                .to_string(),
        );
    }
    if manifest.smoke.runtime_schema_version != EXPECTED_RUNTIME_SCHEMA_VERSION {
        return Err(format!(
            "CrewAI runtime schema mismatch: bundle has {}, expected {}",
            manifest.smoke.runtime_schema_version, EXPECTED_RUNTIME_SCHEMA_VERSION
        ));
    }
    if manifest.smoke.python_version != manifest.python.version {
        return Err(format!(
            "CrewAI release bundle Python smoke version {} does not match archive version {}",
            manifest.smoke.python_version, manifest.python.version
        ));
    }

    let runtime_dir = resource_dir.join(EMBEDDED_RUNTIME_SCRIPT_DIR);
    let requirements_path = runtime_dir.join("requirements.txt");
    let lock_path = runtime_dir.join("requirements.lock");
    let main_script_path = runtime_dir.join("main.py");
    let requirements_hash = sha256_file(&requirements_path).map_err(|error| {
        format!(
            "CrewAI release bundle requirements are missing or unreadable: {}. Network fallback is disabled.",
            error
        )
    })?;
    if !valid_sha256(&manifest.wheelhouse.requirements_sha256)
        || requirements_hash != manifest.wheelhouse.requirements_sha256.to_ascii_lowercase()
    {
        return Err(format!(
            "CrewAI release bundle requirements SHA-256 mismatch: expected {}, found {}",
            manifest.wheelhouse.requirements_sha256, requirements_hash
        ));
    }
    let lock_hash = sha256_file(&lock_path).map_err(|error| {
        format!(
            "CrewAI release bundle lockfile is missing or unreadable: {}. Network fallback is disabled.",
            error
        )
    })?;
    if !valid_sha256(&manifest.wheelhouse.lock_sha256)
        || lock_hash != manifest.wheelhouse.lock_sha256.to_ascii_lowercase()
    {
        return Err(format!(
            "CrewAI release bundle lockfile SHA-256 mismatch: expected {}, found {}",
            manifest.wheelhouse.lock_sha256, lock_hash
        ));
    }

    let requirements_crewai_version = parse_exact_crewai_requirement(&requirements_path)?;
    let script_crewai_version = parse_runtime_script_crewai_version(&main_script_path)?;
    if manifest.smoke.crewai_version != requirements_crewai_version
        || script_crewai_version != requirements_crewai_version
    {
        return Err(format!(
            "CrewAI version mismatch across bundle manifest ({}) / requirements ({}) / runtime script ({})",
            manifest.smoke.crewai_version, requirements_crewai_version, script_crewai_version
        ));
    }

    if manifest.packages.is_empty() {
        return Err("CrewAI release bundle package manifest is empty".to_string());
    }
    let mut package_filenames = HashMap::new();
    let mut crewai_packages = Vec::new();
    for package in &manifest.packages {
        let filename = Path::new(&package.filename);
        if package.filename.trim().is_empty()
            || filename.file_name().and_then(|value| value.to_str())
                != Some(package.filename.as_str())
            || filename.components().count() != 1
        {
            return Err(format!(
                "CrewAI release bundle contains an unsafe package filename: {}",
                package.filename
            ));
        }
        if !valid_sha256(&package.sha256) {
            return Err(format!(
                "CrewAI release bundle package {} has an invalid SHA-256",
                package.filename
            ));
        }
        if package_filenames
            .insert(package.filename.to_ascii_lowercase(), ())
            .is_some()
        {
            return Err(format!(
                "CrewAI release bundle lists package file {} more than once",
                package.filename
            ));
        }
        if normalized_distribution_name(&package.name) == "crewai" {
            crewai_packages.push(package);
        }
    }
    if crewai_packages.len() != 1 || crewai_packages[0].version != requirements_crewai_version {
        return Err(format!(
            "CrewAI release bundle must contain exactly CrewAI {}, found {} matching package entries",
            requirements_crewai_version,
            crewai_packages.len()
        ));
    }

    let python_archive_sha256 =
        validate_bundle_archive(python_archive, &manifest.python.archive, "Python archive")?;
    let wheels_archive_sha256 = validate_bundle_archive(
        wheels_archive,
        &manifest.wheelhouse.archive,
        "wheel archive",
    )?;

    Ok(Some(ValidatedCrewRuntimeBundle {
        python_version: manifest.python.version,
        crewai_version: requirements_crewai_version,
        python_archive_sha256,
        wheels_archive_sha256,
        packages: manifest.packages,
    }))
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

fn ensure_bundled_runtime_assets<R: Runtime>(
    app: &AppHandle<R>,
    verify_cached_wheels: bool,
) -> Result<Option<ValidatedCrewRuntimeBundle>, String> {
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|error| format!("App resource folder could not be resolved: {}", error))?;
    let Some(bundle) = validate_bundled_runtime(&resource_dir)? else {
        return Ok(None);
    };

    let runtime_root = resolve_runtime_root(app)?;
    fs::create_dir_all(&runtime_root)
        .map_err(|error| format!("Crew runtime root could not be created: {}", error))?;

    let python_archive = resource_dir.join(EMBEDDED_WINDOWS_PYTHON_ARCHIVE_RELATIVE_PATH);
    let embedded_python_root = runtime_root.join("python").join("windows");
    let python_extracted = extract_zip_if_needed(
        &python_archive,
        &embedded_python_root,
        &bundle.python_archive_sha256,
    )?;
    let embedded_python = embedded_python_root.join("python.exe");
    let extracted_python_version = read_python_version(embedded_python.to_string_lossy().as_ref())
        .ok_or_else(|| {
            format!(
                "Bundled Python could not be started after extraction: {}",
                embedded_python.display()
            )
        })?;
    if extracted_python_version != bundle.python_version {
        return Err(format!(
            "Bundled Python version mismatch after extraction: expected {}, found {}",
            bundle.python_version, extracted_python_version
        ));
    }
    if python_extracted {
        mark_zip_extraction_complete(&embedded_python_root, &bundle.python_archive_sha256)?;
    }

    let wheels_archive = resource_dir.join(EMBEDDED_RUNTIME_WHEELS_ARCHIVE_RELATIVE_PATH);
    let wheels_destination = runtime_root
        .join("python")
        .join("crew_runtime")
        .join("wheels");
    let wheels_extracted = extract_zip_if_needed(
        &wheels_archive,
        &wheels_destination,
        &bundle.wheels_archive_sha256,
    )?;
    validate_extracted_wheelhouse(
        &wheels_destination,
        &bundle.packages,
        wheels_extracted || verify_cached_wheels,
    )?;
    if wheels_extracted {
        mark_zip_extraction_complete(&wheels_destination, &bundle.wheels_archive_sha256)?;
    }

    Ok(Some(bundle))
}

fn validate_extracted_wheelhouse(
    wheelhouse: &Path,
    packages: &[CrewRuntimeBundlePackage],
    verify_hashes: bool,
) -> Result<(), String> {
    let expected_packages = packages
        .iter()
        .map(|package| (package.filename.to_ascii_lowercase(), package))
        .collect::<HashMap<_, _>>();
    let actual_wheels = fs::read_dir(wheelhouse)
        .map_err(|error| {
            format!(
                "Bundled CrewAI wheelhouse could not be read ({}): {}",
                wheelhouse.display(),
                error
            )
        })?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let is_wheel = path.is_file()
                && path
                    .extension()
                    .and_then(|value| value.to_str())
                    .map(|value| value.eq_ignore_ascii_case("whl"))
                    .unwrap_or(false);
            is_wheel.then(|| {
                (
                    entry.file_name().to_string_lossy().to_ascii_lowercase(),
                    path,
                )
            })
        })
        .collect::<HashMap<_, _>>();

    if actual_wheels.len() != expected_packages.len()
        || expected_packages
            .keys()
            .any(|filename| !actual_wheels.contains_key(filename))
    {
        return Err(format!(
            "Bundled CrewAI wheelhouse does not match its package manifest (expected {} wheels, found {})",
            expected_packages.len(),
            actual_wheels.len()
        ));
    }

    if verify_hashes {
        for (filename, package) in expected_packages {
            let path = actual_wheels
                .get(&filename)
                .expect("wheel set equality was checked above");
            let actual = sha256_file(path)?;
            if actual != package.sha256.to_ascii_lowercase() {
                return Err(format!(
                    "Bundled CrewAI wheel {} SHA-256 mismatch: expected {}, found {}",
                    package.filename, package.sha256, actual
                ));
            }
        }
    }

    Ok(())
}

fn extract_zip_if_needed(
    zip_path: &Path,
    destination: &Path,
    archive_sha256: &str,
) -> Result<bool, String> {
    let marker = destination.join(".localai_cowork_extract_complete");
    let expected_marker = format!("sha256:{}", archive_sha256);

    if marker.exists() {
        if let Ok(current_marker) = fs::read_to_string(&marker) {
            if current_marker.trim() == expected_marker {
                return Ok(false);
            }
        }
    }
    if destination.exists() {
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
            return Err(format!(
                "archive contains an unsafe entry ({} at index {})",
                zip_path.display(),
                index
            ));
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

    Ok(true)
}

fn mark_zip_extraction_complete(destination: &Path, archive_sha256: &str) -> Result<(), String> {
    let marker = destination.join(".localai_cowork_extract_complete");
    fs::write(&marker, format!("sha256:{}", archive_sha256)).map_err(|error| {
        format!(
            "archive marker could not be written ({}): {}",
            marker.display(),
            error
        )
    })
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
        .env("LITELLM_LOCAL_MODEL_COST_MAP", "True")
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
    if normalized.is_empty() || !normalized.contains("\"localAiCoworkEvent\"") {
        return None;
    }

    let event = serde_json::from_str::<CrewRuntimeProtocolEvent>(normalized).ok()?;
    if event.localai_cowork_event != "crew_log" {
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
        if candidate.is_empty() || candidate.contains("\"localAiCoworkEvent\"") {
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

fn enforce_bundle_status(
    status: &mut CrewRuntimeStatusResponse,
    bundle: Option<&ValidatedCrewRuntimeBundle>,
) {
    let Some(bundle) = bundle else {
        return;
    };

    status.expected_crewai_version = Some(bundle.crewai_version.clone());
    let python_matches = status
        .python_version
        .as_deref()
        .map(|version| version == bundle.python_version)
        .unwrap_or(false);
    let crewai_matches = status
        .crewai_version
        .as_deref()
        .map(|version| version == bundle.crewai_version)
        .unwrap_or(false);
    let schema_matches = status.runtime_schema_version == Some(EXPECTED_RUNTIME_SCHEMA_VERSION);

    if status.ready && (!python_matches || !crewai_matches || !schema_matches) {
        status.ready = false;
        status.bootstrap_required = true;
        status.runtime_compatible = false;
        status.message = format!(
            "Installed Crew runtime does not match the verified release bundle (Python {}, CrewAI {}, schema {}). Reinitialization is required.",
            bundle.python_version, bundle.crewai_version, EXPECTED_RUNTIME_SCHEMA_VERSION
        );
    } else if status.crewai_installed && !crewai_matches {
        status.ready = false;
        status.bootstrap_required = true;
        status.runtime_compatible = false;
        status.message = format!(
            "Installed CrewAI {} does not match bundled CrewAI {}. Reinitialization is required.",
            status.crewai_version.as_deref().unwrap_or("unknown"),
            bundle.crewai_version
        );
    }
}

fn crew_runtime_status_internal<R: Runtime>(
    app: &AppHandle<R>,
    bridge: &CrewPythonBridge,
) -> Result<CrewRuntimeStatusResponse, String> {
    let bundle = ensure_bundled_runtime_assets(app, false)?;
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
        .map(|version| {
            python_version_matches(
                version,
                bundle.as_ref().map(|value| value.python_version.as_str()),
            )
        })
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
        if bundle.is_some() {
            format!(
                "Detected Python interpreter ({}) does not match the verified CrewAI runtime. It will be reinitialized from bundled offline assets.",
                detected_python_version
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string())
            )
        } else {
            format!(
                "Detected Python interpreter ({}) is not compatible with CrewAI. The app-internal runtime will be prepared with Python 3.12 during initialization.",
                detected_python_version
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string())
            )
        }
    } else if preferred_python.is_none() {
        if bundle.is_some() {
            "Crew runtime must be initialized from the bundled offline Python and CrewAI assets."
                .to_string()
        } else {
            "Crew runtime must be initialized. Python 3.12 and CrewAI will be downloaded in isolation into the app data folder.".to_string()
        }
    } else {
        "Crew runtime exists but is not prepared yet".to_string()
    };

    let mut status = build_status_from_json(
        app,
        bridge,
        &runtime_root,
        detected_python_path,
        detected_python_version,
        status_json,
        message,
    );
    enforce_bundle_status(&mut status, bundle.as_ref());
    Ok(status)
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
    let bundle = ensure_bundled_runtime_assets(&app, true)?;
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

    let base_python = ensure_compatible_base_python(
        &app,
        bundle.as_ref().map(|value| value.python_version.as_str()),
    )?;
    let use_local_wheels = local_wheels_available(&wheels_path);
    if bundle.is_some() && !use_local_wheels {
        return Err(
            "The verified CrewAI release bundle has no usable local wheelhouse. Network fallback is disabled."
                .to_string(),
        );
    }

    let force_reinstall = request
        .as_ref()
        .map(|value| value.force_reinstall)
        .unwrap_or(false);
    let venv_python_supported = if venv_python.exists() {
        read_python_version(venv_python.to_string_lossy().as_ref())
            .as_deref()
            .map(|version| {
                python_version_matches(
                    version,
                    bundle.as_ref().map(|value| value.python_version.as_str()),
                )
            })
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
        install_requirements_command.arg("--no-index");
        if !base_python.ends_with("uv.exe") {
            install_requirements_command.arg("--no-compile");
        }
        install_requirements_command.args(["--find-links", wheels_path_arg.as_str()]);
        install_requirements_command
            .env("PIP_NO_INDEX", "1")
            .env("PIP_DISABLE_PIP_VERSION_CHECK", "1");
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    struct TestBundleDirectory {
        path: PathBuf,
    }

    impl TestBundleDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "localai-cowork-rust-bundle-test-{}",
                Uuid::new_v4()
            ));
            fs::create_dir_all(&path).expect("test resource root should be created");
            Self { path }
        }
    }

    impl Drop for TestBundleDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn write_valid_test_bundle() -> TestBundleDirectory {
        let bundle = TestBundleDirectory::new();
        let runtime_dir = bundle.path.join(EMBEDDED_RUNTIME_SCRIPT_DIR);
        let python_archive = bundle
            .path
            .join(EMBEDDED_WINDOWS_PYTHON_ARCHIVE_RELATIVE_PATH);
        let wheels_archive = bundle
            .path
            .join(EMBEDDED_RUNTIME_WHEELS_ARCHIVE_RELATIVE_PATH);
        fs::create_dir_all(&runtime_dir).expect("runtime directory should be created");
        fs::create_dir_all(
            python_archive
                .parent()
                .expect("python archive should have a parent"),
        )
        .expect("python archive directory should be created");

        fs::write(
            runtime_dir.join("requirements.txt"),
            b"crewai[litellm]==1.15.8\npydantic==2.12.5\n",
        )
        .expect("requirements should be written");
        fs::write(
            runtime_dir.join("requirements.lock"),
            b"crewai==1.15.8 --hash=sha256:1111111111111111111111111111111111111111111111111111111111111111\n",
        )
        .expect("lockfile should be written");
        fs::write(
            runtime_dir.join("main.py"),
            b"EXPECTED_CREWAI_VERSION = \"1.15.8\"\n",
        )
        .expect("runtime script should be written");
        fs::write(&python_archive, b"test-python-archive")
            .expect("python archive should be written");
        fs::write(&wheels_archive, b"test-wheel-archive").expect("wheel archive should be written");

        let requirements_hash =
            sha256_file(&runtime_dir.join("requirements.txt")).expect("requirements should hash");
        let lock_hash =
            sha256_file(&runtime_dir.join("requirements.lock")).expect("lockfile should hash");
        let python_hash =
            sha256_file(&python_archive).expect("python archive should hash successfully");
        let wheels_hash =
            sha256_file(&wheels_archive).expect("wheel archive should hash successfully");
        let manifest = json!({
            "schemaVersion": 1,
            "python": {
                "version": "3.12.10",
                "archive": {
                    "bytes": fs::metadata(&python_archive).expect("python metadata").len(),
                    "sha256": python_hash
                }
            },
            "wheelhouse": {
                "requirementsSha256": requirements_hash,
                "lockSha256": lock_hash,
                "archive": {
                    "bytes": fs::metadata(&wheels_archive).expect("wheel metadata").len(),
                    "sha256": wheels_hash
                }
            },
            "packages": [{
                "name": "crewai",
                "version": "1.15.8",
                "filename": "crewai-1.15.8-py3-none-any.whl",
                "sha256": "1111111111111111111111111111111111111111111111111111111111111111"
            }],
            "smoke": {
                "verified": true,
                "offline": true,
                "testsPassed": true,
                "pythonVersion": "3.12.10",
                "crewaiVersion": "1.15.8",
                "runtimeCompatible": true,
                "toolDependenciesInstalled": true,
                "runtimeSchemaVersion": 2
            }
        });
        fs::write(
            runtime_dir.join("runtime-bundle-manifest.json"),
            serde_json::to_vec_pretty(&manifest).expect("manifest should serialize"),
        )
        .expect("manifest should be written");
        bundle
    }

    #[test]
    fn exact_bundle_python_prefers_embedded_over_incompatible_custom_python() {
        let selected = select_exact_python_command(
            "3.12.10",
            Some(("embedded-python".to_string(), "3.12.10".to_string())),
            Some(("custom-python".to_string(), "3.14.0".to_string())),
        );

        assert_eq!(selected.as_deref(), Some("embedded-python"));
    }

    #[test]
    fn validates_consistent_offline_release_bundle() {
        let bundle = write_valid_test_bundle();
        let validated = validate_bundled_runtime(&bundle.path)
            .expect("valid bundle should pass")
            .expect("release bundle should be detected");

        assert_eq!(validated.python_version, "3.12.10");
        assert_eq!(validated.crewai_version, "1.15.8");
        assert_eq!(validated.packages.len(), 1);
    }

    #[test]
    fn validates_generated_release_bundle_assets() {
        let tauri_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let validated = validate_bundled_runtime_with_archives(
            tauri_root,
            &tauri_root
                .join("resources")
                .join("python")
                .join("windows.zip"),
            &tauri_root
                .join("python")
                .join("crew_runtime")
                .join("wheels.zip"),
        )
        .expect("generated release bundle should pass Rust validation")
        .expect("generated release bundle should be detected");

        assert_eq!(validated.python_version, EXPECTED_BUNDLED_PYTHON_VERSION);
        assert_eq!(validated.crewai_version, "1.15.8");
        assert!(validated.packages.len() > 100);
    }

    #[test]
    fn rejects_bundle_when_requirements_change_after_manifest_generation() {
        let bundle = write_valid_test_bundle();
        fs::write(
            bundle
                .path
                .join(EMBEDDED_RUNTIME_SCRIPT_DIR)
                .join("requirements.txt"),
            b"crewai[litellm]==1.15.3\n",
        )
        .expect("requirements should be changed");

        let error = validate_bundled_runtime(&bundle.path)
            .expect_err("tampered requirements must be rejected");
        assert!(error.contains("requirements SHA-256 mismatch"), "{error}");
    }

    #[test]
    fn rejects_bundle_when_lockfile_changes_after_manifest_generation() {
        let bundle = write_valid_test_bundle();
        fs::write(
            bundle
                .path
                .join(EMBEDDED_RUNTIME_SCRIPT_DIR)
                .join("requirements.lock"),
            b"crewai==1.15.3 --hash=sha256:2222222222222222222222222222222222222222222222222222222222222222\n",
        )
        .expect("lockfile should be changed");

        let error =
            validate_bundled_runtime(&bundle.path).expect_err("tampered lockfile must be rejected");
        assert!(error.contains("lockfile SHA-256 mismatch"), "{error}");
    }

    #[test]
    fn rejects_bundle_without_lockfile() {
        let bundle = write_valid_test_bundle();
        fs::remove_file(
            bundle
                .path
                .join(EMBEDDED_RUNTIME_SCRIPT_DIR)
                .join("requirements.lock"),
        )
        .expect("lockfile should be removed");

        let error =
            validate_bundled_runtime(&bundle.path).expect_err("missing lockfile must be rejected");
        assert!(
            error.contains("lockfile is missing or unreadable"),
            "{error}"
        );
    }

    #[test]
    fn rejects_bundle_when_an_archive_hash_no_longer_matches() {
        let bundle = write_valid_test_bundle();
        fs::write(
            bundle
                .path
                .join(EMBEDDED_RUNTIME_WHEELS_ARCHIVE_RELATIVE_PATH),
            b"evil-wheel-archive",
        )
        .expect("wheel archive should be changed without changing its size");

        let error =
            validate_bundled_runtime(&bundle.path).expect_err("tampered archive must be rejected");
        assert!(error.contains("wheel archive SHA-256 mismatch"), "{error}");
    }

    #[test]
    fn rejects_manifest_crewai_version_that_differs_from_exact_pin() {
        let bundle = write_valid_test_bundle();
        let manifest_path = bundle.path.join(EMBEDDED_RUNTIME_MANIFEST_RELATIVE_PATH);
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(&manifest_path).expect("manifest should be readable"))
                .expect("manifest should parse");
        manifest["smoke"]["crewaiVersion"] = json!("1.15.3");
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).expect("manifest should serialize"),
        )
        .expect("manifest should be changed");

        let error =
            validate_bundled_runtime(&bundle.path).expect_err("version mismatch must be rejected");
        assert!(error.contains("CrewAI version mismatch"), "{error}");
    }

    #[test]
    fn rejects_bundle_without_passing_runtime_test_suite() {
        let bundle = write_valid_test_bundle();
        let manifest_path = bundle.path.join(EMBEDDED_RUNTIME_MANIFEST_RELATIVE_PATH);
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(&manifest_path).expect("manifest should be readable"))
                .expect("manifest should parse");
        manifest["smoke"]["testsPassed"] = json!(false);
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).expect("manifest should serialize"),
        )
        .expect("manifest should be changed");

        let error = validate_bundled_runtime(&bundle.path)
            .expect_err("failed runtime test suite must be rejected");
        assert!(error.contains("test-suite result"), "{error}");
    }

    #[test]
    fn rejects_release_assets_without_manifest_instead_of_allowing_network_fallback() {
        let bundle = write_valid_test_bundle();
        fs::remove_file(bundle.path.join(EMBEDDED_RUNTIME_MANIFEST_RELATIVE_PATH))
            .expect("manifest should be removed");

        let error = validate_bundled_runtime(&bundle.path)
            .expect_err("incomplete release bundle must be rejected");
        assert!(error.contains("Network fallback is disabled"), "{error}");
    }

    #[test]
    fn accepts_absent_bundle_only_when_no_release_assets_are_present() {
        let bundle = TestBundleDirectory::new();
        assert!(validate_bundled_runtime(&bundle.path)
            .expect("empty dev resource directory should be accepted")
            .is_none());
    }
}
