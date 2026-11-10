//! Built-in document engine for the model tools document_read, document_write and image_edit.
//! Inputs are SHA-checked copies in a private stage; the adapters are pinned and the
//! programs fixed. Processes keep the owner's OS rights: this is stage discipline and
//! bounded IO, not a sandbox. The exception is Tectonic, which runs OS-confined
//! (`compile_tex`). Publishing to a folder is the file broker's job.
use super::*;
use serde_json::{Value, json};
mod document_root;
mod path_limits;
use path_limits::{document_working_directory_fits, latex_working_directories_fit};

struct DocumentLaunch<'a> {
    program: &'a str,
    arguments: Vec<String>,
    directory: &'a Path,
    stage: &'a Path,
    timeout: Duration,
    capture: usize,
}

const DOCUMENT_ENGINE: &[u8] = include_bytes!("document_engine.py");
const IMAGE_ENGINE: &[u8] = include_bytes!("image_engine.py");
const OCR_PROBE: &[u8] = include_bytes!("ocr_probe.py");
pub const MAX_DOCUMENT_INPUT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_OUTPUT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_OUTPUTS: usize = 64;
const MAX_RECEIPT_BYTES: usize = 1024 * 1024;
const MAX_RESULT_BYTES: usize = 32 * 1024;
const MAX_PREVIEWS: usize = 4;
const MAX_PREVIEW_BYTES: u64 = 2 * 1024 * 1024;
const ENGINE_SLOTS: usize = 2;
const OFFICE_EXTENSIONS: [&str; 9] = [
    "docx", "xlsx", "pptx", "odt", "ods", "odp", "doc", "xls", "ppt",
];

