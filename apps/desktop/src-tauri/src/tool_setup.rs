//! Explicit native setup. All downloaded runtimes stay in this V2 data directory.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct SetupStatus {
    pub running: bool,
    pub stage: String,
    pub message: String,
    pub ready: bool,
    pub programs: BTreeMap<String, String>,
    pub error: Option<String>,
    /// Profile of the running or last setup in this session.
    #[serde(default)]
    pub profile: Option<SetupProfile>,
    /// The running or last setup was the automatic first-start document setup.
    #[serde(default)]
    pub automatic: bool,
    /// Receipt `/3` records a ready documents profile for the embedded lock and the warm Tectonic cache.
    #[serde(default)]
    pub documents_ready: bool,
    #[serde(default)]
    pub browser_ready: bool,
    #[serde(default)]
    pub auto: Option<AutoSetup>,
}
#[derive(Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum SetupProfile {
    /// Document wheels and offline LaTeX only; never downloads browsers.
    Documents,
    #[default]
    Full,
}
impl SetupProfile {
    fn argument(self) -> &'static str {
        match self {
            Self::Documents => "documents",
            Self::Full => "full",
        }
    }
}
/// Backoff state of the automatic document setup in `<data>/tool-setup-auto.json`.
#[derive(Clone, Serialize, Deserialize, Default)]
pub struct AutoSetup {
    pub attempts: u32,
    pub last_attempt_at: Option<u64>,
    pub next_attempt_at: Option<u64>,
    pub last_error: Option<String>,
    /// Set when the user cancels an automatic run; cleared by a successful setup.
    pub paused_by_user: bool,
}
const DOCUMENTS_LOCK: &[u8] =
    include_bytes!("../../../../tools/toolbox/requirements-documents.lock");
