//! Explicit owner tools. Native processes have the owner's OS rights; this is
//! lifecycle supervision and managed working directories, not a security sandbox.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use uuid::Uuid;
pub mod browser_bridge;
mod browser_v2;
#[cfg(test)]
mod browser_v2_tests;
mod persistent;
pub use browser_v2::validate_browser_command;
mod documents;
mod root_owner;
#[cfg(test)]
mod root_owner_tests;
pub use documents::DocumentVersion;
mod knowledge;
pub use knowledge::KnowledgeExtraction;
mod document_engine;
mod document_ocr;
pub use document_engine::{
    DocumentFile, DocumentInputFile, DocumentJob, DocumentOutcome, DocumentPreview,
    MAX_DOCUMENT_INPUT_BYTES,
};
pub use persistent::{StdioOptions, validate_stdio_environment};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "This application data directory is already in use; close the other Cowork instance before starting another one"
    )]
    RootInUse,
    #[error("{0}")]
    Invalid(String),
    #[error("Tool operation was not found")]
    NotFound,
    #[error("Tool operation conflicts with its existing admission")]
    Conflict,
    #[error("Tool IO failed; an admitted action must not be automatically replayed")]
    Io(#[from] std::io::Error),
    #[error("Tool journal encoding failed")]
    Json(#[from] serde_json::Error),
}
pub type Result<T> = std::result::Result<T, Error>;
fn invalid(s: &str) -> Error {
    Error::Invalid(s.into())
}
fn hash(raw: &[u8]) -> String {
    Sha256::digest(raw)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Terminal {
        program: String,
        #[serde(default)]
        arguments: Vec<String>,
        #[serde(default)]
        cwd: String,
    },
    Browser {
        session_id: String,
        operation: String,
        #[serde(default)]
        url: Option<String>,
        #[serde(default)]
        selector: Option<String>,
        #[serde(default)]
        text: Option<String>,
    },
    Office {
        operation: String,
        #[serde(default)]
        input: Option<String>,
        #[serde(default)]
        output: Option<String>,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        title: Option<String>,
    },
    Github {
        arguments: Vec<String>,
    },
    Git {
        operation: String,
        #[serde(default)]
        cwd: String,
        #[serde(default)]
        expected_state: Option<String>,
        #[serde(default)]
        paths: Vec<String>,
        #[serde(default)]
        reference: Option<String>,
        #[serde(default)]
        text: Option<String>,
    },
    PullRequest {
        repository: String,
        number: u32,
        operation: String,
        #[serde(default)]
        expected_head: Option<String>,
        #[serde(default)]
        event: Option<String>,
        #[serde(default)]
        body: Option<String>,
        #[serde(default)]
        merge_method: Option<String>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub idempotency_key: String,
    pub action: Action,
    #[serde(default = "timeout_default")]
    pub timeout_seconds: u64,
    #[serde(default = "output_default")]
    pub output_bytes: usize,
    #[serde(default)]
    pub approval_sha256: Option<String>,
}
fn timeout_default() -> u64 {
    120
}
fn output_default() -> usize {
    256 * 1024
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Preview {
    pub request_sha256: String,
    pub mode: String,
    pub cwd: String,
    pub timeout_seconds: u64,
    pub output_bytes: usize,
    pub approval_required: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub order_id: String,
    pub idempotency_key: String,
    pub request_sha256: String,
    pub state: String,
    pub outcome: Option<cowork_executor::Outcome>,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrowserSession {
    pub id: String,
    pub order_id: String,
    pub url: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Availability {
    pub mode: String,
    pub programs: Vec<String>,
    pub browser_available: bool,
    pub office_available: bool,
    pub github_available: bool,
    pub filesystem_isolation: bool,
    /// Built-in document tools: pinned engine probed with the configured Python.
    pub documents_available: bool,
    pub ocr_available: bool,
    pub ocr_status: String,
    pub latex_available: bool,
    pub office_convert_available: bool,
    pub document_engine_version: Option<String>,
    pub document_status: String,
    pub latex_status: String,
    pub office_convert_status: String,
}
pub struct Config {
    pub root: PathBuf,
    pub programs: BTreeMap<String, PathBuf>,
    pub environment: BTreeMap<String, String>,
    pub github_token: Option<String>,
}
struct Running {
    cancel: Arc<AtomicBool>,
    job: Job,
    local: bool,
}
/// Shared only within the trusted daemon. A container caller retains this until
/// its exact guest has reached terminal cleanup, beyond the Docker CLI's exit.
pub struct WorkspaceLease {
    order: String,
    root: PathBuf,
    _file: fs::File,
}
pub struct Toolbox {
    config: Config,
    programs: Mutex<BTreeMap<String, PathBuf>>,
    adapters: PathBuf,
    jobs: Mutex<BTreeMap<String, Running>>,
    sessions: Mutex<BTreeMap<String, BrowserSession>>,
    managed: Mutex<BTreeMap<String, Arc<persistent::ManagedService>>>,
    browser_v2: Mutex<BTreeMap<String, Arc<browser_v2::Session>>>,
    browser_manual: Mutex<BTreeMap<String, bool>>,
    browser_selected: Mutex<BTreeMap<String, String>>,
    browser_bridge: Arc<browser_bridge::Bridge>,
    lifecycle: Mutex<()>,
    closing: AtomicBool,
    workers: Mutex<BTreeMap<String, std::thread::JoinHandle<()>>>,
    imports: Mutex<BTreeMap<String, Arc<AtomicBool>>>,
    imports_finished: Condvar,
    documents: document_engine::DocumentRuntime,
    // Drop last, after the services protected from a second startup cleanup.
    _root_owner: Arc<fs::File>,
}
struct ImportGuard<'a> {
    service: &'a Toolbox,
    id: String,
}
impl Drop for ImportGuard<'_> {
    fn drop(&mut self) {
        self.service
            .imports
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
        self.service.imports_finished.notify_all();
    }
}

fn namespace(path: &Path) -> Result<()> {
    for parent in path.ancestors() {
        match fs::symlink_metadata(parent) {
            Ok(metadata) => {
                if metadata.is_symlink() {
                    return Err(invalid("Managed namespace contains an alias"));
                }
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if metadata.file_attributes() & 0x400 != 0 {
                        return Err(invalid("Managed namespace contains a reparse point"));
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
fn single_file(file: &fs::File) -> Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(invalid("Expected a regular tool file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(invalid("Tool file has multiple hard links"));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetFileInformationByHandle(
                handle: *mut std::ffi::c_void,
                information: *mut [u32; 13],
            ) -> i32;
        }
        // BY_HANDLE_FILE_INFORMATION consists of 13 DWORDs, including FILETIMEs.
        let mut information = [0u32; 13];
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if information[0] & 0x400 != 0 || information[10] != 1 {
            return Err(invalid("Tool file is a reparse point or hard link"));
        }
    }
    Ok(())
}
fn private_dir(path: &Path) -> Result<()> {
    namespace(path)?;
    if let Ok(m) = fs::symlink_metadata(path)
        && (m.is_symlink() || !m.is_dir())
    {
        return Err(invalid("Managed directory changed"));
    }
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn relative(value: &str) -> Result<PathBuf> {
    let p = Path::new(value);
    if p.is_absolute()
        || p.components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
        || value.contains(['\0', ':'])
    {
        return Err(invalid("Expected a managed relative path"));
    }
    Ok(p.into())
}

#[cfg(windows)]
fn python_working_directory(path: &Path) -> Result<PathBuf> {
    // Python's MIME-file search can block when CreateProcess receives a \\?\ CWD.
    // Keep all authority checks canonical; use a conventional spelling for the
    // child only after verifying that it still denotes that exact directory.
    let value = path
        .to_str()
        .ok_or_else(|| invalid("Python working directory must be Unicode"))?;
    let Some(rest) = value.strip_prefix(r"\\?\") else {
        return Ok(path.into());
    };
    let candidate = if let Some(unc) = rest.strip_prefix(r"UNC\") {
        PathBuf::from(format!(r"\\{unc}"))
    } else if rest.as_bytes().get(1) == Some(&b':') && rest.as_bytes().get(2) == Some(&b'\\') {
        PathBuf::from(rest)
    } else {
        return Err(invalid(
            "Python needs a conventional Windows working directory",
        ));
    };
    if fs::canonicalize(&candidate)? != fs::canonicalize(path)? {
        return Err(invalid(
            "Python working directory spelling changes its physical identity",
        ));
    }
    Ok(candidate)
}
fn identifier(value: &str) -> Result<()> {
    Uuid::parse_str(value).map_err(|_| invalid("Expected a V2 identifier"))?;
    Ok(())
}

impl Toolbox {
    /// PTY callers retain their own reviewed admission and workspace lifetime.
    pub fn connector_adapter(&self) -> Result<PathBuf> {
        self.terminal_launch("python")?;
        Ok(self.adapters.join("connector.py"))
    }
    pub fn terminal_launch(&self, program: &str) -> Result<(PathBuf, BTreeMap<String, String>)> {
        if self.closing.load(Ordering::Acquire) {
            return Err(Error::Conflict);
        }
        let path = self
            .programs
            .lock()
            .map_err(|_| Error::Conflict)?
            .get(program)
            .cloned()
            .ok_or_else(|| invalid("Configured terminal program is unavailable"))?;
        Ok((path, self.config.environment.clone()))
    }
    pub fn open(config: Config) -> Result<Arc<Self>> {
        if !config.root.is_absolute() {
            return Err(invalid("Toolbox root must be absolute"));
        }
        private_dir(&config.root)?;
        let root_owner = Arc::new(root_owner::acquire(&config.root)?);
        for name in [
            "jobs",
            "workspaces",
            "sessions",
            "adapters",
            "home",
            "inputs",
            "imports",
            "locks",
            "documents",
        ] {
            private_dir(&config.root.join(name))?;
        }
        for path in config.programs.values() {
            if !path.is_absolute() || !path.is_file() {
                return Err(invalid("Configured executable is unavailable"));
            }
        }
        let inputs: [(&str, &[u8]); 12] = [
            ("managed_browser.py", include_bytes!("managed_browser.py")),
            ("connector.py", include_bytes!("connector.py")),
            (
                "knowledge_extract.py",
                include_bytes!("knowledge_extract.py"),
            ),
            ("persistent.py", include_bytes!("persistent.py")),
            (
                "persistent_browser.cjs",
                include_bytes!("persistent_browser.cjs"),
            ),
            ("office.py", include_bytes!("office.py")),
            ("browser.py", include_bytes!("browser.py")),
            ("git.py", include_bytes!("git.py")),
            ("pull_request.py", include_bytes!("pull_request.py")),
            ("v1_import.py", include_bytes!("v1_import.py")),
            (
                "v1_sqlite_export.py",
                include_bytes!("../../../tools/migration/v1_sqlite_export.py"),
            ),
            (
                "schema_profiles.py",
                include_bytes!("../../../tools/migration/schema_profiles.py"),
            ),
        ];
        let mut digest = Sha256::new();
        for (name, bytes) in inputs {
            digest.update(name.as_bytes());
            digest.update((bytes.len() as u64).to_be_bytes());
            digest.update(bytes);
        }
        let version: String = digest
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let adapters = config.root.join("adapters").join(version);
        private_dir(&adapters)?;
        for (name, bytes) in inputs {
            let path = adapters.join(name);
            match OpenOptions::new().create_new(true).write(true).open(&path) {
                Ok(mut file) => {
                    file.write_all(bytes)?;
                    file.sync_all()?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if fs::symlink_metadata(&path)?.is_symlink() || fs::read(path)? != bytes {
                        return Err(invalid("Pinned native adapter changed"));
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        let documents = document_engine::DocumentRuntime::install(&config.root)?;
        let service = Arc::new(Self {
            browser_bridge: browser_bridge::Bridge::new(config.root.clone()),
            browser_v2: Mutex::new(BTreeMap::new()),
            browser_manual: Mutex::new(BTreeMap::new()),
            browser_selected: Mutex::new(BTreeMap::new()),
            programs: Mutex::new(config.programs.clone()),
            config,
            adapters,
            jobs: Mutex::new(BTreeMap::new()),
            sessions: Mutex::new(BTreeMap::new()),
            managed: Mutex::new(BTreeMap::new()),
            lifecycle: Mutex::new(()),
            closing: AtomicBool::new(false),
            workers: Mutex::new(BTreeMap::new()),
            imports: Mutex::new(BTreeMap::new()),
            imports_finished: Condvar::new(),
            documents,
            _root_owner: root_owner,
        });
        let mut jobs = service.jobs.lock().map_err(|_| Error::Conflict)?;
        for entry in fs::read_dir(service.config.root.join("jobs"))?.take(2049) {
            let entry = entry?;
            if jobs.len() >= 2048 {
                return Err(invalid("Tool journal retention limit reached"));
            }
            if entry.file_type()?.is_symlink()
                || !entry.file_type()?.is_file()
                || entry.path().extension().is_none_or(|e| e != "json")
            {
                continue;
            }
            if entry.metadata()?.len() > 12 * 1024 * 1024 {
                return Err(invalid("Oversized tool journal"));
            }
            let mut job: Job = serde_json::from_slice(&fs::read(entry.path())?)?;
            identifier(&job.id)?;
            identifier(&job.order_id)?;
            if job.state == "running" {
                match service.order_lock(&job.order_id) {
                    Ok(_lease) => {
                        job.state = "uncertain".into();
                        job.error=Some("Daemon stopped before physical completion was recorded; execution is not replayed".into());
                        service.save(&job)?;
                    }
                    Err(Error::Conflict) => {}
                    Err(error) => return Err(error),
                }
            }
            jobs.insert(
                job.id.clone(),
                Running {
                    cancel: Arc::new(AtomicBool::new(false)),
                    job,
                    local: false,
                },
            );
        }
        drop(jobs);
        Ok(service)
    }
    fn order_lock(&self, order: &str) -> Result<fs::File> {
        identifier(order)?;
        let path = self.config.root.join("locks").join(order);
        namespace(&path)?;
        if fs::symlink_metadata(&path).is_ok_and(|m| m.is_symlink() || !m.is_file()) {
            return Err(invalid("Order lease path changed"));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        single_file(&file)?;
        file.try_lock().map_err(|_| Error::Conflict)?;
        Ok(file)
    }
    pub fn workspace_lease(&self, order: &str) -> Result<Arc<WorkspaceLease>> {
        Ok(Arc::new(WorkspaceLease {
            order: order.into(),
            root: self.config.root.clone(),
            _file: self.order_lock(order)?,
        }))
    }
    fn refresh(&self) -> Result<()> {
        let mut jobs = self.jobs.lock().map_err(|_| Error::Conflict)?;
        let mut count = 0;
        for entry in fs::read_dir(self.config.root.join("jobs"))? {
            let entry = entry?;
            if entry.path().extension().is_none_or(|e| e != "json") {
                continue;
            }
            count += 1;
            if count > 2048
                || entry.file_type()?.is_symlink()
                || !entry.file_type()?.is_file()
                || entry.metadata()?.len() > 12 * 1024 * 1024
            {
                return Err(invalid("Invalid or oversized tool journal"));
            }
            let job: Job = serde_json::from_slice(&fs::read(entry.path())?)?;
            identifier(&job.id)?;
            identifier(&job.order_id)?;
            if entry.path().file_stem().and_then(|s| s.to_str()) != Some(&job.id) {
                return Err(invalid("Tool journal identity changed"));
            }
            if let Some(item) = jobs.get_mut(&job.id) {
                item.job = job;
            } else {
                jobs.insert(
                    job.id.clone(),
                    Running {
                        cancel: Arc::new(AtomicBool::new(false)),
                        job,
                        local: false,
                    },
                );
            }
        }
        Ok(())
    }
    pub fn availability(&self) -> Availability {
        let extension_available = self.browser_v2.lock().is_ok_and(|s| {
            s.values()
                .any(|v| v.view.mode == cowork_contracts::BrowserMode::Extension)
        });
        let programs = self.programs.lock().unwrap_or_else(|e| e.into_inner());
        let (
            documents_available,
            latex_available,
            document_engine_version,
            office_convert_available,
            document_status,
            latex_status,
            office_convert_status,
        ) = self
            .documents
            .availability(&programs, &self.config.environment, &self._root_owner);
        let ocr_status = self.document_ocr_status(&programs, documents_available);
        Availability {
            mode: "owner_native".into(),
            programs: programs.keys().cloned().collect(),
            browser_available: programs.contains_key("chromium")
                || programs.contains_key("chrome")
                || programs.contains_key("firefox")
                || programs.contains_key("browser_host")
                || extension_available,
            office_available: programs.contains_key("python"),
            github_available: programs.contains_key("gh") && self.config.github_token.is_some(),
            filesystem_isolation: false,
            documents_available,
            ocr_available: ocr_status == "ready",
            ocr_status: ocr_status.into(),
            latex_available,
            office_convert_available,
            document_status,
            latex_status,
            office_convert_status,
            document_engine_version,
        }
    }
    /// Native setup reloads only executable selection. Existing jobs keep their
    /// admitted launch, workspace, credentials and lifecycle supervision.
    pub fn reload_programs(&self, programs: BTreeMap<String, PathBuf>) -> Result<Availability> {
        let _lifecycle = self.lifecycle.lock().map_err(|_| Error::Conflict)?;
        if self.closing.load(Ordering::Acquire) {
            return Err(invalid("Native tool service is stopping"));
        }
        if programs.len() > 32
            || programs.iter().any(|(name, path)| {
                name.is_empty()
                    || name.len() > 64
                    || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    || !path.is_absolute()
                    || !path.is_file()
            })
        {
            return Err(invalid("Configured executable is unavailable"));
        }
        self.programs
            .lock()
            .map_err(|_| Error::Conflict)?
            .extend(programs);
        self.documents.invalidate();
        Ok(self.availability())
    }
    pub fn setup_data_dir(&self) -> Result<PathBuf> {
        self.config
            .root
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| invalid("Native toolbox data directory is unavailable"))
    }
    /// An offline grant gets isolated workspaces and only its explicitly supplied
    /// personal GitHub key. Team/owner keys are never inherited.
    pub fn fork_offline(&self, grant: &str, github_token: Option<String>) -> Result<Arc<Self>> {
        identifier(grant)?;
        Self::open(Config {
            root: self.config.root.join("offline").join(grant),
            programs: self.programs.lock().map_err(|_| Error::Conflict)?.clone(),
            environment: self.config.environment.clone(),
            github_token,
        })
    }
    pub fn workspace(&self, order: &str) -> Result<PathBuf> {
        identifier(order)?;
        let path = self.config.root.join("workspaces").join(order);
        private_dir(&path)?;
        Ok(fs::canonicalize(path)?)
    }
    pub fn create_browser_session(&self, order: &str) -> Result<BrowserSession> {
        identifier(order)?;
        let mut sessions = self.sessions.lock().map_err(|_| Error::Conflict)?;
        if sessions.len() >= 64 {
            return Err(invalid("Close unused browser sessions first"));
        }
        let session = BrowserSession {
            id: Uuid::new_v4().to_string(),
            order_id: order.into(),
            url: None,
        };
        private_dir(&self.config.root.join("sessions").join(&session.id))?;
        sessions.insert(session.id.clone(), session.clone());
        Ok(session)
    }
    pub fn close_browser_session(&self, order: &str, id: &str) -> Result<()> {
        let v2 = self
            .browser_v2
            .lock()
            .map_err(|_| Error::Conflict)?
            .get(id)
            .cloned();
        let _gate = match &v2 {
            Some(v) => Some(v.gate.lock().map_err(|_| Error::Conflict)?),
            None => None,
        };
        let mut sessions = self.sessions.lock().map_err(|_| Error::Conflict)?;
        if sessions.get(id).is_none_or(|s| s.order_id != order) {
            return Err(Error::NotFound);
        }
        let jobs = self.jobs.lock().map_err(|_| Error::Conflict)?;
        if jobs
            .values()
            .any(|j| j.job.order_id == order && j.job.state == "running")
        {
            return Err(Error::Conflict);
        }
        sessions.remove(id);
        drop(jobs);
        drop(sessions);
        self.stop_persistent(id)?;
        self.browser_v2
            .lock()
            .map_err(|_| Error::Conflict)?
            .remove(id);
        self.browser_manual
            .lock()
            .map_err(|_| Error::Conflict)?
            .remove(id);
        Ok(())
    }
    fn save(&self, job: &Job) -> Result<()> {
        let directory = self.config.root.join("jobs");
        let target = directory.join(format!("{}.json", job.id));
        let temporary = directory.join(format!(".{}", Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(&serde_json::to_vec(job)?)?;
        file.sync_all()?;
        drop(file);
        if fs::symlink_metadata(&target).is_ok_and(|m| m.is_symlink()) {
            return Err(invalid("Tool journal path changed"));
        }
        fs::rename(temporary, target)?;
        Ok(())
    }
    fn adapter_input(&self, value: &serde_json::Value) -> Result<String> {
        let bytes = serde_json::to_vec(value)?;
        if bytes.len() > 2 * 1024 * 1024 {
            return Err(invalid("Adapter input exceeds2MiB"));
        }
        let path = self
            .config
            .root
            .join("inputs")
            .join(format!("{}.json", hash(&bytes)));
        match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(mut file) => {
                file.write_all(&bytes)?;
                file.sync_all()?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if fs::symlink_metadata(&path)?.is_symlink() || fs::read(&path)? != bytes {
                    return Err(invalid("Adapter input changed"));
                }
            }
            Err(error) => return Err(error.into()),
        }
        Ok(format!("@{}", path.display()))
    }
    fn launch(&self, order: &str, request: &Request) -> Result<cowork_executor::TrustedLaunch> {
        let programs = self.programs.lock().map_err(|_| Error::Conflict)?.clone();
        if request.idempotency_key.is_empty()
            || request.idempotency_key.len() > 128
            || !(1..=600).contains(&request.timeout_seconds)
            || !(1..=1024 * 1024).contains(&request.output_bytes)
        {
            return Err(invalid("Tool bounds are outside the supported range"));
        }
        let workspace = self.workspace(order)?;
        if let Action::Browser {
            session_id,
            operation,
            url,
            selector,
            text,
        } = &request.action
            && self.has_browser_v2(session_id)
        {
            self.browser_sessions_v2(order)?
                .into_iter()
                .find(|s| s.id == *session_id)
                .ok_or(Error::NotFound)?;
            let command = if operation == "control" {
                serde_json::from_str::<cowork_contracts::BrowserCommand>(
                    text.as_deref().unwrap_or(""),
                )?
            } else {
                cowork_contracts::BrowserCommand {
                    operation: operation.clone(),
                    url: url.clone(),
                    selector: selector.clone(),
                    text: text.clone(),
                    ..Default::default()
                }
            };
            validate_browser_command(&command)?;
            // The launch identity is hashed into the existing approval receipt.
            return Ok(cowork_executor::TrustedLaunch {
                program: std::env::current_exe()?,
                arguments: vec!["browser-v2-admitted".into(), session_id.clone()],
                directory: workspace,
                environment: BTreeMap::new(),
                timeout: Duration::from_secs(request.timeout_seconds),
                capture_bytes_per_stream: request.output_bytes,
            });
        }
        let mut directory = workspace.clone();
        let mut environment = self.config.environment.clone();
        let mut tool_paths: Vec<PathBuf> = programs
            .values()
            .filter_map(|p| p.parent().map(Path::to_path_buf))
            .collect();
        if let Some(existing) = environment.get("PATH") {
            tool_paths.extend(std::env::split_paths(existing));
        }
        let path = std::env::join_paths(tool_paths)
            .map_err(|_| invalid("Invalid configured tool path"))?;
        environment.insert("PATH".into(), path.to_string_lossy().into_owned());
        environment.insert(
            "HOME".into(),
            self.config.root.join("home").to_string_lossy().into(),
        );
        environment.insert(
            "USERPROFILE".into(),
            self.config.root.join("home").to_string_lossy().into(),
        );
        environment.insert("PYTHONNOUSERSITE".into(), "1".into());
        private_dir(&workspace.join("tmp"))?;
        for key in ["TMP", "TEMP", "TMPDIR"] {
            environment.insert(key.into(), workspace.join("tmp").to_string_lossy().into());
        }
        let (name, arguments) = match &request.action {
            Action::Terminal {
                program,
                arguments,
                cwd,
            } => {
                directory = fs::canonicalize(workspace.join(relative(cwd)?))?;
                if !directory.starts_with(&workspace) {
                    return Err(invalid("Working directory escapes its order"));
                }
                (program.clone(), arguments.clone())
            }
            Action::Github { arguments } => {
                if arguments.is_empty()
                    || !matches!(
                        arguments[0].as_str(),
                        "api" | "repo" | "pr" | "issue" | "gist" | "release" | "run" | "workflow"
                    )
                    || arguments
                        .iter()
                        .any(|a| matches!(a.as_str(), "--web" | "--browser" | "--hostname"))
                {
                    return Err(invalid(
                        "Unsupported GitHub command; use the explicit API/PR/issue adapters",
                    ));
                }
                environment.insert(
                    "GH_TOKEN".into(),
                    self.config
                        .github_token
                        .clone()
                        .ok_or_else(|| invalid("Configure the daemon GitHub token first"))?,
                );
                environment.insert("GH_PROMPT_DISABLED".into(), "1".into());
                environment.insert(
                    "GH_CONFIG_DIR".into(),
                    self.config.root.join("home/gh").to_string_lossy().into(),
                );
                ("gh".into(), arguments.clone())
            }
            Action::PullRequest {
                repository,
                number,
                operation,
                expected_head,
                event,
                body,
                merge_method,
            } => {
                if repository.len() > 140
                    || repository.split('/').count() != 2
                    || repository
                        .bytes()
                        .any(|b| !b.is_ascii_alphanumeric() && !b"/-_.".contains(&b))
                    || *number == 0
                    || *number >= 1_000_000_000
                    || !["inspect", "diff", "reviews", "review", "merge"]
                        .contains(&operation.as_str())
                    || body.as_ref().is_some_and(|v| v.len() > 40000)
                    || event.as_ref().is_some_and(|v| {
                        !["APPROVE", "REQUEST_CHANGES", "COMMENT"].contains(&v.as_str())
                    })
                    || merge_method
                        .as_ref()
                        .is_some_and(|v| !["merge", "squash", "rebase"].contains(&v.as_str()))
                {
                    return Err(invalid("Invalid pinned pull request action"));
                }
                if operation != "inspect"
                    && expected_head.as_ref().is_none_or(|sha| {
                        ![40, 64].contains(&sha.len())
                            || !sha.bytes().all(|b| b.is_ascii_hexdigit())
                    })
                {
                    return Err(invalid(
                        "Inspect and pin the current pull request head first",
                    ));
                }
                let gh = programs
                    .get("gh")
                    .ok_or_else(|| invalid("Configure GitHub CLI first"))?;
                environment.insert(
                    "GH_TOKEN".into(),
                    self.config
                        .github_token
                        .clone()
                        .ok_or_else(|| invalid("Configure GitHub credentials first"))?,
                );
                environment.insert("GH_PROMPT_DISABLED".into(), "1".into());
                environment.insert(
                    "GH_CONFIG_DIR".into(),
                    self.config.root.join("home/gh").to_string_lossy().into(),
                );
                (
                    "python".into(),
                    vec![
                        "-I".into(),
                        self.adapters
                            .join("pull_request.py")
                            .to_string_lossy()
                            .into(),
                        gh.to_string_lossy().into(),
                        self.adapter_input(&serde_json::to_value(&request.action)?)?,
                    ],
                )
            }
            Action::Git {
                operation,
                cwd,
                expected_state,
                paths,
                reference,
                text,
            } => {
                if ![
                    "status",
                    "diff",
                    "log",
                    "branches",
                    "init",
                    "clone",
                    "stage",
                    "unstage",
                    "commit",
                    "create_branch",
                    "switch_branch",
                    "delete_branch",
                    "fetch",
                    "push",
                ]
                .contains(&operation.as_str())
                    || paths.len() > 256
                    || paths.iter().any(|p| p.len() > 4096 || relative(p).is_err())
                    || text.as_ref().is_some_and(|v| v.len() > 40000)
                    || reference
                        .as_ref()
                        .is_some_and(|v| v.len() > 4096 || v.contains(['\0', '\r', '\n']))
                {
                    return Err(invalid("Invalid structured Git action"));
                }
                relative(cwd)?;
                if !["status", "diff", "log", "branches"].contains(&operation.as_str())
                    && expected_state
                        .as_ref()
                        .is_none_or(|v| v.len() != 64 || !v.bytes().all(|b| b.is_ascii_hexdigit()))
                {
                    return Err(invalid(
                        "Refresh and bind the exact repository state before mutation",
                    ));
                }
                let git = programs
                    .get("git")
                    .ok_or_else(|| invalid("Configure Git first"))?;
                if let Some(token) = &self.config.github_token {
                    environment.insert("COWORK_GIT_TOKEN".into(), token.clone());
                }
                (
                    "python".into(),
                    vec![
                        "-I".into(),
                        self.adapters.join("git.py").to_string_lossy().into(),
                        workspace.to_string_lossy().into(),
                        git.to_string_lossy().into(),
                        self.adapter_input(&serde_json::to_value(&request.action)?)?,
                    ],
                )
            }
            Action::Office { operation, .. } => {
                let mut value = serde_json::to_value(&request.action)?;
                if let Some(path) = programs.get("libreoffice") {
                    value["libreoffice"] = serde_json::Value::String(path.to_string_lossy().into());
                }
                let encoded = self.adapter_input(&value)?;
                let mut arguments = vec!["-I".into()];
                if operation == "extract_knowledge" {
                    arguments.extend(["-X".into(), "utf8".into()]);
                }
                arguments.extend([
                    self.adapters
                        .join(if operation == "extract_knowledge" {
                            "knowledge_extract.py"
                        } else {
                            "office.py"
                        })
                        .to_string_lossy()
                        .into(),
                    workspace.to_string_lossy().into(),
                    encoded,
                ]);
                ("python".into(), arguments)
            }
            Action::Browser {
                session_id,
                operation,
                url,
                ..
            } => {
                let sessions = self.sessions.lock().map_err(|_| Error::Conflict)?;
                let session = sessions
                    .get(session_id)
                    .filter(|s| s.order_id == order)
                    .ok_or(Error::NotFound)?;
                let selected = url
                    .as_ref()
                    .or(session.url.as_ref())
                    .ok_or_else(|| invalid("Navigate this browser session first"))?;
                if !(selected.starts_with("https://") || selected.starts_with("http://"))
                    || selected.len() > 8192
                    || selected.contains(['\0', '\r', '\n'])
                {
                    return Err(invalid("Browser navigation requires an HTTP(S) URL"));
                }
                if !matches!(
                    operation.as_str(),
                    "navigate"
                        | "dom"
                        | "screenshot"
                        | "click"
                        | "fill"
                        | "annotate"
                        | "annotations"
                        | "clear_annotations"
                        | "console"
                        | "network"
                        | "network_details"
                        | "fetch_text"
                        | "search"
                ) {
                    return Err(invalid("Unknown browser operation"));
                }
                let mut encoded = serde_json::to_value(&request.action)?;
                encoded["url"] = serde_json::Value::String(selected.clone());
                let profile = self.config.root.join("sessions").join(session_id);
                if let (Some(_), Some(module)) = (programs.get("node"), programs.get("playwright"))
                {
                    (
                        "node".into(),
                        vec![
                            self.adapters
                                .join("persistent_browser.cjs")
                                .to_string_lossy()
                                .into(),
                            module.to_string_lossy().into(),
                            self.adapter_input(&encoded)?,
                        ],
                    )
                } else if programs.contains_key("python") {
                    let chromium = programs
                        .get("chromium")
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    (
                        "python".into(),
                        vec![
                            "-I".into(),
                            self.adapters.join("browser.py").to_string_lossy().into(),
                            profile.to_string_lossy().into(),
                            workspace.to_string_lossy().into(),
                            chromium,
                            self.adapter_input(&encoded)?,
                        ],
                    )
                } else {
                    if !matches!(operation.as_str(), "navigate" | "dom" | "screenshot") {
                        return Err(invalid(
                            "Install Playwright Python for browser interactions",
                        ));
                    }
                    let mut args = vec![
                        "--headless=new".into(),
                        "--no-first-run".into(),
                        "--no-default-browser-check".into(),
                        format!("--user-data-dir={}", profile.display()),
                    ];
                    if operation == "screenshot" {
                        args.push(format!(
                            "--screenshot={}",
                            workspace
                                .join(format!("browser-{session_id}.png"))
                                .display()
                        ));
                        args.push("--window-size=1440,960".into());
                    } else {
                        args.push("--dump-dom".into());
                    }
                    args.push(selected.clone());
                    ("chromium".into(), args)
                }
            }
        };
        let program = programs
            .get(&name)
            .ok_or_else(|| invalid(&format!("Executable adapter '{name}' is unavailable")))?
            .clone();
        if arguments.len() > 128
            || arguments.iter().map(String::len).sum::<usize>() > 32000
            || arguments.iter().any(|a| a.contains('\0'))
        {
            return Err(invalid("Oversized or invalid tool arguments"));
        }
        #[cfg(windows)]
        if name == "python" {
            directory = python_working_directory(&directory)?;
        }
        Ok(cowork_executor::TrustedLaunch {
            program,
            arguments,
            directory,
            environment,
            timeout: Duration::from_secs(request.timeout_seconds),
            capture_bytes_per_stream: request.output_bytes,
        })
    }
    pub fn preview(&self, order: &str, request: &Request) -> Result<Preview> {
        let launch = self.launch(order, request)?;
        let request_sha256 = hash(&serde_json::to_vec(&(
            order,
            &request.idempotency_key,
            &request.action,
            &launch.program,
            &launch.arguments,
            &launch.environment,
            &launch.directory,
            request.timeout_seconds,
            request.output_bytes,
        ))?);
        Ok(Preview {
            request_sha256,
            mode: "owner_native".into(),
            cwd: launch.directory.to_string_lossy().into(),
            timeout_seconds: request.timeout_seconds,
            output_bytes: request.output_bytes,
            approval_required: true,
        })
    }
    pub fn submit(self: &Arc<Self>, order: &str, request: Request) -> Result<Job> {
        self.submit_inner(order, request, None, || Ok(()))
    }
    pub fn submit_with_lease(
        self: &Arc<Self>,
        order: &str,
        request: Request,
        lease: Arc<WorkspaceLease>,
    ) -> Result<Job> {
        if lease.order != order || lease.root != self.config.root {
            return Err(Error::Conflict);
        }
        self.submit_inner(order, request, Some(lease), || Ok(()))
    }
    /// A new job checks its chat while holding exclusive workspace custody;
    /// original receipts return before this check and never start another job.
    pub fn submit_checked(
        self: &Arc<Self>,
        order: &str,
        request: Request,
        check: impl FnOnce() -> Result<()>,
    ) -> Result<Job> {
        self.submit_inner(order, request, None, check)
    }
    fn submit_inner(
        self: &Arc<Self>,
        order: &str,
        request: Request,
        lease: Option<Arc<WorkspaceLease>>,
        check: impl FnOnce() -> Result<()>,
    ) -> Result<Job> {
        let _lifecycle = self.lifecycle.lock().map_err(|_| Error::Conflict)?;
        if self.closing.load(Ordering::Acquire) {
            return Err(invalid("Native tool service is stopping"));
        }
        let preview = self.preview(order, &request)?;
        if request.approval_sha256.as_deref() != Some(&preview.request_sha256) {
            return Err(invalid(
                "Explicit approval for this exact tool request is required",
            ));
        }
        let launch = self.launch(order, &request)?;
        self.refresh()?;
        {
            let jobs = self.jobs.lock().map_err(|_| Error::Conflict)?;
            if let Some(old) = jobs.values().find(|j| {
                j.job.order_id == order && j.job.idempotency_key == request.idempotency_key
            }) {
                if old.job.request_sha256 != preview.request_sha256 {
                    return Err(Error::Conflict);
                }
                return Ok(old.job.clone());
            }
        }
        let lease = match lease {
            Some(lease) => lease,
            None => self.workspace_lease(order)?,
        };
        check()?;
        self.refresh()?;
        let mut jobs = self.jobs.lock().map_err(|_| Error::Conflict)?;
        if let Some(old) = jobs
            .values()
            .find(|j| j.job.order_id == order && j.job.idempotency_key == request.idempotency_key)
        {
            if old.job.request_sha256 != preview.request_sha256 {
                return Err(Error::Conflict);
            }
            return Ok(old.job.clone());
        }
        if jobs.len() >= 2048
            || jobs.values().filter(|j| j.job.state == "running").count() >= 4
            || jobs
                .values()
                .any(|j| j.job.order_id == order && j.job.state == "running")
        {
            return Err(invalid(
                "An order tool is busy or the journal retention limit was reached",
            ));
        }
        let job = Job {
            id: Uuid::new_v4().to_string(),
            order_id: order.into(),
            idempotency_key: request.idempotency_key.clone(),
            request_sha256: preview.request_sha256,
            state: "running".into(),
            outcome: None,
            error: None,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.save(&job)?;
        jobs.insert(
            job.id.clone(),
            Running {
                cancel: cancel.clone(),
                job: job.clone(),
                local: true,
            },
        );
        drop(jobs);
        if let Action::Browser {
            session_id,
            url: Some(url),
            ..
        } = &request.action
            && let Some(session) = self
                .sessions
                .lock()
                .map_err(|_| Error::Conflict)?
                .get_mut(session_id)
        {
            session.url = Some(url.clone());
        }
        let service = self.clone();
        let id = job.id.clone();
        let job_order = job.order_id.clone();
        let spawned=std::thread::Builder::new().name(format!("toolbox-{id}")).spawn(move||{
            let outcome=if matches!(&request.action,Action::Browser{session_id,..} if service.has_browser_v2(session_id)) {
                service.browser_v2_outcome(&job_order,&request,&cancel).map_err(|e|e.to_string())
            }else if matches!(&request.action,Action::Browser{..}) {
                service.persistent_browser(&job_order,&request,launch,&cancel).map_err(|e|e.to_string())
            }else{cowork_executor::supervise(&launch,&cancel).map_err(|e|e.to_string())};
            let document_error=outcome.as_ref().ok().and_then(|value|service.snapshot_office_result(&job_order,&request,&id,value,lease.clone()).err()).map(|error|format!("Document output exists but its immutable revision could not be confirmed: {error}"));
            if let Ok(mut jobs)=service.jobs.lock()&& let Some(running)=jobs.get_mut(&id) {
                match outcome {Ok(value)=>{running.job.state=if document_error.is_some(){"uncertain"}else{"completed"}.into();running.job.outcome=Some(value);running.job.error=document_error;},Err(error)=>{running.job.state="uncertain".into();running.job.error=Some(error.to_string());}}
                if service.save(&running.job).is_err(){running.job.state="uncertain".into();running.job.error=Some("Physical result was not durably stored; never replay automatically".into());}
            }
            drop(lease);
        });
        match spawned {
            Ok(handle) => {
                self.workers
                    .lock()
                    .map_err(|_| Error::Conflict)?
                    .insert(job.id.clone(), handle);
            }
            Err(error) => {
                if let Ok(mut jobs) = self.jobs.lock()
                    && let Some(running) = jobs.get_mut(&job.id)
                {
                    running.job.state = "uncertain".into();
                    running.job.error = Some(
                        "Native worker could not start; this admission will not be replayed".into(),
                    );
                    self.save(&running.job)?;
                }
                return Err(error.into());
            }
        }
        Ok(job)
    }
    pub fn status(&self, order: &str, id: &str) -> Result<Job> {
        self.refresh()?;
        self.jobs
            .lock()
            .map_err(|_| Error::Conflict)?
            .get(id)
            .filter(|r| r.job.order_id == order)
            .map(|r| r.job.clone())
            .ok_or(Error::NotFound)
    }
    pub fn jobs(&self, order: &str) -> Result<Vec<Job>> {
        self.refresh()?;
        Ok(self
            .jobs
            .lock()
            .map_err(|_| Error::Conflict)?
            .values()
            .filter(|r| r.job.order_id == order)
            .map(|r| r.job.clone())
            .collect())
    }
    pub fn shutdown(&self) {
        self.browser_bridge.shutdown();
        let lifecycle = self.lifecycle.lock().unwrap_or_else(|e| e.into_inner());
        self.closing.store(true, Ordering::Release);
        if let Ok(jobs) = self.jobs.lock() {
            for job in jobs.values() {
                if job.local {
                    job.cancel.store(true, Ordering::Release);
                }
            }
        }
        for cancel in self
            .imports
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            cancel.store(true, Ordering::Release);
        }
        let workers = std::mem::take(&mut *self.workers.lock().unwrap_or_else(|e| e.into_inner()));
        drop(lifecycle);
        let managed = std::mem::take(&mut *self.managed.lock().unwrap_or_else(|e| e.into_inner()));
        for (_, context) in managed {
            context.stop();
        }
        for (_, worker) in workers {
            let _ = worker.join();
        }
        let mut imports = self.imports.lock().unwrap_or_else(|e| e.into_inner());
        while !imports.is_empty() {
            imports = self
                .imports_finished
                .wait(imports)
                .unwrap_or_else(|e| e.into_inner());
        }
    }
    pub fn cancel(&self, order: &str, id: &str) -> Result<Job> {
        let jobs = self.jobs.lock().map_err(|_| Error::Conflict)?;
        let item = jobs
            .get(id)
            .filter(|r| r.job.order_id == order)
            .ok_or(Error::NotFound)?;
        if !item.local && item.job.state == "running" {
            return Err(invalid(
                "This native action belongs to the worker process; cancel its admitted run",
            ));
        }
        item.cancel.store(true, Ordering::Release);
        Ok(item.job.clone())
    }
    pub fn read_output_file(&self, order: &str, path: &str) -> Result<Vec<u8>> {
        self.read_output_file_with_lease(order, path, self.workspace_lease(order)?)
    }
    pub fn read_output_file_with_lease(
        &self,
        order: &str,
        path: &str,
        lease: Arc<WorkspaceLease>,
    ) -> Result<Vec<u8>> {
        if lease.order != order || lease.root != self.config.root {
            return Err(Error::Conflict);
        }
        let _lease = lease;
        let root = self.workspace(order)?;
        let selected = root.join(relative(path)?);
        namespace(&selected)?;
        let target = fs::canonicalize(selected)?;
        if !target.starts_with(&root)
            || fs::symlink_metadata(&target)?.is_symlink()
            || !target.is_file()
        {
            return Err(invalid("Output file escapes its order"));
        }
        let mut bytes = Vec::new();
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1).custom_flags(0x00200000);
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(0x20000);
        }
        let file = options.open(&target)?;
        single_file(&file)?;
        if file.metadata()?.len() > 32 * 1024 * 1024 {
            return Err(invalid("Output download exceeds32MiB"));
        }
        (&file).take(32 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        single_file(&file)?;
        namespace(&target)?;
        if bytes.len() > 32 * 1024 * 1024 {
            return Err(invalid("Output download exceeds32MiB"));
        }
        Ok(bytes)
    }
    pub fn write_input_file(&self, order: &str, path: &str, bytes: &[u8]) -> Result<()> {
        self.write_input_file_with_lease(order, path, bytes, self.workspace_lease(order)?)
    }
    pub fn write_input_file_with_lease(
        &self,
        order: &str,
        path: &str,
        bytes: &[u8],
        lease: Arc<WorkspaceLease>,
    ) -> Result<()> {
        if lease.order != order || lease.root != self.config.root {
            return Err(Error::Conflict);
        }
        let _lease = lease;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(invalid("Tool input upload exceeds16MiB"));
        }
        let root = self.workspace(order)?;
        let target = root.join(relative(path)?);
        let parent_path = target
            .parent()
            .ok_or_else(|| invalid("Expected input file"))?;
        let mut checked = root.clone();
        for part in parent_path
            .strip_prefix(&root)
            .map_err(|_| invalid("Input escapes its order"))?
            .components()
        {
            checked.push(part);
            private_dir(&checked)?;
            if !fs::canonicalize(&checked)?.starts_with(&root) {
                return Err(invalid("Input directory escapes its order"));
            }
        }
        let parent = fs::canonicalize(target.parent().unwrap())?;
        if !parent.starts_with(&root) {
            return Err(invalid("Tool input path escapes its order"));
        }
        let mut file = OpenOptions::new().create_new(true).write(true).open(
            parent.join(
                target
                    .file_name()
                    .ok_or_else(|| invalid("Expected input file"))?,
            ),
        )?;
        file.write_all(bytes)?;
        file.sync_all()?;
        single_file(&file)?;
        namespace(&parent)?;
        self.snapshot_uploaded_document(order, path, bytes)?;
        Ok(())
    }
    /// Native picker callers only. Path is never inferred from an account/profile.
    /// The returned JSON contains inert text/IDs, never a Runtime RunSpec.
    pub fn capture_v1_content(
        &self,
        path: &Path,
        kind: &str,
        cold_source_confirmed: bool,
    ) -> Result<serde_json::Value> {
        self.capture_v1_content_with_options(path, kind, cold_source_confirmed, "desktop", None)
    }
    pub fn capture_v1_content_with_options(
        &self,
        path: &Path,
        kind: &str,
        cold_source_confirmed: bool,
        source_family: &str,
        source_root: Option<&Path>,
    ) -> Result<serde_json::Value> {
        let lifecycle = self.lifecycle.lock().map_err(|_| Error::Conflict)?;
        if self.closing.load(Ordering::Acquire) {
            return Err(invalid("Native tool service is stopping"));
        }
        let identifier = Uuid::new_v4().to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        self.imports
            .lock()
            .map_err(|_| Error::Conflict)?
            .insert(identifier.clone(), cancel.clone());
        let _guard = ImportGuard {
            service: self,
            id: identifier.clone(),
        };
        drop(lifecycle);
        if !path.is_absolute()
            || !matches!(kind, "json" | "sqlite")
            || !matches!(source_family, "desktop" | "daemon" | "enterprise")
            || source_root.is_some_and(|root| !root.is_absolute())
        {
            return Err(invalid("Select an absolute V1 JSON/SQLite file"));
        }
        let program = self
            .programs
            .lock()
            .map_err(|_| Error::Conflict)?
            .get("python")
            .ok_or_else(|| invalid("Configure Python for selected V1 imports"))?
            .clone();
        let stage = self.config.root.join("imports").join(identifier);
        private_dir(&stage)?;
        let mut environment = self.config.environment.clone();
        for key in ["HOME", "USERPROFILE"] {
            environment.insert(
                key.into(),
                self.config.root.join("home").to_string_lossy().into(),
            );
        }
        for key in ["TEMP", "TMP", "TMPDIR"] {
            environment.insert(key.into(), stage.to_string_lossy().into());
        }
        let launch = cowork_executor::TrustedLaunch {
            program,
            arguments: vec![
                "-I".into(),
                self.adapters.join("v1_import.py").to_string_lossy().into(),
                path.to_string_lossy().into(),
                kind.into(),
                stage.to_string_lossy().into(),
                cold_source_confirmed.to_string(),
                source_family.into(),
                source_root
                    .map(|root| root.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ],
            directory: stage.clone(),
            environment,
            timeout: Duration::from_secs(120),
            capture_bytes_per_stream: 64 * 1024,
        };
        let result = cowork_executor::supervise(&launch, &cancel)
            .map_err(|_| invalid("Selected V1 capture failed; originals were not SQLite-opened"))?;
        if result.exit_code != Some(0)
            || result.termination != cowork_executor::Termination::Exited
            || !result.output_complete
            || result.stdout.truncated
        {
            let reason = String::from_utf8_lossy(&result.stderr.bytes);
            return Err(Error::Invalid(format!(
                "V1 capture refused: {}",
                reason.chars().take(2048).collect::<String>()
            )));
        }
        let receipt: serde_json::Value = serde_json::from_slice(&result.stdout.bytes)?;
        let file = stage.join("content.json");
        if fs::symlink_metadata(&file)?.is_symlink() || file.metadata()?.len() > 64 * 1024 * 1024 {
            return Err(invalid("Invalid private import content"));
        }
        let raw = fs::read(file)?;
        if receipt.get("content_sha256").and_then(|v| v.as_str()) != Some(&hash(&raw)) {
            return Err(invalid("Private import content changed"));
        }
        Ok(serde_json::from_slice(&raw)?)
    }
}
