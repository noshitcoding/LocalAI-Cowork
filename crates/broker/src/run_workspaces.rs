//! Private copies with a per-execution namespace. Roles retain logical folder
//! IDs while the broker resolves only the execution's private working copies.
use super::*;
use cowork_domain::execution_inputs::CapturedExecutionInput;
use workspace_receive::{NativeObject, directory_identity};

pub use cowork_domain::run_workspaces::RunWorkspaceIntent;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct NativeCopy {
    alias: String,
    identity: NativeObject,
}

/// Local custody, never included in the API schema or transmitted to runners.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RunWorkspaceRecord {
    pub intent: RunWorkspaceIntent,
    container_identity: NativeObject,
    copies: Vec<NativeCopy>,
}

pub struct PreparedRunWorkspace {
    pub record: RunWorkspaceRecord,
    _lock: ResourceLock,
    _source_locks: Vec<ResourceLock>,
}

fn input_reference(capture: &CapturedExecutionInput) -> ExecutionInputRef {
    ExecutionInputRef {
        snapshot_id: capture.snapshot.id.clone(),
        sha256: capture.sha256.clone(),
        manifest_sha256: capture.snapshot.manifest.sha256.clone(),
    }
}

// Private installation witness, not a remotely portable runner identity. A
// copied app directory (including its nonce) must not become the old controller.
#[cfg(windows)]
fn host_identity() -> Result<String> {
    use windows_sys::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY, RegGetValueW,
    };
    let key: Vec<u16> = "SOFTWARE\\Microsoft\\Cryptography\0"
        .encode_utf16()
        .collect();
    let name: Vec<u16> = "MachineGuid\0".encode_utf16().collect();
    let mut value = [0u16; 128];
    let mut length = std::mem::size_of_val(&value) as u32;
    // Fixed, bounded buffer; the registry API supplies the UTF-16 terminator.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            std::ptr::null_mut(),
            value.as_mut_ptr().cast(),
            &mut length,
        )
    };
    if status != 0
        || length < 2
        || !length.is_multiple_of(2)
        || length as usize > std::mem::size_of_val(&value)
    {
        return Err(Error::Forbidden);
    }
    let units = length as usize / 2;
    if value[units - 1] != 0 || value[..units - 1].contains(&0) {
        return Err(Error::Forbidden);
    }
    let text = String::from_utf16(&value[..units - 1]).map_err(|_| Error::Forbidden)?;
    let id = uuid::Uuid::parse_str(&text).map_err(|_| Error::Forbidden)?;
    if id.is_nil() {
        return Err(Error::Forbidden);
    }
    Ok(id.to_string())
}

#[cfg(target_os = "linux")]
fn host_identity() -> Result<String> {
    let file = std::fs::File::open("/etc/machine-id").map_err(storage_error)?;
    let mut text = String::new();
    file.take(128)
        .read_to_string(&mut text)
        .map_err(storage_error)?;
    let raw = text.trim();
    if raw.len() != 32 || !raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Forbidden);
    }
    let id = uuid::Uuid::parse_str(raw).map_err(|_| Error::Forbidden)?;
    if id.is_nil() {
        return Err(Error::Forbidden);
    }
    Ok(id.to_string())
}

#[cfg(not(any(windows, target_os = "linux")))]
fn host_identity() -> Result<String> {
    Err(Error::Forbidden)
}

fn create_private_directory(parent: &Dir, name: &str) -> std::io::Result<()> {
    let builder = cap_std::fs::DirBuilder::new();
    #[cfg(unix)]
    let builder = {
        use cap_std::fs::DirBuilderExt;
        let mut builder = builder;
        builder.mode(0o700);
        builder
    };
    parent.create_dir_with(name, &builder)
}