const BROWSER_LOCK: &[u8] = include_bytes!("../../../../tools/toolbox/requirements-browser.lock");
const OCR_LOCK: &[u8] = include_bytes!("../../../../tools/toolbox/requirements-ocr.lock");
const OCR_LANGUAGES: &[u8] = include_bytes!("../../../../tools/toolbox/ocr-languages.lock.json");
const AUTO_FORMAT: &str = "localai-cowork-tool-setup-auto/1";
#[derive(Default)]
pub struct ToolSetupGate {
    status: Arc<Mutex<SetupStatus>>,
    cancel: Arc<AtomicBool>,
    selecting: Arc<AtomicBool>,
    worker: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    closing: AtomicBool,
}
impl ToolSetupGate {
    /// Normal desktop exit waits for the exact setup worker and its supervised
    /// children, rather than leaving a pip/browser/archive process behind.
    pub fn shutdown(&self) {
        let worker = {
            let mut worker = self.worker.lock().unwrap_or_else(|e| e.into_inner());
            self.closing.store(true, Ordering::Release);
            self.cancel.store(true, Ordering::Release);
            worker.take()
        };
        if let Some(worker) = worker {
            let _ = tauri::async_runtime::block_on(worker);
        }
    }
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupOptions {
    pub git: bool,
    pub github_cli: bool,
    pub libreoffice: bool,
    #[serde(default)]
    pub profile: SetupProfile,
}

fn namespace(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("Absolute private tool directory required".into());
    }
    for parent in path.ancestors() {
        let Ok(metadata) = fs::symlink_metadata(parent) else {
            continue;
        };
        if metadata.is_symlink() {
            return Err("Tool directory contains an alias".into());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err("Tool directory contains a reparse point".into());
            }
        }
    }
    Ok(())
}
fn directory(path: &Path) -> Result<(), String> {
    namespace(path)?;
    fs::create_dir_all(path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    namespace(path)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}
fn read_json(path: &Path) -> Option<serde_json::Value> {
    namespace(path).ok()?;
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > 256 * 1024 {
        return None;
    }
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}
fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}
/// (documents, browser) readiness from the receipt; `/2` receipts predate the documents profile.
fn profile_readiness(data: &Path) -> (bool, bool) {
    let Some(receipt) = read_json(&data.join("tool-setup-receipt.json")) else {
        return (false, false);
    };
    match receipt["format"].as_str() {
        Some("localai-cowork-tool-setup/2") => (false, receipt["ready"] == true),
        Some("localai-cowork-tool-setup/3") => {
            let documents = &receipt["profiles"]["documents"];
            let ocr = &receipt["profiles"]["ocr"];
            let warm = read_json(&data.join("tool-runtime").join("tectonic-warm.json"));
            let ready = documents["ready"] == true
                && documents["requirements_sha256"].as_str()
                    == Some(sha256_hex(DOCUMENTS_LOCK).as_str())
                && ocr["ready"] == true
                && ocr["requirements_sha256"].as_str() == Some(sha256_hex(OCR_LOCK).as_str())
                && ocr["language_manifest_sha256"].as_str()
                    == Some(sha256_hex(OCR_LANGUAGES).as_str())
                && warm.is_some_and(|marker| {
                    marker["format"] == "cowork-tectonic-warm/1"
                        && marker["tectonic_sha256"].is_string()
                        && marker["tectonic_sha256"] == documents["tectonic_sha256"]
                });
            let browser = &receipt["profiles"]["browser"];
            let browser_ready = browser["ready"] == true
                && browser["requirements_sha256"]
                    .as_str()
                    .is_none_or(|sha| sha == sha256_hex(BROWSER_LOCK));
            (ready, browser_ready)
        }
        _ => (false, false),
    }
}
/// The receipt check plus the configured Python and the exact Tectonic binary the cache is bound to.
fn documents_installed(data: &Path) -> bool {
    if !profile_readiness(data).0 {
        return false;
    }
    let programs = read_json(&data.join("toolbox.json"))
        .map(|c| c["programs"].clone())
        .unwrap_or_default();
    let python = programs["python"].as_str().map(PathBuf::from);
    let tectonic = programs["tectonic"].as_str().map(PathBuf::from);
    let marker =
        read_json(&data.join("tool-runtime").join("tectonic-warm.json")).unwrap_or_default();
    python.is_some_and(|p| p.is_file())
        && tectonic
            .and_then(|t| fs::read(t).ok())
            .is_some_and(|bytes| {
                marker["tectonic_sha256"].as_str() == Some(sha256_hex(&bytes).as_str())
            })
}
fn read_auto(data: &Path) -> AutoSetup {
    read_json(&data.join("tool-setup-auto.json"))
        .filter(|value| value["format"] == AUTO_FORMAT)
        .and_then(|value| serde_json::from_value(value["state"].clone()).ok())
        .unwrap_or_default()
}
fn write_auto(data: &Path, state: &AutoSetup) -> Result<(), String> {
    let path = data.join("tool-setup-auto.json");
    namespace(&path)?;
    let temporary = data.join(format!("tool-setup-auto-{}.new", uuid::Uuid::new_v4()));
    write_new(
        &temporary,
        &serde_json::to_vec_pretty(&serde_json::json!({"format": AUTO_FORMAT, "state": state}))
            .map_err(|e| e.to_string())?,
    )?;
    fs::rename(temporary, path).map_err(|e| e.to_string())
}
/// Delay after the given number of failed automatic attempts: 10 min, 1 h, 6 h, then daily.
fn auto_backoff(attempts: u32) -> u64 {
    const MINUTE: u64 = 60_000;
    match attempts {
        0 | 1 => 10 * MINUTE,
        2 => 60 * MINUTE,
        3 => 6 * 60 * MINUTE,
        _ => 24 * 60 * MINUTE,
    }
}
/// Python and pip mishandle Windows `\\?\` verbatim paths: wheel data such as
/// `site-packages\../../Scripts/x.py` is not normalized below that prefix. Local drive paths
/// are returned without it; UNC and other paths stay unchanged.
fn ordinary_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    if let Some(rest) = path.to_str().and_then(|value| value.strip_prefix(r"\\?\"))
        && rest.as_bytes().get(1) == Some(&b':')
    {
        return PathBuf::from(rest);
    }
    path
}
/// A private CPython from an earlier native setup, reused so the document profile adds wheels in place.
fn private_python(data: &Path) -> Option<PathBuf> {
    let runtime = ordinary_path(fs::canonicalize(data.join("tool-runtime")).ok()?);
    let config = read_json(&data.join("toolbox.json"))?;
    let python = ordinary_path(fs::canonicalize(config["programs"]["python"].as_str()?).ok()?);
    let root = if cfg!(windows) {
        python.parent()?
    } else {
        python.parent()?.parent()?
    };
    let stage = root.parent()?;
    (python.is_file()
        && root.file_name()? == "python"
        && stage.parent()? == runtime
        && stage.file_name()?.to_str()?.starts_with("native-"))
    .then_some(python)
}
fn update(status: &Arc<Mutex<SetupStatus>>, stage: &str, message: &str) {
    if let Ok(mut status) = status.lock() {
        status.stage = stage.into();
        status.message = message.into();
    }
}
fn environment(data: &Path) -> Result<BTreeMap<String, String>, String> {
    let mut environment: BTreeMap<String, String> =
        ["PATH", "SystemRoot", "WINDIR", "LANG", "LC_ALL"]
            .into_iter()
            .filter_map(|name| std::env::var(name).ok().map(|value| (name.into(), value)))
            .collect();
    let root = data.join("tool-runtime");
    for (name, folder) in [
        ("TEMP", "tmp"),
        ("TMP", "tmp"),
        ("TMPDIR", "tmp"),
        ("HOME", "home"),
        ("USERPROFILE", "home"),
    ] {
        directory(&root.join(folder))?;
        environment.insert(
            name.into(),
            root.join(folder).to_string_lossy().into_owned(),
        );
    }
    environment.insert("PYTHONNOUSERSITE".into(), "1".into());
    Ok(environment)
}
fn supervise(
    program: PathBuf,
    arguments: Vec<String>,
    data: &Path,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let launch = cowork_executor::TrustedLaunch {
        program,
        arguments,
        directory: data.to_path_buf(),
        environment: environment(data)?,
        timeout: Duration::from_secs(1800),
        capture_bytes_per_stream: 1024 * 1024,
    };
    let result = cowork_executor::supervise(&launch, cancel).map_err(|e| e.to_string())?;
    if result.exit_code != Some(0)
        || result.termination != cowork_executor::Termination::Exited
        || !result.output_complete
    {
        let detail = String::from_utf8_lossy(&result.stderr.bytes);
        return Err(format!(
            "Tool setup {:?} (exit {:?}): {}",
            result.termination,
            result.exit_code,
            detail
                .chars()
                .rev()
                .take(4096)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>()
        ));
    }
    Ok(())
}
async fn cancelable<T>(
    future: impl std::future::Future<Output = Result<T, reqwest::Error>>,
    cancel: &AtomicBool,
) -> Result<T, String> {
    let mut future = Box::pin(future);
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err("Tool setup canceled".into());
        }
        if let Ok(value) = tokio::time::timeout(Duration::from_millis(500), future.as_mut()).await {
            return value.map_err(|e| e.to_string());
        }
    }
}
async fn download(
    path: &Path,
    url: &str,
    expected: &str,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| e.to_string())?;
    let mut response = cancelable(client.get(url).send(), cancel)
        .await?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    if response.url().scheme() != "https" {
        return Err("Tool download requires HTTPS".into());
    }
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    let mut digest = Sha256::new();
    let mut total = 0usize;
    while let Some(chunk) = cancelable(response.chunk(), cancel).await? {
        total = total
            .checked_add(chunk.len())
            .ok_or("Tool download exceeds its bound")?;
        if total > 100 * 1024 * 1024 {
            return Err("Tool download exceeds its bound".into());
        }
        digest.update(&chunk);
        file.write_all(&chunk).map_err(|e| e.to_string())?;
    }
    file.sync_all().map_err(|e| e.to_string())?;
    if format!("{:x}", digest.finalize()) != expected {
        return Err("Official Python archive SHA-256 does not match".into());
    }
    Ok(())
}
async fn run(
    data: PathBuf,
    status: Arc<Mutex<SetupStatus>>,
    cancel: Arc<AtomicBool>,
    options: SetupOptions,
) -> Result<(), String> {
    let stage = data
        .join("tool-runtime")
        .join(format!("native-{}", uuid::Uuid::new_v4()));
    directory(&stage)?;
    write_new(
        &stage.join("runtime_install.py"),
        include_bytes!("../../../../tools/toolbox/runtime_install.py"),
    )?;
    write_new(
        &stage.join("runtime_inventory.py"),
        include_bytes!("../../../../tools/release/runtime_inventory.py"),
    )?;
    write_new(
        &stage.join("sbom.py"),
        include_bytes!("../../../../tools/release/sbom.py"),
    )?;
    write_new(&stage.join("requirements-documents.lock"), DOCUMENTS_LOCK)?;
    write_new(&stage.join("requirements-ocr.lock"), OCR_LOCK)?;
    write_new(&stage.join("ocr-languages.lock.json"), OCR_LANGUAGES)?;
    write_new(
        &stage.join("ocr_setup.py"),
        include_bytes!("../../../../tools/toolbox/ocr_setup.py"),
    )?;
    write_new(&stage.join("requirements-browser.lock"), BROWSER_LOCK)?;
    write_new(
        &stage.join("linux-git.lock.json"),
        include_bytes!("../../../../tools/toolbox/linux-git.lock.json"),
    )?;
    write_new(
        &stage.join("browser-runtimes.lock.json"),
        include_bytes!("../../../../tools/toolbox/browser-runtimes.lock.json"),
    )?;
    // The automatic document setup adds wheels to an existing private CPython instead of
    // replacing it, so packages from an earlier full setup stay importable.
    let reused = match options.profile {
        SetupProfile::Documents => private_python(&data),
        SetupProfile::Full => None,
    };
    let reused_python = reused.is_some();
    let python = match reused {
        Some(python) => python,
        None => fresh_python(&data, &stage, &status, &cancel).await?,
    };
    let mut args = vec![
        "-I".into(),
        stage
            .join("runtime_install.py")
            .to_string_lossy()
            .into_owned(),
        "--data-dir".into(),
        data.to_string_lossy().into_owned(),
        "--embedded".into(),
        "--profile".into(),
        options.profile.argument().into(),
    ];
    if options.git {
        args.push("--git".into());
    }
    if options.github_cli {
        args.push("--github-cli".into());
    }
    if options.libreoffice {
        args.push("--libreoffice".into());
    }
    update(
        &status,
        "packages",
        match options.profile {
            SetupProfile::Documents => {
                "Installing the private Office/PDF adapters and offline LaTeX"
            }
            SetupProfile::Full => "Installing the private Office/PDF/browser adapters",
        },
    );
    tauri::async_runtime::spawn_blocking(move || supervise(python, args, &data, &cancel))
        .await
        .map_err(|e| e.to_string())??;
    if reused_python {
        // Only the embedded scripts were staged; the reused Python lives in an earlier stage.
        let _ = fs::remove_dir_all(&stage);
    }
    Ok(())
}

