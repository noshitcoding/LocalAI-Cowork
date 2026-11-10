//! Native file capabilities. Only explicitly configured roots acquire ambient authority.
mod automation_watch;
pub mod delivery;
mod locks;
pub use automation_watch::PreparedFileWatchScan;
pub use locks::ResourceLock;
mod operations;
mod private_paths;
mod registry;
pub use private_paths::{DOCUMENT_WORKSPACE_INIT_PREFIX, document_workspace_base};
mod execution_inputs;
mod managed_inputs;
mod rename;
mod uploads;
pub mod workspace_receive;
mod workspace_transfer;
mod workspaces;
pub use managed_inputs::{ManagedResultReadGuard, PreparedManagedInput};
mod managed_result_stages;
pub use managed_result_stages::{
    ManagedResultStage, ManagedResultStageBinding, ManagedResultStageRecord,
    OwnedVerifiedManagedResultStage, VerifiedManagedResultStage,
};
mod run_workspace_limits;
mod run_workspaces;
pub use cowork_contracts::{NativeFolderSelection, RegisteredFolder};
pub use execution_inputs::PreparedInputCapture;
pub use operations::{GeneratedContent, PreparedOperation, RetainedContent};
pub use registry::inspect_native_folder;
pub use run_workspaces::{PreparedRunWorkspace, RunWorkspaceIntent, RunWorkspaceRecord};
pub use workspaces::{BinaryFile, PreparedSnapshot};