fn parent(broker: &FileBroker) -> Result<Dir> {
    match create_private_directory(&broker.registry_state, "run-workspaces") {
        Ok(()) => {
            #[cfg(unix)]
            sync_directory(&broker.registry_state)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(storage_error(e)),
    }
    open_directory(&broker.registry_state, Path::new("run-workspaces"))
}

fn read_record(container: &Dir) -> Result<RunWorkspaceRecord> {
    let bytes = read_regular_limit(container, "workspace.json", 128 * 1024)?
        .ok_or(Error::UncertainEffect)?;
    let record: RunWorkspaceRecord = serde_json::from_slice(&bytes).map_err(storage_error)?;
    record.intent.validate()?;
    Ok(record)
}

fn initial_entries(
    intent: &RunWorkspaceIntent,
    capture: &CapturedExecutionInput,
) -> Vec<WorkspaceEntry> {
    let mut entries: Vec<_> = capture
        .snapshot
        .manifest
        .directories
        .iter()
        .map(|path| WorkspaceEntry {
            path: path.clone(),
            sha256: None,
            size: 0,
        })
        .chain(
            capture
                .snapshot
                .manifest
                .files
                .iter()
                .map(|f| WorkspaceEntry {
                    path: f.path.clone(),
                    sha256: Some(f.sha256.clone()),
                    size: f.size,
                }),
        )
        .collect();
    if intent.copies.iter().any(|c| c.alias == "output") {
        entries.push(WorkspaceEntry {
            path: "output".into(),
            sha256: None,
            size: 0,
        });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries
}

impl FileBroker {
    /// Private witness: a native object plus its expected canonical anchor.
    /// Callers store only its private digest; it is never a public target ID.
    pub fn local_controller_origin(&self) -> Result<String> {
        // Concurrent reviews/dispatches may briefly share the marker's private
        // initialization fence. Waiting here never retries a workload effect.
        let until = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let _lock = loop {
            match self.lock_named("broker-controller-origin", true) {
                Err(Error::WorkspaceBusy) if std::time::Instant::now() < until => {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                result => break result?,
            }
        };
        let anchor = self.protected_paths.first().ok_or(Error::Forbidden)?;
        if &comparison_path(anchor)? != anchor {
            return Err(Error::Forbidden);
        }
        let current = Dir::open_ambient_dir(anchor, ambient_authority()).map_err(storage_error)?;
        let native = directory_identity(&self.registry_state)?;
        if directory_identity(&current)? != native {
            return Err(Error::Forbidden);
        }
        let marker = "broker-controller-origin.json";
        let nonce = if let Some(bytes) = read_regular_limit(&self.registry_state, marker, 256)? {
            let nonce: String = serde_json::from_slice(&bytes).map_err(storage_error)?;
            uuid::Uuid::parse_str(&nonce).map_err(|_| Error::Forbidden)?;
            nonce
        } else {
            let nonce = uuid::Uuid::new_v4().to_string();
            let mut options = OpenOptions::new();
            options
                .write(true)
                .create_new(true)
                .follow(FollowSymlinks::No);
            #[cfg(unix)]
            {
                use cap_std::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = self
                .registry_state
                .open_with(marker, &options)
                .map_err(storage_error)?;
            file.write_all(&serde_json::to_vec(&nonce).map_err(storage_error)?)
                .map_err(storage_error)?;
            file.sync_all().map_err(storage_error)?;
            #[cfg(unix)]
            sync_directory(&self.registry_state)?;
            nonce
        };
        let mut marker_options = OpenOptions::new();
        marker_options.read(true).follow(FollowSymlinks::No);
        let marker_file = self
            .registry_state
            .open_with(marker, &marker_options)
            .map_err(storage_error)?
            .into_std();
        let marker_identity = workspace_receive::file_identity(&marker_file)?;
        let host = host_identity()?;
        Ok(canonical_hash(
            &json!({"domain":"native-broker-controller-origin/1","host":host,"anchor":anchor,"native":native,"marker":marker_identity,"nonce":nonce}),
        ))
    }
    /// The controller first checks confirmation and retains the exact private
    /// intent. This method materializes retained bytes and starts no process.
    pub fn prepare_run_workspace(
        &self,
        intent: &RunWorkspaceIntent,
        capture: &CapturedExecutionInput,
    ) -> Result<PreparedRunWorkspace> {
        intent.validate()?;
        capture.validate()?;
        workspaces::validate_entries(&initial_entries(intent, capture))?;
        if intent.binding.input != input_reference(capture)
            || intent.project_id != capture.snapshot.project_id
            || intent.order_id != capture.snapshot.order_id
        {
            return Err(Error::Forbidden);
        }
        let mut source_roots = Vec::new();
        for (index, origin) in capture.origins.iter().enumerate() {
            let copy = intent
                .copies
                .iter()
                .find(|c| c.alias == capture.snapshot.sources[index].alias)
                .ok_or(Error::Forbidden)?;
            if copy.grant.id != origin.id
                || copy.grant.name != origin.name
                || copy.grant.writable && !origin.writable
            {
                return Err(Error::Forbidden);
            }
            let current = self.lookup_root(&origin.id, false)?;
            if current.grant != *origin {
                return Err(Error::Forbidden);
            }
            source_roots.push(current);
        }
        if intent.copies.iter().filter(|c| c.alias != "output").count() != capture.origins.len() {
            return Err(Error::Forbidden);
        }
        source_roots.sort_by(|a, b| a.lock_name.cmp(&b.lock_name));
        let mut source_locks = Vec::new();
        for root in &source_roots {
            source_locks.push(self.lock(root, false)?);
        }
        let lock = self.lock_named(&format!("run-workspace-create-{}", intent.binding.id), true)?;
        let parent = parent(self)?;
        let existing = match parent.symlink_metadata(&intent.binding.id) {
            Ok(meta) if meta.is_dir() && !meta.is_symlink() => true,
            Ok(_) => return Err(Error::Forbidden),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(storage_error(e)),
        };
        if !existing {
            let stage_name = format!(".cowork-{}", uuid::Uuid::new_v4());
            create_private_directory(&parent, &stage_name).map_err(storage_error)?;
            let stage = open_directory(&parent, Path::new(&stage_name))?;
            create_private_directory(&stage, "files").map_err(storage_error)?;
            let files = open_directory(&stage, Path::new("files"))?;
            workspaces::materialize_entries(
                &self.versions,
                &files,
                &initial_entries(intent, capture),
            )?;
            let copies = intent
                .copies
                .iter()
                .map(|copy| {
                    let dir = open_directory(&files, Path::new(&copy.alias))?;
                    Ok(NativeCopy {
                        alias: copy.alias.clone(),
                        identity: directory_identity(&dir)?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let record = RunWorkspaceRecord {
                intent: intent.clone(),
                container_identity: directory_identity(&files)?,
                copies,
            };
            let mut options = OpenOptions::new();
            options
                .write(true)
                .create_new(true)
                .follow(FollowSymlinks::No);
            #[cfg(unix)]
            {
                use cap_std::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut marker = stage
                .open_with("workspace.json", &options)
                .map_err(storage_error)?;
            marker
                .write_all(&serde_json::to_vec(&record).map_err(storage_error)?)
                .map_err(storage_error)?;
            marker.sync_all().map_err(storage_error)?;
            drop(marker);
            #[cfg(unix)]
            {
                sync_directory(&files)?;
                sync_directory(&stage)?;
            }
            drop(files);
            drop(stage);
            rename::no_replace(&parent, &stage_name, &parent, &intent.binding.id)?;
            #[cfg(unix)]
            sync_directory(&parent)?;
        }
        let container = open_directory(&parent, Path::new(&intent.binding.id))?;
        let record = read_record(&container)?;
        if record.intent != *intent {
            return Err(Error::UncertainEffect);
        }
        let files = self.verify_run_record(&record, &container)?;
        // Before first admission, lost publication acknowledgement may be
        // reconciled only with the exact private intent and unchanged bytes.
        let mut observed = Vec::new();
        let mut size = 0;
        workspaces::capture_transfer_directory(
            &self.versions,
            &files,
            "",
            0,
            &mut size,
            &mut observed,
        )?;
        observed.sort_by(|a, b| a.path.cmp(&b.path));
        if observed != initial_entries(intent, capture) {
            return Err(Error::FileConflict);
        }
        self.install_run_roots(&record, &files)?;
        Ok(PreparedRunWorkspace {
            record,
            _lock: lock,
            _source_locks: source_locks,
        })
    }

    /// Restore accepted custody; never create/adopt an absent or replaced tree.
    pub fn restore_run_workspace_record(
        &self,
        record: &RunWorkspaceRecord,
        spec: &RunSpec,
    ) -> Result<()> {
        record.intent.validate()?;
        if spec.input_workspace.as_ref() != Some(&record.intent.binding)
            || spec.project_id != record.intent.project_id
            || spec.order_id != record.intent.order_id
        {
            return Err(Error::Forbidden);
        }
        for supplied in &spec.folders {
            let stored = record
                .intent
                .copies
                .iter()
                .find(|c| c.grant.id == supplied.id)
                .ok_or(Error::Forbidden)?;
            let mut narrowed = stored.grant.clone();
            narrowed.writable = supplied.writable;
            if &narrowed != supplied || supplied.writable && !stored.grant.writable {
                return Err(Error::Forbidden);
            }
        }
        let parent = open_directory(&self.registry_state, Path::new("run-workspaces"))?;
        let container = open_directory(&parent, Path::new(&record.intent.binding.id))?;
        if read_record(&container)? != *record {
            return Err(Error::FileConflict);
        }
        let files = self.verify_run_record(record, &container)?;
        self.install_run_roots(record, &files)
    }

    fn verify_run_record(&self, record: &RunWorkspaceRecord, container: &Dir) -> Result<Dir> {
        let files = open_directory(container, Path::new("files"))?;
        if directory_identity(&files)? != record.container_identity
            || record.copies.len() != record.intent.copies.len()
        {
            return Err(Error::FileConflict);
        }
        for (native, copy) in record.copies.iter().zip(&record.intent.copies) {
            if native.alias != copy.alias
                || directory_identity(&open_directory(&files, Path::new(&copy.alias))?)?
                    != native.identity
            {
                return Err(Error::FileConflict);
            }
        }
        Ok(files)
    }

    fn install_run_roots(&self, record: &RunWorkspaceRecord, files: &Dir) -> Result<()> {
        let mut roots = BTreeMap::new();
        for copy in &record.intent.copies {
            let dir = open_directory(files, Path::new(&copy.alias))?;
            roots.insert(
                copy.grant.id.clone(),
                Arc::new(Root {
                    grant: copy.grant.clone(),
                    dir: Arc::new(dir),
                    namespace: Some(record.intent.binding.clone()),
                    lock_name: format!("run-workspace-data-{}", record.intent.binding.id),
                }),
            );
        }
        self.run_roots
            .write()
            .map_err(|_| Error::Forbidden)?
            .insert(record.intent.binding.id.clone(), roots);
        Ok(())
    }

    pub(super) fn spec_root(
        &self,
        spec: &RunSpec,
        id: &str,
        reconciliation: bool,
    ) -> Result<Arc<Root>> {
        if let Some(binding) = &spec.input_workspace {
            if !reconciliation {
                self.local_controller_origin()?;
            }
            uuid::Uuid::parse_str(&binding.id).map_err(|_| Error::Forbidden)?;
            let root = self
                .run_roots
                .read()
                .map_err(|_| Error::Forbidden)?
                .get(&binding.id)
                .and_then(|roots| roots.get(id))
                .cloned()
                .ok_or(Error::Forbidden)?;
            if root.namespace.as_ref() != Some(binding) {
                return Err(Error::Forbidden);
            }
            Ok(root)
        } else {
            self.lookup_root(id, reconciliation)
        }
    }

    pub(super) fn snapshot_run_workspace(&self, run: &RunRecord) -> Result<PreparedSnapshot> {
        let binding = run.spec.input_workspace.as_ref().ok_or(Error::Forbidden)?;
        let parent = open_directory(&self.registry_state, Path::new("run-workspaces"))?;
        let container = open_directory(&parent, Path::new(&binding.id))?;
        let record = read_record(&container)?;
        if &record.intent.binding != binding
            || record.intent.order_id != run.spec.order_id
            || record.intent.project_id != run.spec.project_id
        {
            return Err(Error::Forbidden);
        }
        let files = self.verify_run_record(&record, &container)?;
        let lock = self.lock_named(&format!("run-workspace-data-{}", binding.id), false)?;
        let mut entries = Vec::new();
        let mut size = 0;
        let mut seen = std::collections::BTreeSet::new();
        for grant in &run.spec.folders {
            if !seen.insert(&grant.id) {
                return Err(Error::Forbidden);
            }
            let copy = record
                .intent
                .copies
                .iter()
                .find(|c| c.grant.id == grant.id)
                .ok_or(Error::Forbidden)?;
            let mut narrowed = copy.grant.clone();
            narrowed.writable = grant.writable;
            if narrowed != *grant || grant.writable && !copy.grant.writable {
                return Err(Error::Forbidden);
            }
            entries.push(WorkspaceEntry {
                path: copy.alias.clone(),
                sha256: None,
                size: 0,
            });
            let dir = open_directory(&files, Path::new(&copy.alias))?;
            workspaces::capture_transfer_directory(
                &self.versions,
                &dir,
                &copy.alias,
                1,
                &mut size,
                &mut entries,
            )?;
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        let mut snapshot = WorkspaceSnapshot {
            run_id: run.id.clone(),
            order_id: run.spec.order_id.clone(),
            entries,
            sha256: String::new(),
        };
        snapshot.sha256 = workspace_snapshot_hash(&snapshot);
        verify_workspace_snapshot(&snapshot)?;
        Ok(PreparedSnapshot {
            snapshot,
            _lock: lock,
        })
    }
}