/// Downloads and extracts the pinned private CPython into this setup's stage.
async fn fresh_python(
    data: &Path,
    stage: &Path,
    status: &Arc<Mutex<SetupStatus>>,
    cancel: &Arc<AtomicBool>,
) -> Result<PathBuf, String> {
    update(
        status,
        "python_download",
        "Downloading pinned private CPython 3.13.16",
    );
    #[cfg(windows)]
    let (url, sha) = (
        "https://www.python.org/ftp/python/3.13.16/python-3.13.16-embed-amd64.zip",
        "97dae5274cc54867065e8d5a3226e48c35017ed332a0fdb0e27d5b5821961297",
    );
    #[cfg(not(windows))]
    let (url, sha) = (
        "https://github.com/astral-sh/python-build-standalone/releases/download/20261009/cpython-3.13.16%2B20261009-x86_64-unknown-linux-gnu-install_only.tar.gz",
        "1cca1041f693f110a2e0d16417090b8996258c7a121e90eeab66c2f40ea407f2",
    );
    let archive = stage.join(if cfg!(windows) {
        "python.zip"
    } else {
        "python.tar.gz"
    });
    download(&archive, url, sha, cancel).await?;
    if cancel.load(Ordering::Acquire) {
        return Err("Tool setup canceled before extraction".into());
    }
    let python_root = stage.join("python");
    directory(&python_root)?;
    update(status, "python_extract", "Extracting private CPython");
    #[cfg(windows)]
    let (program, args) = {
        let script = stage.join("bootstrap.ps1");
        write_new(
            &script,
            include_bytes!("../../../../tools/toolbox/bootstrap-windows.ps1"),
        )?;
        let system =
            std::env::var("SystemRoot").map_err(|_| "Windows system directory unavailable")?;
        (
            PathBuf::from(system).join("System32/WindowsPowerShell/v1.0/powershell.exe"),
            vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-File".into(),
                script.to_string_lossy().into_owned(),
                "-Archive".into(),
                archive.to_string_lossy().into_owned(),
                "-Destination".into(),
                python_root.to_string_lossy().into_owned(),
            ],
        )
    };
    #[cfg(not(windows))]
    let (program, args) = (
        PathBuf::from("/usr/bin/tar"),
        vec![
            "-xzf".into(),
            archive.to_string_lossy().into_owned(),
            "--strip-components=1".into(),
            "-C".into(),
            python_root.to_string_lossy().into_owned(),
        ],
    );
    let dir = data.to_path_buf();
    let stopper = cancel.clone();
    tauri::async_runtime::spawn_blocking(move || supervise(program, args, &dir, &stopper))
        .await
        .map_err(|e| e.to_string())??;
    fs::remove_file(archive).map_err(|e| e.to_string())?;
    let python = python_root.join(if cfg!(windows) {
        "python.exe"
    } else {
        "bin/python3"
    });
    fs::canonicalize(python)
        .map(ordinary_path)
        .map_err(|e| e.to_string())
}