/// One SHA-verified input copy. `path` is the user-visible path inside `folder_id`.
#[derive(Clone, Debug)]
pub struct DocumentInputFile {
    pub folder_id: String,
    pub path: String,
    pub sha256: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct DocumentJob {
    pub engine_revision: Option<String>,
    pub tool: String,
    pub operation: String,
    pub inputs: Vec<DocumentInputFile>,
    /// Basename of the requested output; a directory name when `output_directory`.
    pub output_name: Option<String>,
    pub output_directory: bool,
    pub spec: Value,
    pub options: Value,
    pub timeout: Duration,
}

/// A published output. `path` is empty for a single file, else relative inside the new directory.
#[derive(Clone, Debug)]
pub struct DocumentFile {
    pub path: String,
    pub bytes: Vec<u8>,
    pub sha256: String,
    pub media_type: String,
}

#[derive(Clone, Debug)]
pub struct DocumentPreview {
    pub page: u32,
    pub width: u32,
    pub height: u32,
    pub sha256: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct DocumentOutcome {
    pub ok: bool,
    /// Engine receipt with model-facing `result`, `diagnostics`, `qa` or `error`.
    pub receipt: Value,
    pub receipt_sha256: String,
    pub engine_version: String,
    pub outputs: Vec<DocumentFile>,
    pub previews: Vec<DocumentPreview>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ProbeKey {
    generation: u64,
    python: Option<PathBuf>,
    tectonic: Option<PathBuf>,
    runtime: Option<PathBuf>,
    office: Option<PathBuf>,
}

#[derive(Default)]
struct DocumentState {
    key: Option<ProbeKey>,
    probing: bool,
    documents: bool,
    latex: bool,
    version: Option<String>,
    office: bool,
    document_status: String,
    latex_status: String,
    office_status: String,
    ocr_native: Option<super::document_ocr::NativeIdentity>,
}

pub(crate) struct DocumentRuntime {
    root: Arc<document_root::DocumentRoot>,
    generation: std::sync::atomic::AtomicU64,
    adapters: PathBuf,
    runtime: Mutex<Option<PathBuf>>,
    state: Arc<Mutex<DocumentState>>,
    slots: Arc<(Mutex<usize>, Condvar)>,
}

struct Slot(Arc<(Mutex<usize>, Condvar)>);
impl Drop for Slot {
    fn drop(&mut self) {
        let (count, ready) = &*self.0;
        *count.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
        ready.notify_one();
    }
}

struct Stage(PathBuf);
impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn digest() -> String {
    let mut digest = Sha256::new();
    for (name, bytes) in [
        ("document_engine.py", DOCUMENT_ENGINE),
        ("image_engine.py", IMAGE_ENGINE),
        ("ocr_probe.py", OCR_PROBE),
    ] {
        digest.update(name.as_bytes());
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    }
    digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl DocumentRuntime {
    /// Installs the pinned adapters in their own digest directory, separate from the
    /// native adapter set whose path is part of every native preview hash.
    pub(crate) fn install(root: &Path) -> Result<Self> {
        let root_binding = document_root::DocumentRoot::open(root)?;
        Self::install_binding(root_binding)
    }

    fn install_binding(root_binding: Arc<document_root::DocumentRoot>) -> Result<Self> {
        let root = root_binding.path()?;
        private_dir(&root.join("home"))?;
        let adapters = root
            .join("adapters")
            .join(format!("documents-{}", digest()));
        private_dir(&adapters)?;
        for (name, bytes) in [
            ("document_engine.py", DOCUMENT_ENGINE),
            ("image_engine.py", IMAGE_ENGINE),
            ("ocr_probe.py", OCR_PROBE),
        ] {
            let path = adapters.join(name);
            match OpenOptions::new().create_new(true).write(true).open(&path) {
                Ok(mut file) => {
                    file.write_all(bytes)?;
                    file.sync_all()?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if fs::symlink_metadata(&path)?.is_symlink() || fs::read(&path)? != bytes {
                        return Err(invalid("Pinned document adapter changed"));
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        // Stages are private scratch space; a crash may leave some behind.
        let stages = root.join("document-stages");
        if stages.is_dir() && !fs::symlink_metadata(&stages)?.is_symlink() {
            let _ = fs::remove_dir_all(&stages);
        }
        private_dir(&stages)?;
        // Only our UUID-named transient Office profiles are eligible for crash cleanup.
        let profiles = root.join(".lo");
        private_dir(&profiles)?;
        for entry in fs::read_dir(&profiles)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if office_profile_name(&name) && entry.file_type()?.is_dir() {
                namespace(&entry.path())?;
                let _ = fs::remove_dir_all(entry.path());
            }
        }
        Ok(Self {
            root: root_binding,
            generation: std::sync::atomic::AtomicU64::new(0),
            adapters,
            runtime: Mutex::new(None),
            state: Arc::new(Mutex::new(DocumentState::default())),
            slots: Arc::new((Mutex::new(0), Condvar::new())),
        })
    }

    /// Returns cached readiness and starts a background probe when programs changed.
    pub(crate) fn availability(
        &self,
        programs: &BTreeMap<String, PathBuf>,
        environment: &BTreeMap<String, String>,
        root_owner: &Arc<fs::File>,
    ) -> (bool, bool, Option<String>, bool, String, String, String) {
        let Ok(root) = self.root.path() else {
            return (
                false,
                false,
                None,
                false,
                "probe_failed".into(),
                "probe_failed".into(),
                "probe_failed".into(),
            );
        };
        let Ok(physical_fits) = self.root.physical_path_fits() else {
            return (
                false,
                false,
                None,
                false,
                "probe_failed".into(),
                "probe_failed".into(),
                "probe_failed".into(),
            );
        };
        if !physical_fits || !document_working_directory_fits(root) {
            return (
                false,
                false,
                None,
                false,
                "data_path_too_long".into(),
                "data_path_too_long".into(),
                "data_path_too_long".into(),
            );
        }
        let runtime = self
            .runtime
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let key = ProbeKey {
            generation: self.generation.load(Ordering::Acquire),
            python: programs.get("python").cloned(),
            tectonic: programs.get("tectonic").cloned(),
            runtime: runtime.clone(),
            office: programs.get("libreoffice").cloned(),
        };
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.key.as_ref() != Some(&key) && !state.probing {
            state.probing = true;
            let shared = self.state.clone();
            let adapter = self.adapters.join("document_engine.py");
            let ocr_adapter = self.adapters.join("ocr_probe.py");
            let home = root.join("home");
            let toolbox = root.to_path_buf();
            let environment = environment.clone();
            let probed = key.clone();
            // Retain the original OS lease even after the last Toolbox Arc drops.
            // Startup cleanup must wait for this probe and its supervised children.
            let root_owner = Arc::clone(root_owner);
            let root_binding = Arc::clone(&self.root);
            std::thread::spawn(move || {
                let _root_owner = root_owner;
                let root_custody = root_binding;
                if root_custody.verify().is_err() {
                    let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                    *state = DocumentState {
                        key: Some(probed),
                        document_status: "probe_failed".into(),
                        latex_status: "probe_failed".into(),
                        office_status: "probe_failed".into(),
                        ..DocumentState::default()
                    };
                    return;
                }
                let (documents, version) = probed
                    .python
                    .as_ref()
                    .map(|python| probe(python, &adapter, &environment, &home))
                    .unwrap_or((false, None));
                let document_status = if documents {
                    "ready"
                } else if probed.python.is_none() {
                    "missing_runtime"
                } else {
                    "probe_failed"
                };
                let ocr_native = if documents {
                    probed.python.as_ref().and_then(|python| {
                        super::document_ocr::probe(python, &ocr_adapter, &environment, &home)
                    })
                } else {
                    None
                };
                let root_valid = root_custody.verify().is_ok();
                let latex_status = if !root_valid {
                    "probe_failed"
                } else if !documents {
                    document_status
                } else {
                    match probed.tectonic.as_ref().zip(probed.runtime.as_ref()) {
                        None => "missing_runtime",
                        Some(_) if !latex_working_directories_fit(&toolbox) => "data_path_too_long",
                        Some((tectonic, runtime)) if !tectonic_warm(tectonic, runtime) => {
                            "cache_incomplete"
                        }
                        Some((tectonic, runtime))
                            if !tex_confinement_available(tectonic, runtime, &toolbox) =>
                        {
                            "isolation_unavailable"
                        }
                        Some(_) => "ready",
                    }
                };
                let office_status = if root_custody.verify().is_err() {
                    "probe_failed"
                } else if !documents {
                    document_status
                } else {
                    match probed.office.as_ref() {
                        None => "missing_runtime",
                        Some(_) if !office_profile_path_fits(&toolbox) => "data_path_too_long",
                        Some(program) if !probe_office(program, &environment, &toolbox) => {
                            "probe_failed"
                        }
                        Some(_) => "ready",
                    }
                };
                let latex = latex_status == "ready";
                let office = office_status == "ready";
                let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                if root_custody.verify().is_err() {
                    *state = DocumentState {
                        key: Some(probed),
                        document_status: "probe_failed".into(),
                        latex_status: "probe_failed".into(),
                        office_status: "probe_failed".into(),
                        ..DocumentState::default()
                    };
                    return;
                }
                *state = DocumentState {
                    key: Some(probed),
                    probing: false,
                    documents,
                    latex,
                    version,
                    office,
                    document_status: document_status.into(),
                    latex_status: latex_status.into(),
                    office_status: office_status.into(),
                    ocr_native,
                };
            });
        }
        if state.key.as_ref() == Some(&key) {
            (
                state.documents,
                state.latex,
                state.version.clone(),
                state.office,
                state.document_status.clone(),
                state.latex_status.clone(),
                state.office_status.clone(),
            )
        } else {
            (
                false,
                false,
                None,
                false,
                "probing".into(),
                "probing".into(),
                "probing".into(),
            )
        }
    }

    /// Setup may repair wheels or the cache without changing an executable path.
    pub(crate) fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    fn ocr_native(
        &self,
        programs: &BTreeMap<String, PathBuf>,
    ) -> Option<super::document_ocr::NativeIdentity> {
        let state = self.state.lock().ok()?;
        let key = state.key.as_ref()?;
        if key.generation != self.generation.load(Ordering::Acquire)
            || key.python.as_ref() != programs.get("python")
        {
            return None;
        }
        state.ocr_native.clone()
    }

    fn acquire(&self, cancel: &AtomicBool) -> Result<Slot> {
        let (count, ready) = &*self.slots;
        let mut current = count.lock().map_err(|_| Error::Conflict)?;
        let started = std::time::Instant::now();
        while *current >= ENGINE_SLOTS {
            if cancel.load(Ordering::Acquire) || started.elapsed() > Duration::from_secs(60) {
                return Err(invalid("The document engine is busy; retry shortly"));
            }
            current = ready
                .wait_timeout(current, Duration::from_millis(200))
                .map_err(|_| Error::Conflict)?
                .0;
        }
        *current += 1;
        Ok(Slot(self.slots.clone()))
    }
}

/// A configured path alone does not qualify conversion. Use only a fixed trusted text.
fn probe_office(program: &Path, environment: &BTreeMap<String, String>, root: &Path) -> bool {
    let stage = Stage(root.join(format!("office-probe-{}", Uuid::new_v4())));
    if private_dir(&stage.0).is_err()
        || write_new(&stage.0.join("probe.txt"), b"Cowork conversion probe").is_err()
    {
        return false;
    }
    let Ok(profile) = office_profile(root) else {
        return false;
    };
    let launch = cowork_executor::TrustedLaunch {
        program: program.into(),
        arguments: vec![
            "--headless".into(),
            "--norestore".into(),
            "--nodefault".into(),
            "--nologo".into(),
            format!("-env:UserInstallation={}", file_uri(&profile.0)),
            "--convert-to".into(),
            "pdf".into(),
            "--outdir".into(),
            stage.0.to_string_lossy().into(),
            stage.0.join("probe.txt").to_string_lossy().into(),
        ],
        // First start can spend over 30 seconds building the private Office/font profile.
        // Qualify against the same bounded conversion window used by real document jobs.
        directory: stage.0.clone(),
        environment: environment.clone(),
        timeout: Duration::from_secs(90),
        capture_bytes_per_stream: 16 * 1024,
    };
    cowork_executor::supervise(&launch, &AtomicBool::new(false)).is_ok_and(|outcome| {
        outcome.termination == cowork_executor::Termination::Exited
            && outcome.exit_code == Some(0)
            && fs::read(stage.0.join("probe.pdf")).is_ok_and(|bytes| bytes.starts_with(b"%PDF-"))
    })
}

fn probe(
    python: &Path,
    adapter: &Path,
    environment: &BTreeMap<String, String>,
    home: &Path,
) -> (bool, Option<String>) {
    let mut environment = environment.clone();
    for key in ["HOME", "USERPROFILE"] {
        environment.insert(key.into(), home.to_string_lossy().into());
    }
    let launch = cowork_executor::TrustedLaunch {
        program: python.into(),
        arguments: vec![
            "-I".into(),
            "-X".into(),
            "utf8".into(),
            adapter.to_string_lossy().into(),
            "--probe".into(),
        ],
        directory: home.into(),
        environment,
        timeout: Duration::from_secs(60),
        capture_bytes_per_stream: 64 * 1024,
    };
    let Ok(outcome) = cowork_executor::supervise(&launch, &AtomicBool::new(false)) else {
        return (false, None);
    };
    let receipt: Value = serde_json::from_slice(&outcome.stdout.bytes).unwrap_or(Value::Null);
    let ok = outcome.exit_code == Some(0)
        && receipt["format"] == "cowork-document-probe/1"
        && receipt["ok"] == true;
    (
        ok,
        receipt["engine_version"]
            .as_str()
            .filter(|_| ok)
            .map(|v| v.chars().take(64).collect()),
    )
}

/// Tectonic's `--untrusted` mode does not confine TeX: `\input`/`\openin` read and
/// `\openout` writes any file the user can access, before any post-run audit could
/// object (see `.local/document-tools-20261018/TECTONIC-SPIKE.md`). LaTeX therefore runs
/// only inside OS confinement (`cowork_executor::supervise_confined`: a plain AppContainer
/// on Windows, Landlock plus seccomp on Linux; see TEX-ISOLATION-NOTES.md). This launches
/// the confined binary once; any failure keeps LaTeX unavailable.
pub(crate) fn tex_confinement_available(tectonic: &Path, runtime: &Path, root: &Path) -> bool {
    if !latex_working_directories_fit(root) || !cowork_executor::confinement_supported() {
        return false;
    }
    let probe = root
        .join("document-stages")
        .join(format!("tex-probe-{}", Uuid::new_v4().simple()));
    let available = private_dir(&probe).is_ok()
        && tex_confinement(tectonic, runtime, root, &[], &probe).is_ok_and(|confinement| {
            let launch = cowork_executor::TrustedLaunch {
                program: tectonic.into(),
                arguments: vec!["--version".into()],
                directory: probe.clone(),
                environment: tex_environment(runtime, &probe),
                timeout: Duration::from_secs(60),
                capture_bytes_per_stream: 64 * 1024,
            };
            cowork_executor::supervise_confined(&launch, &confinement, &AtomicBool::new(false))
                .is_ok_and(|outcome| {
                    outcome.exit_code == Some(0) && outcome.stdout.bytes.starts_with(b"Tectonic ")
                })
        });
    let _ = fs::remove_dir_all(&probe);
    available
}

/// Trees of one confined Tectonic run. Shared and read-only: the binary and the warm cache.
/// On Windows the binary's directory keeps a read grant for ALL APPLICATION PACKAGES, so it
/// must be the private copy tool setup installs as `<runtime>/tectonic-*/`. Read-only:
/// `sources`. Writable: only `output`.
fn tex_confinement(
    tectonic: &Path,
    runtime: &Path,
    root: &Path,
    sources: &[PathBuf],
    output: &Path,
) -> Result<cowork_executor::Confinement> {
    let program = if cfg!(windows) {
        let directory = tectonic
            .parent()
            .ok_or_else(|| invalid("Expected a Tectonic binary"))?;
        let private = fs::canonicalize(directory)?.parent()
            == Some(fs::canonicalize(runtime)?.as_path())
            && directory
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("tectonic-"));
        if !private {
            return Err(invalid(
                "LaTeX needs the private Tectonic copy from tool setup",
            ));
        }
        directory.to_path_buf()
    } else {
        tectonic.to_path_buf()
    };
    let state = root.join("tex-confinement");
    private_dir(&state)?;
    Ok(cowork_executor::Confinement {
        state,
        shared_read: vec![program, runtime.join("tectonic-cache")],
        read: sources.to_vec(),
        write: vec![output.to_path_buf()],
        memory_bytes: 2 << 30,
        processes: 1,
        cpu_millis: 2000,
    })
}

/// No HOME/USERPROFILE on Windows: Tectonic resolves its known folders in the AppContainer
/// profile, and a stage USERPROFILE without AppData breaks it (TECTONIC-SPIKE.md, 4).
fn tex_environment(runtime: &Path, temp: &Path) -> BTreeMap<String, String> {
    let temp = temp.to_string_lossy().into_owned();
    let mut environment = BTreeMap::from([(
        "TECTONIC_CACHE_DIR".to_owned(),
        runtime
            .join("tectonic-cache")
            .to_string_lossy()
            .into_owned(),
    )]);
    let keys: &[&str] = if cfg!(windows) {
        &["TEMP", "TMP"]
    } else {
        &["TMPDIR", "HOME"]
    };
    for key in keys {
        environment.insert((*key).into(), temp.clone());
    }
    environment
}

/// Compiles the stage-relative `main` with the pinned flags, confined to its source folder
/// (read) and `work/latex-out` (write). There is no unconfined fallback.
fn compile_tex(
    tectonic: &Path,
    runtime: &Path,
    root: &Path,
    stage: &Path,
    main: &str,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<(bool, String, String)> {
    let main_path = stage.join(relative(main)?);
    let directory = main_path
        .parent()
        .ok_or_else(|| invalid("Expected a LaTeX main file"))?
        .to_path_buf();
    if directory != stage.join("in").join("tex") && directory != stage.join("work").join("latex") {
        return Err(invalid("Expected a LaTeX main file in the stage"));
    }
    if !cowork_executor::working_directory_path_fits(&directory) {
        return Err(invalid("workflow_document_runtime_path_too_long"));
    }
    let output = stage.join("work/latex-out");
    let temp = output.join("tmp");
    private_dir(&temp)?;
    let confinement = tex_confinement(
        tectonic,
        runtime,
        root,
        std::slice::from_ref(&directory),
        &output,
    )?;
    let launch = cowork_executor::TrustedLaunch {
        program: tectonic.into(),
        arguments: vec![
            "-X".into(),
            "compile".into(),
            "--untrusted".into(),
            "--only-cached".into(),
            "--keep-logs".into(),
            "--outdir".into(),
            output.to_string_lossy().into(),
            main_path.to_string_lossy().into(),
        ],
        directory,
        environment: tex_environment(runtime, &temp),
        timeout,
        capture_bytes_per_stream: 256 * 1024,
    };
    let outcome = cowork_executor::supervise_confined(&launch, &confinement, cancel)
        .map_err(|_| invalid("LaTeX could not run in its isolated runtime"))?;
    if outcome.termination != cowork_executor::Termination::Exited {
        return Err(invalid("LaTeX compilation timed out"));
    }
    let rejected = scrub_tex_output(&output)?;
    let stem = main
        .rsplit('/')
        .next()
        .and_then(|n| n.rsplit_once('.'))
        .map(|(s, _)| s)
        .unwrap_or("main");
    let pdf = format!("work/latex-out/{stem}.pdf");
    let log = format!("work/latex-out/{stem}.log");
    let compiled = outcome.exit_code == Some(0) && stage.join(&pdf).is_file();
    if !compiled && !stage.join(&log).is_file() {
        // Tectonic reports some failures only on stderr; keep a bounded copy for diagnostics.
        let mut diagnostic = outcome.stderr.bytes.clone();
        diagnostic.extend_from_slice(b"\n");
        diagnostic.extend_from_slice(&outcome.stdout.bytes);
        diagnostic.extend_from_slice(
            format!("\nTectonic process exit status: {:?}.\n", outcome.exit_code).as_bytes(),
        );
        for reason in rejected.iter().take(4) {
            diagnostic
                .extend_from_slice(format!("Rejected isolated output: {reason}\n").as_bytes());
        }
        write_new(&stage.join(&log), &diagnostic)?;
    }
    Ok((compiled, pdf, log))
}

/// The confined run may leave links or special files in its output. Before the unconfined
/// engine reads it, only ordinary directories and single-link regular files remain.
fn scrub_tex_output(directory: &Path) -> Result<Vec<String>> {
    let mut rejected = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        #[cfg(windows)]
        let alias = {
            use std::os::windows::fs::MetadataExt;
            metadata.is_symlink() || metadata.file_attributes() & 0x400 != 0
        };
        #[cfg(not(windows))]
        let alias = metadata.is_symlink();
        if !alias && metadata.is_dir() {
            let nested = scrub_tex_output(&path)?;
            rejected.extend(
                nested
                    .into_iter()
                    .take(4usize.saturating_sub(rejected.len())),
            );
        } else {
            let checked = if alias {
                Err(invalid("alias or reparse point"))
            } else if !metadata.is_file() {
                Err(invalid("not a regular file"))
            } else {
                fs::File::open(&path)
                    .map_err(Error::from)
                    .and_then(|f| single_file(&f))
            };
            if let Err(reason) = checked {
                if rejected.len() < 4 {
                    let reason = match reason {
                        Error::Io(error) => {
                            format!("I/O {:?}, OS code {:?}", error.kind(), error.raw_os_error())
                        }
                        other => other.to_string(),
                    };
                    rejected.push(format!("{}: {reason}", entry.file_name().to_string_lossy()));
                }
                fs::remove_file(&path).or_else(|_| fs::remove_dir(&path))?;
            }
        }
    }
    Ok(rejected)
}

/// The Tectonic cache is warm only for exactly this binary, as recorded by tool setup.
fn tectonic_warm(tectonic: &Path, runtime: &Path) -> bool {
    let marker = runtime.join("tectonic-warm.json");
    let Ok(raw) = fs::read(&marker) else {
        return false;
    };
    let Ok(marker) = serde_json::from_slice::<Value>(&raw) else {
        return false;
    };
    let Ok(binary) = fs::read(tectonic) else {
        return false;
    };
    marker["format"] == "cowork-tectonic-warm/1"
        && marker["tectonic_sha256"].as_str() == Some(hash(&binary).as_str())
        && runtime.join("tectonic-cache").is_dir()
}

fn stage_name(index: usize, path: &str) -> Result<String> {
    let name = path
        .rsplit('/')
        .next()
        .filter(|n| !n.is_empty())
        .ok_or_else(|| invalid("Expected an input file name"))?;
    relative(name)?;
    Ok(format!("in/{index}/{name}"))
}

fn extension(path: &str) -> String {
    path.rsplit('/')
        .next()
        .and_then(|n| n.rsplit_once('.'))
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

fn office_diagnostic(stage: &Path, text: &str) -> String {
    let native = stage.to_string_lossy();
    let display = native.strip_prefix(r"\\?\").unwrap_or(&native);
    text.replace(native.as_ref(), "<stage>")
        .replace(&native.replace('\\', "/"), "<stage>")
        .replace(display, "<stage>")
        .replace(&display.replace('\\', "/"), "<stage>")
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(700)
        .collect()
}

fn office_profile_path(root: &Path, id: Uuid) -> PathBuf {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    // Preserve all 128 UUID bits while saving ten characters in Office's paths.
    root.join(".lo").join(URL_SAFE_NO_PAD.encode(id.as_bytes()))
}

fn office_profile_name(name: &str) -> bool {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    // Retain crash cleanup for the previous UUID hex directory format.
    if name.len() == 32 && Uuid::parse_str(name).is_ok() {
        return true;
    }
    if name.len() != 22 {
        return false;
    }
    URL_SAFE_NO_PAD
        .decode(name)
        .ok()
        .and_then(|bytes| Uuid::from_slice(&bytes).ok())
        .is_some_and(|id| {
            id.get_version() == Some(uuid::Version::Random)
                && id.get_variant() == uuid::Variant::RFC4122
                && URL_SAFE_NO_PAD.encode(id.as_bytes()) == name
        })
}

fn office_profile_path_fits(root: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        // Reserve room for Office's own registry, cache and extension subdirectories.
        office_profile_path(root, Uuid::nil())
            .as_os_str()
            .encode_wide()
            .count()
            <= 128
    }
    #[cfg(not(windows))]
    {
        let _ = root;
        true
    }
}

fn office_profile(root: &Path) -> Result<Stage> {
    // Calc can return exit 0 without an output when its internal profile paths
    // exceed Windows path limits. Keep the profile outside the deeper document stage.
    if !office_profile_path_fits(root) {
        return Err(invalid(
            "LibreOffice needs a shorter private application data directory on Windows",
        ));
    }
    let path = office_profile_path(root, Uuid::new_v4());
    private_dir(&path)?;
    let profile = Stage(path);
    write_new(&profile.0.join("user/registrymodifications.xcu"), br#"<?xml version="1.0" encoding="UTF-8"?>
<oor:items xmlns:oor="http://openoffice.org/2001/registry" xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="MacroSecurityLevel" oor:op="fuse"><value>3</value></prop></item>
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="DisableMacrosExecution" oor:op="fuse"><value>true</value></prop></item>
</oor:items>"#)?;
    Ok(profile)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        private_dir(parent)?;
    }
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    single_file(&file)?;
    Ok(())
}

fn read_stage_file(stage: &Path, name: &str, limit: u64) -> Result<Vec<u8>> {
    let relative = relative(name)?;
    let path = stage.join(&relative);
    namespace(&path)?;
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(invalid("Document engine output is missing or too large"));
    }
    let file = fs::File::open(&path)?;
    single_file(&file)?;
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(invalid("Document engine output is too large"));
    }
    Ok(bytes)
}

fn file_uri(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('\\', "/");
    let encoded: String = raw
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' | b':' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect();
    if encoded.starts_with('/') {
        format!("file://{encoded}")
    } else {
        format!("file:///{encoded}")
    }
}

impl Toolbox {
    /// Native startup uses the physical path budget before publishing availability.
    pub fn document_workspace_required(&self) -> Result<bool> {
        self.documents.root.physical_path_fits().map(|fits| !fits)
    }
    /// Path budget only; this does not grant custody or authorize a workspace.
    pub fn document_workspace_path_supported(directory: &Path) -> Result<bool> {
        document_root::DocumentRoot::open(directory)?.physical_path_fits()
    }

    /// Trusted native startup only, before runtime setup, probes or publication.
    /// The caller creates and owns private directory custody and must protect the workspace
    /// and its private parent in the file broker. Renderer/API paths cannot call this.
    pub fn configure_document_workspace(
        &mut self,
        directory: PathBuf,
        custody: Arc<dyn Send + Sync>,
    ) -> Result<()> {
        let state = self.documents.state.lock().map_err(|_| Error::Conflict)?;
        if state.key.is_some()
            || state.probing
            || self.documents.root.source.is_some()
            || self
                .documents
                .runtime
                .lock()
                .map_err(|_| Error::Conflict)?
                .is_some()
            || !self.imports.lock().map_err(|_| Error::Conflict)?.is_empty()
            || self.closing.load(Ordering::Acquire)
        {
            return Err(invalid(
                "Document workspace must be selected once during native startup",
            ));
        }
        drop(state);
        let binding = document_root::DocumentRoot::workspace(
            Arc::clone(&self.documents.root),
            &directory,
            custody,
        )?;
        self.documents = DocumentRuntime::install_binding(binding)?;
        Ok(())
    }

    /// Binds reviewed UI actions to the adapters, configured binaries and TeX marker.
    pub fn document_engine_revision(&self) -> String {
        let programs = self
            .programs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        self.document_revision_for(&programs)
    }

    fn document_revision_for(&self, programs: &BTreeMap<String, PathBuf>) -> String {
        use std::io::Read;
        let mut parts = vec![json!([
            "document-runtime/4",
            digest(),
            self.documents.root.revision()
        ])];
        for alias in ["python", "tectonic", "libreoffice"] {
            if let Some(path) = programs.get(alias) {
                let mut bytes = Vec::new();
                let checksum = match fs::File::open(path)
                    .and_then(|file| file.take(64 * 1024 * 1024 + 1).read_to_end(&mut bytes))
                {
                    Ok(_) if bytes.len() <= 64 * 1024 * 1024 => hash(&bytes),
                    _ => "unreadable".into(),
                };
                parts.push(json!([alias, path, checksum]));
            }
        }
        if let Some(runtime) = self
            .documents
            .runtime
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            let marker = fs::read(runtime.join("tectonic-warm.json")).unwrap_or_default();
            parts.push(json!([runtime, hash(&marker)]));
        }
        let runtime = self
            .documents
            .runtime
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        parts.push(super::document_ocr::identity(runtime.as_deref()));
        parts.push(
            self.documents
                .ocr_native(programs)
                .map(|native| native.revision())
                .unwrap_or_else(|| json!(["ocr-native/1", "unavailable"])),
        );
        hash(&serde_json::to_vec(&parts).expect("runtime identity contains serializable values"))
    }

    /// Daemon-provided private runtime directory holding the Tectonic cache and its warm marker.
    pub fn configure_document_runtime(&self, directory: PathBuf) -> Result<()> {
        if !directory.is_absolute() {
            return Err(invalid("Document runtime directory must be absolute"));
        }
        *self.documents.runtime.lock().map_err(|_| Error::Conflict)? = Some(directory);
        self.documents.invalidate();
        Ok(())
    }

    pub(crate) fn document_ocr_status(
        &self,
        programs: &BTreeMap<String, PathBuf>,
        documents_ready: bool,
    ) -> &'static str {
        if !documents_ready {
            return "document_runtime_unavailable";
        }
        let Some(native) = self.documents.ocr_native(programs) else {
            return "missing_ocr_runtime";
        };
        if !native.unchanged_metadata() {
            return "ocr_runtime_changed";
        }
        let runtime = self
            .documents
            .runtime
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if !super::document_ocr::models_ready(runtime.as_deref()) {
            return "ocr_language_pack_incomplete";
        }
        "ready"
    }

    fn document_environment(&self, stage: &Path) -> BTreeMap<String, String> {
        let mut environment = self.config.environment.clone();
        for key in ["HOME", "USERPROFILE"] {
            environment.insert(key.into(), stage.join("home").to_string_lossy().into());
        }
        for key in ["TEMP", "TMP", "TMPDIR"] {
            environment.insert(key.into(), stage.join("tmp").to_string_lossy().into());
        }
        environment
    }

    fn document_launch(
        &self,
        programs: &BTreeMap<String, PathBuf>,
        request: DocumentLaunch<'_>,
        cancel: &AtomicBool,
    ) -> Result<cowork_executor::Outcome> {
        let DocumentLaunch {
            program,
            arguments,
            directory,
            stage,
            timeout,
            capture,
        } = request;
        self.documents.root.verify()?;
        if !cowork_executor::working_directory_path_fits(directory) {
            return Err(invalid("workflow_document_runtime_path_too_long"));
        }
        let program = programs
            .get(program)
            .cloned()
            .ok_or_else(|| invalid("The document runtime is not installed"))?;
        let mut environment = self.document_environment(stage);
        if let Some(runtime) = self
            .documents
            .runtime
            .lock()
            .map_err(|_| Error::Conflict)?
            .as_ref()
        {
            environment.insert(
                "TECTONIC_CACHE_DIR".into(),
                runtime.join("tectonic-cache").to_string_lossy().into(),
            );
        }
        let launch = cowork_executor::TrustedLaunch {
            program,
            arguments,
            directory: directory.into(),
            environment,
            timeout,
            capture_bytes_per_stream: capture,
        };
        cowork_executor::supervise(&launch, cancel)
            .map_err(|_| invalid("The document engine could not be started"))
    }

    fn run_adapter(
        &self,
        programs: &BTreeMap<String, PathBuf>,
        adapter: &str,
        stage: &Path,
        request: &Value,
        deadline: std::time::Instant,
        cancel: &AtomicBool,
    ) -> Result<(i64, Value)> {
        let path = stage.join("request.json");
        let _ = fs::remove_file(&path);
        write_new(&path, &serde_json::to_vec(request)?)?;
        let outcome = self.document_launch(
            programs,
            DocumentLaunch {
                program: "python",
                arguments: vec![
                    "-I".into(),
                    "-X".into(),
                    "utf8".into(),
                    self.documents
                        .adapters
                        .join(adapter)
                        .to_string_lossy()
                        .into(),
                    stage.to_string_lossy().into(),
                ],
                directory: stage,
                stage,
                timeout: remaining(deadline)?,
                capture: MAX_RECEIPT_BYTES + 1,
            },
            cancel,
        )?;
        if outcome.termination != cowork_executor::Termination::Exited {
            return Err(invalid("The document engine timed out or was canceled"));
        }
        if outcome.stdout.truncated || outcome.stdout.bytes.len() > MAX_RECEIPT_BYTES {
            return Err(invalid("The document engine returned an oversized receipt"));
        }
        let receipt: Value = serde_json::from_slice(&outcome.stdout.bytes)
            .map_err(|_| invalid("The document engine failed"))?;
        let code = outcome.exit_code.unwrap_or(-1);
        let consistent = receipt["format"] == "cowork-document-receipt/1"
            && ((code == 0 && receipt["ok"] == true) || (code == 2 && receipt["ok"] == false));
        if !consistent {
            return Err(invalid("The document engine failed"));
        }
        Ok((code, receipt))
    }

    fn libreoffice_convert(
        &self,
        programs: &BTreeMap<String, PathBuf>,
        stage: &Path,
        input: &str,
        target: &str,
        deadline: std::time::Instant,
        cancel: &AtomicBool,
    ) -> Result<String> {
        let profile = office_profile(self.documents.root.path()?)?;
        let output = stage.join("work/office-out");
        private_dir(&output)?;
        let outcome = self.document_launch(
            programs,
            DocumentLaunch {
                program: "libreoffice",
                arguments: vec![
                    "--headless".into(),
                    "--norestore".into(),
                    "--nolockcheck".into(),
                    "--nodefault".into(),
                    "--nologo".into(),
                    format!("-env:UserInstallation={}", file_uri(&profile.0)),
                    "--convert-to".into(),
                    target.into(),
                    "--outdir".into(),
                    output.to_string_lossy().into(),
                    stage.join(input).to_string_lossy().into(),
                ],
                directory: stage,
                stage,
                timeout: remaining(deadline)?.min(Duration::from_secs(90)),
                capture: 64 * 1024,
            },
            cancel,
        )?;
        let stem = input
            .rsplit('/')
            .next()
            .and_then(|n| n.rsplit_once('.'))
            .map(|(s, _)| s)
            .unwrap_or("document");
        let converted = format!("work/office-out/{stem}.{target}");
        if outcome.termination != cowork_executor::Termination::Exited
            || outcome.exit_code != Some(0)
            || !stage.join(&converted).is_file()
        {
            let detail = office_diagnostic(
                &profile.0,
                &office_diagnostic(
                    stage,
                    &format!(
                        "{} {}",
                        String::from_utf8_lossy(&outcome.stdout.bytes),
                        String::from_utf8_lossy(&outcome.stderr.bytes)
                    ),
                ),
            );
            return Err(invalid(&format!(
                "LibreOffice could not convert this file (exit {:?}, {:?}). {}",
                outcome.exit_code,
                outcome.termination,
                detail.trim()
            )));
        }
        Ok(converted)
    }

    fn tectonic_compile(
        &self,
        programs: &BTreeMap<String, PathBuf>,
        stage: &Path,
        main: &str,
        deadline: std::time::Instant,
        cancel: &AtomicBool,
    ) -> Result<(bool, String, String)> {
        let tectonic = programs
            .get("tectonic")
            .cloned()
            .ok_or_else(|| invalid("LaTeX is not installed yet"))?;
        let runtime = self
            .documents
            .runtime
            .lock()
            .map_err(|_| Error::Conflict)?
            .clone()
            .ok_or_else(|| invalid("LaTeX is not installed yet"))?;
        compile_tex(
            &tectonic,
            &runtime,
            self.documents.root.path()?,
            stage,
            main,
            remaining(deadline)?.min(Duration::from_secs(120)),
            cancel,
        )
    }

    /// Runs one built-in document operation in a fresh private stage.
    pub fn execute_document(
        &self,
        job: DocumentJob,
        cancel: Arc<AtomicBool>,
    ) -> Result<DocumentOutcome> {
        if !self.documents.root.physical_path_fits()?
            || !document_working_directory_fits(self.documents.root.path()?)
        {
            return Err(invalid("workflow_document_runtime_path_too_long"));
        }
        let lifecycle = self.lifecycle.lock().map_err(|_| Error::Conflict)?;
        if self.closing.load(Ordering::Acquire) {
            return Err(invalid("Native tool service is stopping"));
        }
        let identifier = Uuid::new_v4().to_string();
        self.imports
            .lock()
            .map_err(|_| Error::Conflict)?
            .insert(identifier.clone(), cancel.clone());
        let _guard = ImportGuard {
            service: self,
            id: identifier.clone(),
        };
        drop(lifecycle);
        // Keep one immutable executable selection across every phase, including conversion.
        let programs = self.programs.lock().map_err(|_| Error::Conflict)?.clone();
        if job
            .engine_revision
            .as_ref()
            .is_some_and(|expected| *expected != self.document_revision_for(&programs))
        {
            return Err(invalid(
                "The reviewed document runtime changed; prepare a new action",
            ));
        }
        let availability = self.availability();
        // create_pdf falls back to the engine's ReportLab layout while LaTeX is unavailable.
        let latex = job.operation == "compile_latex";
        if !availability.documents_available || (latex && !availability.latex_available) {
            return Err(invalid(if latex {
                "LaTeX is not installed yet; the tool setup has not finished"
            } else {
                "The document runtime is not installed yet"
            }));
        }
        let native_ocr = if job.tool == "document_read" && job.operation == "ocr" {
            if !availability.ocr_available {
                return Err(invalid(
                    "OCR is unavailable; complete or repair the private OCR runtime and language pack",
                ));
            }
            Some(
                self.documents
                    .ocr_native(&programs)
                    .ok_or_else(|| invalid("The selected OCR runtime changed"))?
                    .lease()?,
            )
        } else {
            None
        };
        if job.inputs.len() > 16
            || job.inputs.iter().any(|i| {
                i.bytes.len() as u64 > MAX_DOCUMENT_INPUT_BYTES || hash(&i.bytes) != i.sha256
            })
        {
            return Err(invalid("Document inputs changed or exceed 32 MiB"));
        }
        let _slot = self.documents.acquire(&cancel)?;
        let deadline = std::time::Instant::now() + job.timeout.min(Duration::from_secs(300));
        let stage_path = self
            .documents
            .root
            .path()?
            .join("document-stages")
            .join(&identifier);
        private_dir(&stage_path)?;
        let stage = Stage(stage_path);
        let root = stage.0.clone();
        for directory in ["in", "work/previews", "out", "tmp", "home"] {
            private_dir(&root.join(directory))?;
        }
        let names = if job.operation == "compile_latex" {
            let main = &job
                .inputs
                .first()
                .ok_or_else(|| invalid("Expected a LaTeX main file"))?
                .path;
            let base = main
                .rsplit_once('/')
                .map(|(d, _)| format!("{d}/"))
                .unwrap_or_default();
            let mut names = Vec::new();
            for input in &job.inputs {
                let inner = input.path.strip_prefix(&base).ok_or_else(|| {
                    invalid("LaTeX assets must be inside the folder of the main .tex file")
                })?;
                relative(inner)?;
                let name = format!("in/tex/{inner}");
                if names.contains(&name) {
                    return Err(invalid("A LaTeX asset is listed twice"));
                }
                names.push(name);
            }
            names
        } else {
            job.inputs
                .iter()
                .enumerate()
                .map(|(i, input)| stage_name(i, &input.path))
                .collect::<Result<Vec<_>>>()?
        };
        for (input, name) in job.inputs.iter().zip(&names) {
            write_new(&root.join(relative(name)?), &input.bytes)?;
        }
        if job.tool == "document_read" && job.operation == "ocr" {
            let runtime = self
                .documents
                .runtime
                .lock()
                .map_err(|_| Error::Conflict)?
                .clone();
            super::document_ocr::stage(runtime.as_deref(), &root)?;
        }
        let output = match (&job.output_name, job.output_directory) {
            (Some(name), _) => {
                relative(name)?;
                if name.contains('/') {
                    return Err(invalid("Expected an output name"));
                }
                Some(format!("out/{name}"))
            }
            (None, _) => None,
        };
        let inputs: Vec<Value> = job
            .inputs
            .iter()
            .zip(&names)
            .map(|(input, name)| json!({"name":name,"path":input.path,"sha256":input.sha256,"bytes":input.bytes.len()}))
            .collect();
        let request = |phase: Option<&str>, options: Value| {
            json!({
                "format":"cowork-document-request/1","tool":job.tool,"operation":job.operation,"phase":phase,
                "inputs":inputs,"output":output.as_ref().map(|name|json!({"name":name})),"spec":job.spec,"options":options,
                "limits":{"max_output_bytes":MAX_OUTPUT_BYTES,"max_outputs":MAX_OUTPUTS,"max_result_bytes":MAX_RESULT_BYTES,
                    "max_pages_read":500,"max_pages_render":200,"max_preview_images":MAX_PREVIEWS,"preview_max_edge":1600,
                    "preview_max_bytes":MAX_PREVIEW_BYTES,"max_image_pixels":50_000_000u64}
            })
        };
        let merge = |extra: Value| {
            let mut options = job.options.clone();
            if let (Some(target), Some(source)) = (options.as_object_mut(), extra.as_object()) {
                target.extend(source.clone());
            }
            options
        };
        let adapter = if job.tool == "image_edit" {
            "image_engine.py"
        } else {
            "document_engine.py"
        };
        let (_, receipt) = match (job.tool.as_str(), job.operation.as_str()) {
            ("document_read", "preview")
                if OFFICE_EXTENSIONS.contains(
                    &extension(
                        job.inputs
                            .first()
                            .map(|i| i.path.as_str())
                            .unwrap_or_default(),
                    )
                    .as_str(),
                ) =>
            {
                if !availability.office_convert_available {
                    return Err(invalid(
                        "Office previews need LibreOffice, which is not installed",
                    ));
                }
                let converted = self
                    .libreoffice_convert(&programs, &root, &names[0], "pdf", deadline, &cancel)?;
                self.run_adapter(
                    &programs,
                    adapter,
                    &root,
                    &request(
                        Some("render_pdf"),
                        merge(json!({"render_source":converted})),
                    ),
                    deadline,
                    &cancel,
                )?
            }
            ("document_write", "convert") => {
                if !availability.office_convert_available {
                    return Err(invalid(
                        "Conversion needs LibreOffice, which is not installed",
                    ));
                }
                let target = extension(job.output_name.as_deref().unwrap_or_default());
                let input = if job.spec["recalculate"] == true {
                    let (code, prepared) = self.run_adapter(
                        &programs,
                        adapter,
                        &root,
                        &request(Some("recalculate_prepare"), job.options.clone()),
                        deadline,
                        &cancel,
                    )?;
                    if code != 0 {
                        return self.finish(&root, &job, output.as_deref(), prepared);
                    }
                    let input = prepared["result"]["input"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned();
                    relative(&input)?;
                    if !input.starts_with("work/recalculate/") || !input.ends_with(".xlsx") {
                        return Err(invalid(
                            "The document engine failed to prepare recalculation",
                        ));
                    }
                    input
                } else {
                    names[0].clone()
                };
                let converted =
                    self.libreoffice_convert(&programs, &root, &input, &target, deadline, &cancel)?;
                self.run_adapter(
                    &programs,
                    adapter,
                    &root,
                    &request(
                        Some("office_finalize"),
                        merge(json!({"converted":converted})),
                    ),
                    deadline,
                    &cancel,
                )?
            }
            ("document_write", "create_pdf") if !availability.latex_available => self.run_adapter(
                &programs,
                adapter,
                &root,
                &request(None, merge(json!({"latex_available":false}))),
                deadline,
                &cancel,
            )?,
            ("document_write", "compile_latex" | "create_pdf") => {
                let main = if job.operation == "create_pdf" {
                    let (code, prepared) = self.run_adapter(
                        &programs,
                        adapter,
                        &root,
                        &request(Some("latex_prepare"), job.options.clone()),
                        deadline,
                        &cancel,
                    )?;
                    if code != 0 {
                        return self.finish(&root, &job, output.as_deref(), prepared);
                    }
                    let main = prepared["result"]["main"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned();
                    if !main.starts_with("work/latex/") || !main.ends_with(".tex") {
                        return Err(invalid("The document engine failed to prepare LaTeX"));
                    }
                    main
                } else {
                    names[0].clone()
                };
                let (compiled, pdf, log) =
                    self.tectonic_compile(&programs, &root, &main, deadline, &cancel)?;
                self.run_adapter(
                    &programs,
                    adapter,
                    &root,
                    &request(
                        Some("latex_finalize"),
                        merge(json!({"compile_failed":!compiled,"pdf":pdf,"log":log,"main":main})),
                    ),
                    deadline,
                    &cancel,
                )?
            }
            _ => self.run_adapter(
                &programs,
                adapter,
                &root,
                &request(None, job.options.clone()),
                deadline,
                &cancel,
            )?,
        };
        if let Some(native) = &native_ocr {
            native.assert_current()?;
        }
        self.finish(&root, &job, output.as_deref(), receipt)
    }

    fn finish(
        &self,
        root: &Path,
        job: &DocumentJob,
        output: Option<&str>,
        receipt: Value,
    ) -> Result<DocumentOutcome> {
        let ok = receipt["ok"] == true;
        let engine_version = receipt["engine_version"]
            .as_str()
            .unwrap_or("unknown")
            .chars()
            .take(64)
            .collect();
        let mut outputs = Vec::new();
        let mut previews = Vec::new();
        if ok {
            let listed = receipt["outputs"].as_array().cloned().unwrap_or_default();
            if listed.len() > MAX_OUTPUTS || job.output_name.is_some() == listed.is_empty() {
                return Err(invalid("The document engine returned unexpected outputs"));
            }
            let mut total = 0u64;
            for item in &listed {
                let name = item["name"].as_str().unwrap_or_default();
                let target = output
                    .ok_or_else(|| invalid("The document engine returned unexpected outputs"))?;
                let path = if job.output_directory {
                    name.strip_prefix(&format!("{target}/"))
                        .ok_or_else(|| invalid("The document engine wrote outside its output"))?
                        .to_owned()
                } else if name == target && listed.len() == 1 {
                    String::new()
                } else {
                    return Err(invalid("The document engine wrote outside its output"));
                };
                let bytes = read_stage_file(root, name, MAX_OUTPUT_BYTES)?;
                total += bytes.len() as u64;
                let sha256 = hash(&bytes);
                if total > MAX_OUTPUT_BYTES || item["sha256"].as_str() != Some(sha256.as_str()) {
                    return Err(invalid("Document outputs exceed 64 MiB or changed"));
                }
                let media_type = item["media_type"]
                    .as_str()
                    .unwrap_or("application/octet-stream")
                    .chars()
                    .take(200)
                    .collect();
                outputs.push(DocumentFile {
                    path,
                    bytes,
                    sha256,
                    media_type,
                });
            }
        }
        for item in receipt["previews"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .take(MAX_PREVIEWS)
        {
            let name = item["name"].as_str().unwrap_or_default();
            if !name.starts_with("work/previews/") {
                continue;
            }
            let bytes = read_stage_file(root, name, MAX_PREVIEW_BYTES)?;
            let sha256 = hash(&bytes);
            if !bytes.starts_with(b"\x89PNG\r\n\x1a\n")
                || item["sha256"].as_str() != Some(sha256.as_str())
            {
                return Err(invalid("The document engine returned an invalid preview"));
            }
            previews.push(DocumentPreview {
                page: item["page"].as_u64().unwrap_or(1).min(100_000) as u32,
                width: item["width"].as_u64().unwrap_or(0).min(100_000) as u32,
                height: item["height"].as_u64().unwrap_or(0).min(100_000) as u32,
                sha256,
                bytes,
            });
        }
        let mut model = json!({});
        for key in ["result", "diagnostics", "qa", "error"] {
            if !receipt[key].is_null() {
                model[key] = receipt[key].clone();
            }
        }
        if serde_json::to_vec(&model)?.len() > MAX_RESULT_BYTES * 2 {
            return Err(invalid("The document engine returned an oversized result"));
        }
        Ok(DocumentOutcome {
            ok,
            receipt_sha256: hash(&serde_json::to_vec(&receipt)?),
            receipt: model,
            engine_version,
            outputs,
            previews,
        })
    }
}

fn remaining(deadline: std::time::Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(std::time::Instant::now())
        .filter(|d| *d >= Duration::from_secs(1))
        .ok_or_else(|| invalid("The document operation ran out of time"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn office_diagnostics_redact_stage_paths_and_bound_untrusted_output() {
        let stage = Path::new(r"C:\private\office-stage");
        let text = format!(
            "failed C:\\private\\office-stage\\in.xlsx\nfile:///C:/private/office-stage/out.pdf {}",
            "x".repeat(1000)
        );
        let diagnostic = office_diagnostic(stage, &text);
        assert!(!diagnostic.contains("private"));
        assert!(!diagnostic.contains('\n'));
        assert!(diagnostic.contains("<stage>"));
        assert_eq!(diagnostic.chars().count(), 700);
    }

    #[test]
    fn file_uris_are_absolute_and_encoded() {
        assert_eq!(
            file_uri(Path::new("C:\\Users\\A B\\p")),
            "file:///C:/Users/A%20B/p"
        );
        assert_eq!(file_uri(Path::new("/tmp/x y")), "file:///tmp/x%20y");
    }

    #[test]
    fn office_profile_names_keep_full_random_identity_and_legacy_cleanup() {
        let id = Uuid::parse_str("ffffffff-ffff-4fff-bfff-ffffffffffff").unwrap();
        let path = office_profile_path(Path::new("root"), id);
        let name = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(name.len(), 22);
        assert!(office_profile_name(name));
        assert!(office_profile_name(&id.simple().to_string()));
        for foreign in [
            "",
            "not-a-profile",
            "../foreign",
            "AAAAAAAAAAAAAAAAAAAAAA",
            "______________________",
        ] {
            assert!(!office_profile_name(foreign), "{foreign}");
        }
        assert_ne!(office_profile_path(Path::new("root"), Uuid::new_v4()), path);
    }

    #[cfg(windows)]
    #[test]
    fn office_profile_fits_the_retained_profile_without_raising_the_limit() {
        let root = PathBuf::from(format!("C:\\{}", "x".repeat(93)));
        assert!(
            root.join(".lo")
                .join(Uuid::new_v4().simple().to_string())
                .as_os_str()
                .len()
                > 128
        );
        assert!(office_profile_path_fits(&root));
        assert!(!office_profile_path_fits(
            &root.join("a-much-longer-private-data-root")
        ));
    }

    #[cfg(windows)]
    #[test]
    fn long_document_stage_is_unavailable_before_probe_or_job_admission() {
        use std::os::windows::ffi::OsStrExt;
        let scratch = Scratch::new();
        let prefix = scratch.0.as_os_str().encode_wide().count() + 1;
        let root = scratch.0.join("x".repeat(206 - prefix));
        let mut toolbox = Toolbox::open(Config {
            root: root.clone(),
            programs: BTreeMap::new(),
            environment: BTreeMap::new(),
            github_token: None,
        })
        .unwrap();
        // Explicitly exercise the OS-no-short-name fallback without changing the volume.
        Arc::get_mut(&mut toolbox).unwrap().documents.root =
            document_root::DocumentRoot::without_short_spelling(&root).unwrap();
        let available = toolbox.availability();
        assert_eq!(available.document_status, "data_path_too_long");
        assert!(
            !available.documents_available
                && !available.latex_available
                && !available.ocr_available
                && !available.office_convert_available
        );
        assert!(!toolbox.documents.state.lock().unwrap().probing);
        let error = toolbox
            .execute_document(
                DocumentJob {
                    engine_revision: None,
                    tool: "document_read".into(),
                    operation: "text".into(),
                    inputs: vec![],
                    output_name: None,
                    output_directory: false,
                    spec: Value::Null,
                    options: Value::Null,
                    timeout: Duration::from_secs(10),
                },
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap_err();
        assert!(
            matches!(error, Error::Invalid(message) if message == "workflow_document_runtime_path_too_long")
        );
        assert!(toolbox.imports.lock().unwrap().is_empty());
        assert!(
            fs::read_dir(root.join("document-stages"))
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[cfg(windows)]
    #[test]
    fn office_probe_converts_with_a_long_private_data_root() {
        let Some(program) = std::env::var_os("COWORK_TEST_LIBREOFFICE") else {
            assert!(
                std::env::var_os("COWORK_REQUIRE_OFFICE_TESTS").is_none(),
                "COWORK_TEST_LIBREOFFICE is required"
            );
            return;
        };
        use std::os::windows::ffi::OsStrExt;
        let scratch = Scratch::new();
        let length = scratch.0.as_os_str().encode_wide().count();
        assert!(
            length < 95,
            "This qualification needs a shorter private TEMP path"
        );
        let root = scratch.0.join("x".repeat(95 - length));
        private_dir(&root).unwrap();
        assert!(
            root.join(".lo")
                .join(Uuid::new_v4().simple().to_string())
                .as_os_str()
                .encode_wide()
                .count()
                > 128
        );
        let environment = ["SystemRoot", "PATH", "TEMP", "TMP"]
            .into_iter()
            .filter_map(|key| std::env::var(key).ok().map(|value| (key.into(), value)))
            .collect();
        assert!(probe_office(&PathBuf::from(program), &environment, &root));
        assert_eq!(
            fs::read_dir(root.join(".lo")).unwrap().count(),
            0,
            "transient profile must be removed"
        );
    }

    #[test]
    fn input_stage_names_stay_inside_the_stage() {
        assert_eq!(stage_name(2, "reports/q3.docx").unwrap(), "in/2/q3.docx");
        assert!(stage_name(0, "").is_err());
        assert_eq!(extension("a/b.DOCX"), "docx");
    }

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("cowork-tex-{}", Uuid::new_v4().simple()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn tex_output_keeps_only_ordinary_files() {
        let scratch = Scratch::new();
        let output = scratch.0.join("latex-out");
        fs::create_dir_all(output.join("sub")).unwrap();
        let outside = scratch.0.join("outside.txt");
        fs::write(&outside, b"owner data").unwrap();
        fs::write(output.join("main.pdf"), b"%PDF").unwrap();
        fs::write(output.join("sub/main.log"), b"log").unwrap();
        fs::hard_link(&outside, output.join("hard.pdf")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, output.join("sub/link.log")).unwrap();
        scrub_tex_output(&output).unwrap();
        assert!(output.join("main.pdf").is_file() && output.join("sub/main.log").is_file());
        assert!(
            !output.join("hard.pdf").exists()
                && fs::symlink_metadata(output.join("sub/link.log")).is_err()
        );
        assert_eq!(fs::read(&outside).unwrap(), b"owner data");
    }

    #[cfg(windows)]
    #[test]
    fn windows_latex_needs_the_private_tectonic_copy() {
        let scratch = Scratch::new();
        let runtime = scratch.0.join("tool-runtime");
        for directory in ["tectonic-0.17.0-x", "tectonic-cache", "elsewhere", "out"] {
            fs::create_dir_all(runtime.join(directory)).unwrap();
        }
        let output = runtime.join("out");
        assert!(
            tex_confinement(
                &runtime.join("tectonic-0.17.0-x/tectonic.exe"),
                &runtime,
                &scratch.0,
                &[],
                &output
            )
            .is_ok()
        );
        for foreign in [
            runtime.join("elsewhere/tectonic.exe"),
            runtime.join("tectonic.exe"),
            scratch.0.join("tectonic-1/tectonic.exe"),
        ] {
            let _ = fs::create_dir_all(foreign.parent().unwrap());
            assert!(
                tex_confinement(&foreign, &runtime, &scratch.0, &[], &output).is_err(),
                "{foreign:?}"
            );
        }
    }

    /// Uses the scratch runtime of crates/executor/tests/tex_confinement.rs:
    /// COWORK_TEST_TECTONIC (inside `<runtime>/tectonic-*/` on Windows) and
    /// COWORK_TEST_TECTONIC_CACHE (`<runtime>/tectonic-cache`).
    #[test]
    fn latex_compiles_only_inside_its_confinement() {
        let (Some(tectonic), Some(cache)) = (
            std::env::var_os("COWORK_TEST_TECTONIC"),
            std::env::var_os("COWORK_TEST_TECTONIC_CACHE"),
        ) else {
            assert!(
                std::env::var_os("COWORK_REQUIRE_TEX_TESTS").is_none(),
                "COWORK_TEST_TECTONIC and COWORK_TEST_TECTONIC_CACHE are required"
            );
            return;
        };
        let (tectonic, cache) = (PathBuf::from(tectonic), PathBuf::from(cache));
        assert_eq!(cache.file_name().unwrap(), "tectonic-cache");
        let runtime = cache.parent().unwrap();
        let scratch = Scratch::new();
        let root = scratch.0.join("toolbox");
        private_dir(&root.join("document-stages")).unwrap();
        assert!(tex_confinement_available(&tectonic, runtime, &root));
        let secret = scratch.0.join("secret.txt");
        fs::write(&secret, "SECRETMARKER").unwrap();
        let target = scratch.0.join("target.txt");
        fs::write(&target, b"ORIGINAL").unwrap();
        let path = |p: &Path| p.to_string_lossy().replace('\\', "/");
        for (name, body, expect) in [
            ("legit", "\\input{chapters/one}".to_owned(), true),
            ("read", format!("\\input{{{}}}", path(&secret)), false),
            (
                "write",
                format!(
                    "\\newwrite\\w\\immediate\\openout\\w={}\\relax\\immediate\\write\\w{{x}}\\immediate\\closeout\\w",
                    path(&target)
                ),
                false,
            ),
        ] {
            let stage = root.join("document-stages").join(name);
            private_dir(&stage.join("in/tex/chapters")).unwrap();
            fs::write(stage.join("in/tex/chapters/one.tex"), "Chapter one.").unwrap();
            fs::write(
                stage.join("in/tex/main.tex"),
                format!("\\documentclass{{article}}\\begin{{document}}x {body}\\end{{document}}"),
            )
            .unwrap();
            let (compiled, pdf, log) = compile_tex(
                &tectonic,
                runtime,
                &root,
                &stage,
                "in/tex/main.tex",
                Duration::from_secs(120),
                &AtomicBool::new(false),
            )
            .unwrap();
            let log = fs::read_to_string(stage.join(log)).unwrap_or_default();
            assert_eq!(compiled, expect, "{name}: {log}");
            assert!(!expect || stage.join(pdf).is_file(), "{name}");
            assert!(!log.contains("SECRETMARKER"), "{name}");
        }
        assert_eq!(fs::read(&target).unwrap(), b"ORIGINAL");
    }
}