use cap_fs_ext::{FollowSymlinks, MetadataExt, OpenOptionsFollowExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use cowork_contracts::*;
use cowork_domain::*;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

pub const MAX_TEXT_BYTES: u64 = 1_000_000;
const MAX_PREVIEW_BYTES: u64 = 2 * 1024 * 1024;
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FolderConfig {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub writable: bool,
}

struct Root {
    grant: FolderGrant,
    dir: Arc<Dir>,
    lock_name: String,
    namespace: Option<RunWorkspaceBinding>,
}

pub struct FileBroker {
    roots: RwLock<BTreeMap<String, Arc<Root>>>,
    run_roots: RwLock<BTreeMap<String, BTreeMap<String, Arc<Root>>>>,
    workspaces: Dir,
    locks: Dir,
    versions: Arc<Dir>,
    registry_state: Dir,
    protected_paths: Vec<PathBuf>,
    document_workspace: Option<private_paths::DocumentWorkspaceReservation>,
    configured_paths: BTreeMap<String, PathBuf>,
}

/// Keeps the OS writer lock until the application has durably recorded the outcome.
pub struct PreparedWrite {
    pub change: FileChange,
    parent: Dir,
    name: String,
    content: String,
    _lock: ResourceLock,
    versions: Arc<Dir>,
}

impl FileBroker {
    pub fn open(state_dir: &Path, configs: Vec<FolderConfig>) -> Result<Self> {
        Self::open_with_protected(state_dir, configs, &[])
    }

    pub fn open_with_protected(
        state_dir: &Path,
        configs: Vec<FolderConfig>,
        protected: &[PathBuf],
    ) -> Result<Self> {
        let document_workspace = private_paths::DocumentWorkspaceReservation::native()?;
        if let Some(reservation) = &document_workspace {
            reservation.check(state_dir, None)?;
        }
        std::fs::create_dir_all(state_dir).map_err(storage_error)?;
        let mut protected_paths = vec![comparison_path(state_dir)?];
        for path in protected {
            protected_paths.push(comparison_path(path)?);
        }
        let state = Dir::open_ambient_dir(state_dir, ambient_authority()).map_err(storage_error)?;
        state.create_dir_all("locks").map_err(storage_error)?;
        state.create_dir_all("versions").map_err(storage_error)?;
        state.create_dir_all("workspaces").map_err(storage_error)?;
        let mut roots = BTreeMap::new();
        let mut configured_paths = BTreeMap::new();
        let mut paths = Vec::<PathBuf>::new();
        for c in configs {
            valid_text(&c.id, 128)?;
            if c.id.starts_with("workspace:") || c.id.starts_with("native:") {
                return Err(Error::Forbidden);
            }
            valid_text(&c.project_id, 128)?;
            valid_text(&c.name, 200)?;
            if !c.path.is_absolute() {
                return Err(Error::Validation(
                    "A native folder grant requires an absolute path".into(),
                ));
            }
            if let Some(reservation) = &document_workspace {
                reservation.check(&c.path, None)?;
            }
            let path = std::fs::canonicalize(&c.path).map_err(storage_error)?;
            if let Some(reservation) = &document_workspace {
                reservation.check(&path, None)?;
            }
            let comparison = if cfg!(windows) {
                PathBuf::from(path.to_string_lossy().to_lowercase())
            } else {
                path.clone()
            };
            if protected_paths
                .iter()
                .any(|p| p.starts_with(&comparison) || comparison.starts_with(p))
            {
                return Err(Error::Validation(
                    "A folder grant must not overlap private application data".into(),
                ));
            }
            if paths
                .iter()
                .any(|p| p.starts_with(&comparison) || comparison.starts_with(p))
            {
                return Err(Error::Validation(
                    "Overlapping folder grants are not supported; use one shared root".into(),
                ));
            }
            paths.push(comparison.clone());
            configured_paths.insert(c.id.clone(), path.clone());
            let dir = Dir::open_ambient_dir(&path, ambient_authority()).map_err(storage_error)?;
            if let Some(reservation) = &document_workspace {
                reservation.check(&path, Some(&dir))?;
            }
            let metadata = dir.dir_metadata().map_err(storage_error)?;
            let identity = format!("{}-{}", metadata.dev(), metadata.ino());
            if roots
                .values()
                .any(|root: &Arc<Root>| root.lock_name == identity)
            {
                return Err(Error::Validation("Duplicate folder identity".into()));
            }
            let grant = FolderGrant {
                order_id: None,
                resource_id: hash(identity.as_bytes()),
                id: c.id.clone(),
                project_id: c.project_id.clone(),
                name: c.name.clone(),
                writable: c.writable,
                revision: canonical_hash(&json!({"config":c,"canonical":path,"identity":identity})),
            };
            if roots
                .insert(
                    c.id,
                    Arc::new(Root {
                        namespace: None,
                        grant,
                        dir: Arc::new(dir),
                        lock_name: identity,
                    }),
                )
                .is_some()
            {
                return Err(Error::Validation("Duplicate folder id".into()));
            }
        }
        let broker = Self {
            roots: RwLock::new(roots),
            run_roots: RwLock::new(BTreeMap::new()),
            workspaces: state.open_dir("workspaces").map_err(storage_error)?,
            locks: state.open_dir("locks").map_err(storage_error)?,
            versions: Arc::new(state.open_dir("versions").map_err(storage_error)?),
            registry_state: state,
            protected_paths,
            document_workspace,
            configured_paths,
        };
        broker.validate_registry_config()?;
        Ok(broker)
    }

    pub fn grants(&self, project: &str) -> Result<Vec<FolderGrant>> {
        self.current_grants(project)
    }
    pub fn validate_grants(&self, project: &str, grants: &[FolderGrant]) -> Result<()> {
        if grants.iter().any(|g| g.order_id.is_some()) {
            return Err(Error::Forbidden);
        }
        self.validate_run_grants(project, None, grants)
    }
    /// Run-scoped capabilities may address a confirmed private namespace or an
    /// order workspace; neither is reconstructed from native source grants.
    pub fn validate_spec_grants(&self, spec: &RunSpec) -> Result<()> {
        for grant in &spec.folders {
            self.root_for_spec(spec, &grant.id, false)?;
        }
        Ok(())
    }
    fn validate_run_grants(
        &self,
        project: &str,
        order: Option<&str>,
        grants: &[FolderGrant],
    ) -> Result<()> {
        let mut seen = std::collections::HashSet::new();
        for grant in grants {
            let current = self.lookup_root(&grant.id, false)?;
            let mut narrowed = current.grant.clone();
            narrowed.writable = grant.writable;
            if !seen.insert(&grant.id)
                || grant.project_id != project
                || grant
                    .order_id
                    .as_deref()
                    .is_some_and(|id| Some(id) != order)
                || narrowed != *grant
                || grant.writable && !current.grant.writable
            {
                return Err(Error::Forbidden);
            }
        }
        Ok(())
    }
    fn root(&self, run: &RunRecord, id: &str, write: bool) -> Result<Arc<Root>> {
        self.root_for_spec(&run.spec, id, write)
    }
    fn root_for_spec(&self, spec: &RunSpec, id: &str, write: bool) -> Result<Arc<Root>> {
        let root = self.spec_root(spec, id, false)?;
        let supplied = spec
            .folders
            .iter()
            .find(|grant| grant.id == id)
            .ok_or(Error::Forbidden)?;
        let mut narrowed = root.grant.clone();
        narrowed.writable = supplied.writable;
        if root.grant.project_id != spec.project_id
            || root
                .grant
                .order_id
                .as_ref()
                .is_some_and(|order| order != &spec.order_id)
            || spec.folders.iter().filter(|g| g.id == id).count() != 1
            || supplied != &narrowed
            || supplied.writable && !root.grant.writable
            || (write
                && (!supplied.writable
                    || !root.grant.writable
                    || spec.access_mode == AccessMode::ReadOnly))
        {
            return Err(Error::Forbidden);
        }
        Ok(root)
    }
    fn lock(&self, root: &Root, write: bool) -> Result<ResourceLock> {
        let file = self.lock_named(&root.lock_name, write)?;
        // Revalidate after locking: a revocation may have committed after root lookup.
        if root.grant.order_id.is_none() && root.namespace.is_none() {
            self.require_active(&root.grant)?;
        }
        Ok(file)
    }
    fn lock_named(&self, name: &str, write: bool) -> Result<ResourceLock> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        let file = self
            .locks
            .open_with(name, &options)
            .map_err(storage_error)?
            .into_std();
        let result = if write {
            file.try_lock()
        } else {
            file.try_lock_shared()
        };
        match result {
            Ok(()) => Ok(ResourceLock(file)),
            Err(std::fs::TryLockError::WouldBlock) => Err(Error::WorkspaceBusy),
            Err(std::fs::TryLockError::Error(e)) => Err(storage_error(e)),
        }
    }
    pub fn list(&self, run: &RunRecord, location: &FileLocation) -> Result<DirectoryListing> {
        let root = self.root(run, &location.folder_id, false)?;
        self.list_root(&root, location)
    }
    fn list_root(&self, root: &Root, location: &FileLocation) -> Result<DirectoryListing> {
        let _lock = self.lock(root, false)?;
        let path = relative_path(&location.path, true)?;
        let directory = open_directory(&root.dir, &path)?;
        let mut entries = Vec::new();
        let mut truncated = false;
        for entry in directory.entries().map_err(storage_error)? {
            let entry = entry.map_err(storage_error)?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if name.to_ascii_lowercase().starts_with(".cowork-") {
                continue;
            }
            if entries.len() >= 500 {
                truncated = true;
                break;
            }
            let metadata = directory.symlink_metadata(&name).map_err(storage_error)?;
            let kind = if metadata.is_symlink() {
                "blocked_link"
            } else if metadata.is_dir() {
                "directory"
            } else if metadata.is_file() {
                "file"
            } else {
                "blocked_special"
            };
            entries.push(FileEntry {
                name: name.clone(),
                path: if path == Path::new(".") {
                    name
                } else {
                    format!("{}/{}", location.path, name)
                },
                kind: kind.into(),
                size: metadata.len(),
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(DirectoryListing {
            folder_id: location.folder_id.clone(),
            path: location.path.clone(),
            entries,
            truncated,
        })
    }
    /// Keeps a rendered PNG preview content-addressed with the other retained versions.
    pub fn retain_preview(&self, bytes: &[u8]) -> Result<String> {
        if bytes.len() as u64 > MAX_PREVIEW_BYTES || !bytes.starts_with(PNG_SIGNATURE) {
            return Err(Error::Validation(
                "Previews must be PNG images of at most 2 MiB".into(),
            ));
        }
        save_version(&self.versions, bytes)?;
        Ok(hash(bytes))
    }
    /// Only PNG previews are readable this way, never other retained file versions.
    pub fn read_preview(&self, sha256: &str) -> Result<Vec<u8>> {
        validate_hash(sha256)?;
        let bytes = read_regular_limit(&self.versions, Path::new(sha256), MAX_PREVIEW_BYTES)?
            .ok_or(Error::NotFound)?;
        if hash(&bytes) != sha256 || !bytes.starts_with(PNG_SIGNATURE) {
            return Err(Error::NotFound);
        }
        Ok(bytes)
    }
    /// Bounded binary read for built-in document tools; the caller hashes the returned bytes.
    pub fn read_binary(
        &self,
        run: &RunRecord,
        location: &FileLocation,
        limit: u64,
    ) -> Result<Vec<u8>> {
        let root = self.root(run, &location.folder_id, false)?;
        let _lock = self.lock(&root, false)?;
        let path = relative_path(&location.path, false)?;
        let parent = open_directory(&root.dir, path.parent().unwrap_or(Path::new(".")))?;
        let name = path.file_name().ok_or(Error::Forbidden)?;
        if parent
            .symlink_metadata(name)
            .is_ok_and(|m| m.is_file() && m.len() > limit)
        {
            return Err(Error::Validation(format!(
                "The file exceeds the {} MiB document limit",
                limit / (1024 * 1024)
            )));
        }
        read_regular_limit(&parent, name, limit)?.ok_or(Error::NotFound)
    }
    pub fn read(&self, run: &RunRecord, location: &FileLocation) -> Result<TextFile> {
        let root = self.root(run, &location.folder_id, false)?;
        self.read_root(&root, location)
    }
    /// Admission-time context read uses only the exact selected capabilities.
    pub fn read_context(&self, spec: &RunSpec, location: &FileLocation) -> Result<TextFile> {
        let root = self.root_for_spec(spec, &location.folder_id, false)?;
        self.read_root(&root, location)
    }
    fn read_root(&self, root: &Root, location: &FileLocation) -> Result<TextFile> {
        let _lock = self.lock(root, false)?;
        let path = relative_path(&location.path, false)?;
        let parent = open_directory(&root.dir, path.parent().unwrap_or(Path::new(".")))?;
        let bytes = read_regular(&parent, path.file_name().ok_or(Error::Forbidden)?)?
            .ok_or(Error::NotFound)?;
        let sha256 = hash(&bytes);
        let size = bytes.len() as u64;
        let content = String::from_utf8(bytes)
            .map_err(|_| Error::Validation("File is not UTF-8 text".into()))?;
        Ok(TextFile {
            folder_id: location.folder_id.clone(),
            path: location.path.clone(),
            content,
            sha256,
            size,
        })
    }
    pub fn prepare_write(&self, run: &RunRecord, call: &ToolCall) -> Result<PreparedWrite> {
        let input: WriteTextFile = serde_json::from_value(call.arguments.clone())
            .map_err(|_| Error::Validation("Invalid write_text_file arguments".into()))?;
        if input.content.len() as u64 > MAX_TEXT_BYTES {
            return Err(Error::Validation("Text file exceeds the 1 MB limit".into()));
        }
        if let Some(expected) = &input.expected_sha256 {
            validate_hash(expected)?;
        }
        let root = self.root(run, &input.folder_id, true)?;
        let lock = self.lock(&root, true)?;
        run_workspace_limits::validate_path(&root, &input.path)?;
        let path = relative_path(&input.path, false)?;
        let parent = open_directory(&root.dir, path.parent().unwrap_or(Path::new(".")))?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or(Error::Forbidden)?
            .to_owned();
        let before = read_regular(&parent, Path::new(&name))?;
        let before_sha256 = before.as_ref().map(|b| hash(b));
        if before_sha256 != input.expected_sha256 {
            return Err(Error::FileConflict);
        }
        self.check_run_growth(
            &root,
            run_workspace_limits::Growth::file(
                before.as_ref().map(|bytes| bytes.len() as u64),
                input.content.len() as u64,
            )
            .node_at(&input.path, true, input.content.len() as u64, &[]),
        )?;
        if let Some(bytes) = before {
            save_version(&self.versions, &bytes)?;
        }
        // Retain proposed bytes before the journal boundary, including uncertain outcomes.
        save_version(&self.versions, input.content.as_bytes())?;
        let change = FileChange {
            run_id: run.id.clone(),
            tool_call_id: call.id.clone(),
            folder_id: input.folder_id,
            path: input.path,
            before_sha256,
            after_sha256: hash(input.content.as_bytes()),
            size: input.content.len() as u64,
        };
        Ok(PreparedWrite {
            change,
            parent,
            name,
            content: input.content,
            _lock: lock,
            versions: self.versions.clone(),
        })
    }
    /// No writing occurs here. The lock fences reconciliation against a still-alive writer.
    pub fn observe_change(
        &self,
        run: &RunRecord,
        change: &FileChange,
    ) -> Result<(Option<String>, ResourceLock)> {
        let root = self.reconciliation_root(run, &change.folder_id)?;
        let lock = self.lock_named(&root.lock_name, true)?;
        let path = relative_path(&change.path, false)?;
        let parent = open_directory(&root.dir, path.parent().unwrap_or(Path::new(".")))?;
        let bytes = read_regular(&parent, path.file_name().ok_or(Error::Forbidden)?)?;
        Ok((bytes.map(|b| hash(&b)), lock))
    }
    pub fn read_version(&self, sha256: &str) -> Result<String> {
        validate_hash(sha256)?;
        let bytes = read_regular(&self.versions, Path::new(sha256))?.ok_or(Error::NotFound)?;
        if hash(&bytes) != sha256 {
            return Err(Error::Validation(
                "Previous version integrity check failed".into(),
            ));
        }
        String::from_utf8(bytes)
            .map_err(|_| Error::Validation("Previous version is not UTF-8".into()))
    }
}

fn comparison_path(path: &Path) -> Result<PathBuf> {
    let canonical = std::fs::canonicalize(path).map_err(storage_error)?;
    Ok(if cfg!(windows) {
        PathBuf::from(canonical.to_string_lossy().to_lowercase())
    } else {
        canonical
    })
}

impl PreparedWrite {
    /// Application must persist the in-flight marker before calling this method.
    pub fn apply(&mut self) -> Result<()> {
        let current = read_regular(&self.parent, Path::new(&self.name))?.map(|b| hash(&b));
        if current != self.change.before_sha256 {
            return Err(Error::FileConflict);
        }
        let temporary = format!(".cowork-{}.tmp", uuid::Uuid::new_v4());
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        let mut file = self
            .parent
            .open_with(&temporary, &options)
            .map_err(storage_error)?;
        file.write_all(self.content.as_bytes())
            .map_err(storage_error)?;
        file.sync_all().map_err(storage_error)?;
        drop(file);
        // Check again immediately before publication. Other Cowork writers are OS-locked.
        let current = read_regular(&self.parent, Path::new(&self.name))?.map(|b| hash(&b));
        if current != self.change.before_sha256 {
            let _ = self.parent.remove_file(&temporary);
            return Err(Error::FileConflict);
        }
        if self.change.before_sha256.is_none() {
            // A concurrently created destination must never be overwritten.
            self.parent
                .hard_link(&temporary, &self.parent, &self.name)
                .map_err(storage_error)?;
            self.parent.remove_file(&temporary).map_err(storage_error)?;
        } else {
            self.parent
                .rename(&temporary, &self.parent, &self.name)
                .map_err(storage_error)?;
        }
        #[cfg(unix)]
        sync_directory(&self.parent)?;
        // Once publication was attempted, no error is proof that nothing changed.
        let after = read_regular(&self.parent, Path::new(&self.name))
            .map_err(|_| Error::UncertainEffect)?
            .ok_or(Error::UncertainEffect)?;
        if hash(&after) != self.change.after_sha256 {
            return Err(Error::UncertainEffect);
        }
        save_version(&self.versions, self.content.as_bytes())
            .map_err(|_| Error::UncertainEffect)?;
        Ok(())
    }
}

pub fn relative_path(value: &str, allow_root: bool) -> Result<PathBuf> {
    if allow_root && value == "." {
        return Ok(PathBuf::from("."));
    }
    if value.is_empty() || value.len() > 4096 || value.contains('\\') {
        return Err(Error::Forbidden);
    }
    for part in value.split('/') {
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.len() > 255
            || part.ends_with(['.', ' '])
            || part.to_ascii_lowercase().starts_with(".cowork-")
            || part
                .chars()
                .any(|c| c.is_control() || matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|'))
            || matches!(
                stem.as_str(),
                "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
            )
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit())
            || ["COM¹", "COM²", "COM³", "LPT¹", "LPT²", "LPT³"].contains(&stem.as_str())
        {
            return Err(Error::Forbidden);
        }
    }
    Ok(PathBuf::from(value))
}

fn open_directory(root: &Dir, path: &Path) -> Result<Dir> {
    let mut directory = root.try_clone().map_err(storage_error)?;
    for component in path.components() {
        if let std::path::Component::CurDir = component {
            continue;
        }
        let std::path::Component::Normal(name) = component else {
            return Err(Error::Forbidden);
        };
        if directory
            .symlink_metadata(name)
            .map_err(storage_error)?
            .is_symlink()
        {
            return Err(Error::Forbidden);
        }
        // Capability-relative traversal also rejects a link exchanged after the metadata check.
        directory = directory.open_dir(name).map_err(|_| Error::Forbidden)?;
    }
    Ok(directory)
}

fn read_regular(dir: &Dir, path: impl AsRef<Path>) -> Result<Option<Vec<u8>>> {
    read_regular_limit(dir, path, MAX_TEXT_BYTES)
}

fn read_regular_limit(dir: &Dir, path: impl AsRef<Path>, limit: u64) -> Result<Option<Vec<u8>>> {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = match dir.open_with(path, &options) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(Error::Forbidden),
    };
    let before = file.metadata().map_err(storage_error)?;
    if !before.is_file() || before.nlink() != 1 || before.len() > limit {
        return Err(Error::Forbidden);
    }
    let mut content = Vec::new();
    (&file)
        .take(limit + 1)
        .read_to_end(&mut content)
        .map_err(storage_error)?;
    let after = file.metadata().map_err(storage_error)?;
    if content.len() as u64 > limit
        || after.len() != before.len()
        || after.modified().ok() != before.modified().ok()
        || after.nlink() != 1
    {
        return Err(Error::FileConflict);
    }
    Ok(Some(content))
}

fn validate_hash(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::Validation("Invalid SHA-256".into()));
    }
    Ok(())
}
fn save_version(dir: &Dir, content: &[u8]) -> Result<()> {
    let sha = hash(content);
    if let Some(existing) = read_regular_limit(dir, Path::new(&sha), workspaces::MAX_SNAPSHOT_FILE)?
    {
        if hash(&existing) != sha {
            return Err(Error::Validation(
                "Version store integrity check failed".into(),
            ));
        }
        return Ok(());
    }
    let temporary = format!(".cowork-{}.tmp", uuid::Uuid::new_v4());
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = dir.open_with(&temporary, &options).map_err(storage_error)?;
    file.write_all(content).map_err(storage_error)?;
    file.sync_all().map_err(storage_error)?;
    drop(file);
    dir.rename(&temporary, dir, &sha).map_err(storage_error)?;
    #[cfg(unix)]
    sync_directory(dir)?;
    Ok(())
}

#[cfg(unix)]
fn sync_directory(dir: &Dir) -> Result<()> {
    use cap_std::fs::OpenOptionsExt;
    // Dir may use O_PATH on Linux; fsync on that descriptor is EBADF. Reopen only
    // this capability's "." with a readable directory descriptor, never ambient authority.
    let mut options = OpenOptions::new();
    options.read(true).custom_flags(libc::O_DIRECTORY);
    dir.open_with(".", &options)
        .map_err(storage_error)?
        .sync_all()
        .map_err(storage_error)
}