fn current_status(data: &Path, gate: &ToolSetupGate) -> Result<SetupStatus, String> {
    let mut status = gate
        .status
        .lock()
        .map_err(|_| "Tool setup state unavailable")?
        .clone();
    if let Some(config) = read_json(&data.join("toolbox.json")) {
        status.programs =
            serde_json::from_value(config.get("programs").cloned().unwrap_or_default())
                .unwrap_or_default();
    }
    if status.running {
        if let Some(progress) = read_json(&data.join("tool-setup-progress.json")) {
            status.stage = progress
                .get("stage")
                .and_then(|v| v.as_str())
                .unwrap_or(&status.stage)
                .into();
            status.message = progress
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or(&status.message)
                .into();
        }
    } else if status.error.is_none() {
        status.ready =
            read_json(&data.join("tool-setup-receipt.json")).is_some_and(|v| v["ready"] == true);
    }
    (status.documents_ready, status.browser_ready) = profile_readiness(data);
    status.auto = Some(read_auto(data));
    Ok(status)
}
#[tauri::command]
pub async fn toolbox_setup_status(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, ToolSetupGate>,
) -> Result<SetupStatus, String> {
    if !super::application_window(&window) {
        return Err("Forbidden".into());
    }
    let data = super::private_data_dir(window.app_handle())?;
    current_status(&data, &gate)
}
#[tauri::command]
pub async fn toolbox_setup_start(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, ToolSetupGate>,
    options: SetupOptions,
) -> Result<SetupStatus, String> {
    if !super::application_window(&window) {
        return Err("Forbidden".into());
    }
    start(&window, &gate, options, false)
}
/// Called once by the UI after it connected: installs the document profile (wheels + offline
/// LaTeX, no browsers) when no valid receipt exists, with backoff between automatic attempts.
#[tauri::command]
pub async fn toolbox_setup_ensure_documents(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, ToolSetupGate>,
) -> Result<SetupStatus, String> {
    if !super::application_window(&window) {
        return Err("Forbidden".into());
    }
    let data = super::private_data_dir(window.app_handle())?;
    {
        let status = gate
            .status
            .lock()
            .map_err(|_| "Tool setup state unavailable")?;
        if status.running
            || gate.selecting.load(Ordering::Acquire)
            || gate.closing.load(Ordering::Acquire)
        {
            drop(status);
            return current_status(&data, &gate);
        }
    }
    let probe = data.clone();
    let installed = tauri::async_runtime::spawn_blocking(move || documents_installed(&probe))
        .await
        .map_err(|e| e.to_string())?;
    let mut auto = read_auto(&data);
    let now = now_millis();
    if installed || auto.paused_by_user || auto.next_attempt_at.is_some_and(|next| now < next) {
        return current_status(&data, &gate);
    }
    directory(&data)?;
    auto.attempts = auto.attempts.saturating_add(1);
    auto.last_attempt_at = Some(now);
    auto.next_attempt_at = Some(now + auto_backoff(auto.attempts));
    write_auto(&data, &auto)?;
    let options = SetupOptions {
        git: false,
        github_cli: false,
        libreoffice: false,
        profile: SetupProfile::Documents,
    };
    match start(&window, &gate, options, true) {
        Ok(status) => Ok(status),
        // A manual setup or the picker won the race; the automatic attempt simply waits.
        Err(_) => current_status(&data, &gate),
    }
}
fn start(
    window: &tauri::WebviewWindow,
    gate: &ToolSetupGate,
    options: SetupOptions,
    automatic: bool,
) -> Result<SetupStatus, String> {
    let mut worker_slot = gate.worker.lock().unwrap_or_else(|e| e.into_inner());
    if gate.closing.load(Ordering::Acquire) {
        return Err("The application is closing".into());
    }
    let data = super::private_data_dir(window.app_handle())?;
    directory(&data)?;
    let mut status = gate
        .status
        .lock()
        .map_err(|_| "Tool setup state unavailable")?;
    if status.running || gate.selecting.load(Ordering::Acquire) {
        return Err("Tool setup or its picker is already running".into());
    }
    let progress = data.join("tool-setup-progress.json");
    namespace(&progress)?;
    if progress.exists() {
        fs::remove_file(progress).map_err(|e| e.to_string())?;
    }
    *status = SetupStatus {
        running: true,
        stage: "starting".into(),
        message: if automatic {
            "Starting the automatic document tool setup".into()
        } else {
            "Starting explicit private setup".into()
        },
        profile: Some(options.profile),
        automatic,
        ..SetupStatus::default()
    };
    let response = status.clone();
    drop(status);
    gate.cancel.store(false, Ordering::Release);
    let state = gate.status.clone();
    let cancel = gate.cancel.clone();
    let client_window = window.clone();
    let worker = tauri::async_runtime::spawn(async move {
        let result = run(data.clone(), state.clone(), cancel, options).await;
        let installed = result.is_ok();
        let result = match result {
            Ok(()) => {
                match super::native_client::NativeClient::new(
                    &client_window,
                    "tool_setup_reload_failed",
                    "tool_setup_reload_failed",
                ) {
                    Ok(client) => client
                        .post::<serde_json::Value>("/native/toolbox/reload", &serde_json::json!({}))
                        .await
                        .map(|_| ())
                        .map_err(|e| format!("Tools installed; reload failed: {}", e.code)),
                    Err(e) => Err(format!(
                        "Tools installed; connect to the local instance and reload: {}",
                        e.code
                    )),
                }
            }
            Err(e) => Err(e),
        };
        // Every profile installs the document runtime, so any completed setup ends the backoff.
        if installed {
            let _ = write_auto(&data, &AutoSetup::default());
        } else if automatic {
            let mut auto = read_auto(&data);
            auto.last_error = result
                .as_ref()
                .err()
                .map(|e| e.chars().take(2048).collect());
            let _ = write_auto(&data, &auto);
        }
        if let Ok(mut status) = state.lock() {
            status.running = false;
            status.ready = result.is_ok();
            status.stage = if result.is_ok() { "ready" } else { "failed" }.into();
            status.message = if result.is_ok() {
                "Private tools installed and active".into()
            } else {
                "Tool setup needs attention".into()
            };
            status.error = result.err();
        }
    });
    *worker_slot = Some(worker);
    Ok(response)
}
#[tauri::command]
pub async fn toolbox_setup_select(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, ToolSetupGate>,
    name: String,
) -> Result<bool, String> {
    if !super::application_window(&window)
        || !matches!(
            name.as_str(),
            "python"
                | "chromium"
                | "chrome"
                | "firefox"
                | "browser_host"
                | "git"
                | "gh"
                | "libreoffice"
                | "tectonic"
                | "docker"
                | "node"
                | "powershell"
                | "bash"
        )
    {
        return Err("Forbidden".into());
    }
    {
        let status = gate
            .status
            .lock()
            .map_err(|_| "Tool setup state unavailable")?;
        if status.running || gate.selecting.swap(true, Ordering::AcqRel) {
            return Err("Tool setup or its picker is running".into());
        }
    }
    struct Selection(Arc<AtomicBool>);
    impl Drop for Selection {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    let _selection = Selection(gate.selecting.clone());
    let (sender, receiver) = tokio::sync::oneshot::channel();
    window
        .app_handle()
        .dialog()
        .file()
        .set_parent(&window)
        .set_title(format!("Select {name} executable"))
        .pick_file(move |path| {
            let _ = sender.send(path);
        });
    let Some(selected) = receiver.await.map_err(|_| "Tool picker failed")? else {
        return Ok(false);
    };
    let selected = selected
        .into_path()
        .map_err(|_| "Select a local executable")?;
    if gate.closing.load(Ordering::Acquire) {
        return Err("The application is closing".into());
    }
    let selected = fs::canonicalize(selected).map_err(|e| e.to_string())?;
    if !selected.is_file()
        || selected
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
    {
        return Err("Select an executable program".into());
    }
    let data = super::private_data_dir(window.app_handle())?;
    directory(&data)?;
    let path = data.join("toolbox.json");
    namespace(&path)?;
    let mut config = if path.exists() {
        read_json(&path).ok_or("Existing tool configuration is invalid or oversized")?
    } else {
        serde_json::json!({"programs":{}})
    };
    if !config.is_object() || !config["programs"].is_object() {
        return Err("Existing tool configuration is invalid".into());
    }
    config["programs"][name] = serde_json::Value::String(selected.to_string_lossy().into_owned());
    let temporary = data.join(format!("toolbox-{}.new", uuid::Uuid::new_v4()));
    write_new(
        &temporary,
        &serde_json::to_vec_pretty(&config).map_err(|e| e.to_string())?,
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    fs::rename(temporary, path).map_err(|e| e.to_string())?;
    let client = super::native_client::NativeClient::new(
        &window,
        "tool_setup_reload_failed",
        "tool_setup_reload_failed",
    )
    .map_err(|e| e.code)?;
    client
        .post::<serde_json::Value>("/native/toolbox/reload", &serde_json::json!({}))
        .await
        .map_err(|e| e.code)?;
    Ok(true)
}
#[tauri::command]
pub async fn toolbox_setup_cancel(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, ToolSetupGate>,
) -> Result<(), String> {
    if !super::application_window(&window) {
        return Err("Forbidden".into());
    }
    let automatic = gate
        .status
        .lock()
        .map(|status| status.running && status.automatic)
        .unwrap_or(false);
    gate.cancel.store(true, Ordering::Release);
    if automatic {
        // The user declined the automatic download; only a manual setup resumes it.
        let data = super::private_data_dir(window.app_handle())?;
        let mut auto = read_auto(&data);
        auto.paused_by_user = true;
        write_auto(&data, &auto)?;
    }
    Ok(())
}
#[tauri::command]
pub async fn toolbox_setup_reload(window: tauri::WebviewWindow) -> Result<(), String> {
    let client = super::native_client::NativeClient::new(
        &window,
        "tool_setup_reload_failed",
        "tool_setup_reload_failed",
    )
    .map_err(|e| e.code)?;
    client
        .post::<serde_json::Value>("/native/toolbox/reload", &serde_json::json!({}))
        .await
        .map_err(|e| e.code)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn python_gets_ordinary_drive_paths() {
        assert_eq!(
            ordinary_path(PathBuf::from(r"\\?\C:\Data\python\python.exe")),
            PathBuf::from(r"C:\Data\python\python.exe")
        );
        assert_eq!(
            ordinary_path(PathBuf::from(r"\\?\UNC\server\share\python.exe")),
            PathBuf::from(r"\\?\UNC\server\share\python.exe")
        );
        assert_eq!(
            ordinary_path(PathBuf::from(r"C:\Data\python.exe")),
            PathBuf::from(r"C:\Data\python.exe")
        );
    }

    fn receipt(data: &Path, value: serde_json::Value) {
        fs::write(data.join("tool-setup-receipt.json"), value.to_string()).unwrap();
    }

    #[test]
    fn documents_are_ready_only_for_the_embedded_lock_and_a_matching_warm_cache() {
        let directory = tempfile::tempdir().unwrap();
        let data = directory.path();
        assert_eq!(profile_readiness(data), (false, false));
        receipt(
            data,
            serde_json::json!({"format": "localai-cowork-tool-setup/2", "ready": true}),
        );
        assert_eq!(
            profile_readiness(data),
            (false, true),
            "a /2 receipt predates the document profile"
        );
        let lock = sha256_hex(DOCUMENTS_LOCK);
        receipt(
            data,
            serde_json::json!({"format": "localai-cowork-tool-setup/3", "ready": true,
            "profiles": {"documents": {"ready": true, "requirements_sha256": lock, "tectonic_sha256": "a".repeat(64)}}}),
        );
        assert_eq!(
            profile_readiness(data),
            (false, false),
            "no warm marker yet"
        );
        fs::create_dir_all(data.join("tool-runtime")).unwrap();
        let marker = data.join("tool-runtime").join("tectonic-warm.json");
        fs::write(&marker, serde_json::json!({"format": "cowork-tectonic-warm/1", "tectonic_sha256": "b".repeat(64)}).to_string()).unwrap();
        assert_eq!(
            profile_readiness(data),
            (false, false),
            "cache warmed for another binary"
        );
        fs::write(&marker, serde_json::json!({"format": "cowork-tectonic-warm/1", "tectonic_sha256": "a".repeat(64)}).to_string()).unwrap();
        assert_eq!(
            profile_readiness(data),
            (false, false),
            "older documents without OCR need setup"
        );
        let ready = serde_json::json!({"format":"localai-cowork-tool-setup/3", "ready":true,
            "profiles":{"documents":{"ready":true,"requirements_sha256":lock,"tectonic_sha256":"a".repeat(64)},
                        "ocr":{"ready":true,"requirements_sha256":sha256_hex(OCR_LOCK),"language_manifest_sha256":sha256_hex(OCR_LANGUAGES)}}});
        receipt(data, ready.clone());
        assert_eq!(profile_readiness(data), (true, false));
        for field in ["requirements_sha256", "language_manifest_sha256"] {
            let mut changed = ready.clone();
            changed["profiles"]["ocr"][field] = serde_json::json!("0".repeat(64));
            receipt(data, changed);
            assert_eq!(
                profile_readiness(data),
                (false, false),
                "changed OCR {field}"
            );
        }
        receipt(
            data,
            serde_json::json!({"format": "localai-cowork-tool-setup/3", "ready": true,
            "profiles": {"documents": {"ready": true, "requirements_sha256": "0".repeat(64), "tectonic_sha256": "a".repeat(64)},
                         "browser": {"ready": true, "requirements_sha256": sha256_hex(BROWSER_LOCK)}}}),
        );
        assert_eq!(
            profile_readiness(data),
            (false, true),
            "an older document lock is set up again"
        );
        assert!(!documents_installed(data));
    }

    #[test]
    fn setup_options_default_to_the_full_profile() {
        let options: SetupOptions = serde_json::from_value(
            serde_json::json!({"git": true, "github_cli": false, "libreoffice": false}),
        )
        .unwrap();
        assert_eq!(options.profile, SetupProfile::Full);
        let options: SetupOptions = serde_json::from_value(
            serde_json::json!({"git": false, "github_cli": false, "libreoffice": false, "profile": "documents"}),
        )
        .unwrap();
        assert_eq!(options.profile.argument(), "documents");
        assert!(serde_json::from_value::<SetupOptions>(
            serde_json::json!({"git": false, "github_cli": false, "libreoffice": false, "profile": "browser-only"})
        )
        .is_err());
    }

    #[test]
    fn automatic_setup_backs_off_and_keeps_its_state() {
        assert!(auto_backoff(1) < auto_backoff(2) && auto_backoff(2) < auto_backoff(3));
        assert_eq!(auto_backoff(9), auto_backoff(4));
        let directory = tempfile::tempdir().unwrap();
        let state = AutoSetup {
            attempts: 2,
            last_attempt_at: Some(1),
            next_attempt_at: Some(2),
            last_error: Some("offline".into()),
            paused_by_user: true,
        };
        write_auto(directory.path(), &state).unwrap();
        let read = read_auto(directory.path());
        assert_eq!(
            (read.attempts, read.next_attempt_at, read.paused_by_user),
            (2, Some(2), true)
        );
        write_auto(directory.path(), &AutoSetup::default()).unwrap();
        assert_eq!(read_auto(directory.path()).attempts, 0);
    }
}
