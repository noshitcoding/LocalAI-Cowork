use super::*;

pub(super) const MAX_SNAPSHOT_FILE: u64 = MAX_DOWNLOAD_BYTES;
const MAX_SNAPSHOT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;

/// Holds the read lock until the snapshot and completed result commit together.
pub struct PreparedSnapshot {
    pub snapshot: WorkspaceSnapshot,
    pub(super) _lock: ResourceLock,
}

pub struct BinaryFile {
    pub metadata: DownloadMetadata,
    pub bytes: Vec<u8>,
}

fn binary_file(input: &DownloadFile, bytes: Vec<u8>) -> Result<BinaryFile> {
    let path = relative_path(&input.path, false)?;
    let sha256 = hash(&bytes);
    if let Some(expected) = &input.expected_sha256 {
        validate_hash(expected)?;
        if expected != &sha256 {
            return Err(Error::FileConflict);
        }
    }
    Ok(BinaryFile {
        metadata: DownloadMetadata {
            file_name: path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or(Error::Forbidden)?
                .into(),
            size: bytes.len() as u64,
            sha256,
        },
        bytes,
    })
}

impl FileBroker {
    fn workspace_root(&self, workspace: &Workspace) -> Result<Arc<Root>> {
        let root = self
            .roots
            .read()
            .map_err(|_| Error::Forbidden)?
            .get(&workspace.folder.id)
            .cloned()
            .ok_or(Error::Forbidden)?;
        if root.grant != workspace.folder
            || root.grant.order_id.as_deref() != Some(&workspace.order_id)
        {
            return Err(Error::Forbidden);
        }
        Ok(root)
    }
    pub fn list_workspace(&self, workspace: &Workspace, path: &str) -> Result<DirectoryListing> {
        self.list_root(
            self.workspace_root(workspace)?.as_ref(),
            &FileLocation {
                folder_id: workspace.folder.id.clone(),
                path: path.into(),
            },
        )
    }
    pub fn read_workspace(&self, workspace: &Workspace, path: &str) -> Result<TextFile> {
        self.read_root(
            self.workspace_root(workspace)?.as_ref(),
            &FileLocation {
                folder_id: workspace.folder.id.clone(),
                path: path.into(),
            },
        )
    }
    pub fn stat_workspace(&self, workspace: &Workspace, path: &str) -> Result<FileStat> {
        let root = self.workspace_root(workspace)?;
        let _lock = self.lock(&root, false)?;
        let (node, _) = operations::inspect(&root.dir, path, &self.versions)?;
        Ok(FileStat {
            folder_id: workspace.folder.id.clone(),
            path: path.into(),
            node,
        })
    }

    pub fn download_workspace(
        &self,
        workspace: &Workspace,
        input: &DownloadFile,
    ) -> Result<BinaryFile> {
        let root = self.workspace_root(workspace)?;
        let _lock = self.lock(&root, false)?;
        let path = relative_path(&input.path, false)?;
        let parent = open_directory(&root.dir, path.parent().unwrap_or(Path::new(".")))?;
        let bytes = read_regular_limit(
            &parent,
            path.file_name().ok_or(Error::Forbidden)?,
            MAX_DOWNLOAD_BYTES,
        )?
        .ok_or(Error::NotFound)?;
        binary_file(input, bytes)
    }

    /// The runtime supplies a confirmed snapshot from storage, never a client-supplied manifest.
    pub fn download_snapshot(
        &self,
        snapshot: &WorkspaceSnapshot,
        input: &DownloadFile,
    ) -> Result<BinaryFile> {
        validate_manifest(snapshot)?;
        relative_path(&input.path, false)?;
        let entry = snapshot
            .entries
            .iter()
            .find(|e| e.path == input.path)
            .ok_or(Error::NotFound)?;
        let sha = entry.sha256.as_ref().ok_or(Error::Forbidden)?;
        let bytes =
            read_regular_limit(&self.versions, sha, MAX_DOWNLOAD_BYTES)?.ok_or(Error::NotFound)?;
        if bytes.len() as u64 != entry.size || hash(&bytes) != *sha {
            return Err(Error::Validation(
                "Retained content integrity check failed".into(),
            ));
        }
        binary_file(input, bytes)
    }

    /// Materialization is recoverable: an unpublished stage is never adopted.
    /// A previously registered directory that disappears must not become an empty replacement.
    pub fn ensure_workspace(
        &self,
        order: &Order,
        base: Option<&WorkspaceSnapshot>,
        expected: Option<&Workspace>,
    ) -> Result<Workspace> {
        uuid::Uuid::parse_str(&order.id).map_err(|_| Error::Forbidden)?;
        if let Some(snapshot) = base {
            validate_manifest(snapshot)?;
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        let lock = self
            .locks
            .open_with(format!("workspace-{}", order.id), &options)
            .map_err(storage_error)?
            .into_std();
        lock.try_lock().map_err(|_| Error::WorkspaceBusy)?;
        let exists = match self.workspaces.symlink_metadata(&order.id) {
            Ok(m) if m.is_dir() && !m.is_symlink() => true,
            Ok(_) => return Err(Error::Forbidden),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(storage_error(e)),
        };
        if !exists {
            if expected.is_some() {
                return Err(Error::Validation("Registered workspace is missing".into()));
            }
            let stage = format!(".cowork-{}", uuid::Uuid::new_v4());
            self.workspaces.create_dir(&stage).map_err(storage_error)?;
            let staging = self.workspaces.open_dir(&stage).map_err(storage_error)?;
            staging.create_dir("files").map_err(storage_error)?;
            let files = staging.open_dir("files").map_err(storage_error)?;
            if let Some(snapshot) = base {
                materialize_entries(&self.versions, &files, &snapshot.entries)?;
            }
            let workspace = workspace_record(order, base, &files)?;
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            let mut marker = staging
                .open_with("workspace.json", &options)
                .map_err(storage_error)?;
            marker
                .write_all(&serde_json::to_vec(&workspace).map_err(storage_error)?)
                .map_err(storage_error)?;
            marker.sync_all().map_err(storage_error)?;
            drop(marker);
            #[cfg(unix)]
            {
                sync_directory(&files)?;
                sync_directory(&staging)?;
            }
            drop(files);
            drop(staging);
            self.workspaces
                .rename(&stage, &self.workspaces, &order.id)
                .map_err(storage_error)?;
            #[cfg(unix)]
            sync_directory(&self.workspaces)?;
        }
        let container = open_directory(&self.workspaces, Path::new(&order.id))?;
        let bytes = read_regular(&container, "workspace.json")?.ok_or(Error::NotFound)?;
        let stored: Workspace = serde_json::from_slice(&bytes).map_err(storage_error)?;
        let files = open_directory(&container, Path::new("files"))?;
        let workspace = workspace_record(order, base, &files)?;
        if stored != workspace || expected.is_some_and(|w| w != &workspace) {
            return Err(Error::Validation(
                "Workspace identity or origin changed".into(),
            ));
        }
        let metadata = files.dir_metadata().map_err(storage_error)?;
        let root = Arc::new(Root {
            namespace: None,
            grant: workspace.folder.clone(),
            dir: Arc::new(files),
            lock_name: format!("{}-{}", metadata.dev(), metadata.ino()),
        });
        self.roots
            .write()
            .map_err(|_| Error::Forbidden)?
            .insert(root.grant.id.clone(), root);
        Ok(workspace)
    }

    pub fn snapshot_workspace(&self, run: &RunRecord) -> Result<PreparedSnapshot> {
        if run.spec.input_workspace.is_some() {
            return self.snapshot_run_workspace(run);
        }
        let grant = run
            .spec
            .folders
            .iter()
            .find(|f| f.order_id.as_deref() == Some(&run.spec.order_id))
            .ok_or(Error::Forbidden)?;
        let root = self.root(run, &grant.id, false)?;
        let lock = self.lock(&root, false)?;
        let mut entries = Vec::new();
        let mut size = 0;
        capture_directory(&self.versions, &root.dir, "", 0, &mut size, &mut entries)?;
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        let mut snapshot = WorkspaceSnapshot {
            run_id: run.id.clone(),
            order_id: run.spec.order_id.clone(),
            entries,
            sha256: String::new(),
        };
        snapshot.sha256 = workspace_snapshot_hash(&snapshot);
        validate_manifest(&snapshot)?;
        Ok(PreparedSnapshot {
            snapshot,
            _lock: lock,
        })
    }
}

pub(super) fn capture_directory(
    versions: &Dir,
    directory: &Dir,
    prefix: &str,
    depth: usize,
    total: &mut u64,
    entries: &mut Vec<WorkspaceEntry>,
) -> Result<()> {
    capture_directory_checked(versions, directory, prefix, depth, total, entries, false)
}

pub(super) fn capture_transfer_directory(
    versions: &Dir,
    directory: &Dir,
    prefix: &str,
    depth: usize,
    total: &mut u64,
    entries: &mut Vec<WorkspaceEntry>,
) -> Result<()> {
    capture_directory_checked(versions, directory, prefix, depth, total, entries, true)
}

fn capture_directory_checked(
    versions: &Dir,
    directory: &Dir,
    prefix: &str,
    depth: usize,
    total: &mut u64,
    entries: &mut Vec<WorkspaceEntry>,
    portable: bool,
) -> Result<()> {
    if depth > 64 {
        return Err(Error::Validation(
            "Workspace directory depth exceeds 64".into(),
        ));
    }
    let mut names = Vec::new();
    for entry in directory.entries().map_err(storage_error)? {
        let entry = entry.map_err(storage_error)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| Error::Forbidden)?;
        if name.to_ascii_lowercase().starts_with(".cowork-") {
            continue;
        }
        names.push(name);
        if names.len() + entries.len() > MAX_ENTRIES {
            return Err(Error::Validation("Workspace exceeds 10000 entries".into()));
        }
    }
    names.sort();
    for name in names {
        if entries.len() >= MAX_ENTRIES {
            return Err(Error::Validation("Workspace exceeds 10000 entries".into()));
        }
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        relative_path(&path, false)?;
        if portable {
            cowork_domain::workspace_transfer::validate_path(&path)?;
        }
        let metadata = directory.symlink_metadata(&name).map_err(storage_error)?;
        if metadata.is_symlink() {
            return Err(Error::Forbidden);
        }
        if metadata.is_dir() {
            entries.push(WorkspaceEntry {
                path: path.clone(),
                sha256: None,
                size: 0,
            });
            let child = open_directory(directory, Path::new(&name))?;
            capture_directory_checked(
                versions,
                &child,
                &path,
                depth + 1,
                total,
                entries,
                portable,
            )?;
        } else {
            let bytes = read_regular_limit(directory, &name, MAX_SNAPSHOT_FILE)?
                .ok_or(Error::FileConflict)?;
            *total += bytes.len() as u64;
            if *total > MAX_SNAPSHOT_BYTES {
                return Err(Error::Validation(
                    "Workspace snapshot exceeds 512 MiB".into(),
                ));
            }
            save_version(versions, &bytes)?;
            entries.push(WorkspaceEntry {
                path,
                sha256: Some(hash(&bytes)),
                size: bytes.len() as u64,
            });
        }
    }
    Ok(())
}

fn workspace_record(
    order: &Order,
    base: Option<&WorkspaceSnapshot>,
    files: &Dir,
) -> Result<Workspace> {
    let metadata = files.dir_metadata().map_err(storage_error)?;
    let identity = format!("{}-{}", metadata.dev(), metadata.ino());
    let base_hash = base.map(|s| s.sha256.clone());
    Ok(Workspace {
        order_id: order.id.clone(),
        base_snapshot_sha256: base_hash.clone(),
        folder: FolderGrant {
            id: format!("workspace:{}", order.id),
            project_id: order.project_id.clone(),
            order_id: Some(order.id.clone()),
            name: "Workspace".into(),
            writable: true,
            resource_id: hash(identity.as_bytes()),
            revision: canonical_hash(
                &json!({"order":order.id,"project":order.project_id,"base":base_hash,"identity":identity}),
            ),
        },
    })
}

fn validate_manifest(snapshot: &WorkspaceSnapshot) -> Result<()> {
    verify_workspace_snapshot(snapshot)?;
    validate_entries(&snapshot.entries)
}

pub(super) fn validate_entries(entries: &[WorkspaceEntry]) -> Result<()> {
    if entries.len() > MAX_ENTRIES {
        return Err(Error::Forbidden);
    }
    let mut previous = "";
    let mut directories = std::collections::HashSet::new();
    let mut total = 0u64;
    for entry in entries {
        let path = relative_path(&entry.path, false)?;
        if entry.path.as_str() <= previous || path.components().count() > 65 {
            return Err(Error::Forbidden);
        }
        previous = &entry.path;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty())
            && !directories.contains(parent)
        {
            return Err(Error::Forbidden);
        }
        if let Some(sha) = &entry.sha256 {
            validate_hash(sha)?;
            if entry.size > MAX_SNAPSHOT_FILE {
                return Err(Error::Forbidden);
            }
            total = total.checked_add(entry.size).ok_or(Error::Forbidden)?;
            if total > MAX_SNAPSHOT_BYTES {
                return Err(Error::Forbidden);
            }
        } else {
            if entry.size != 0 {
                return Err(Error::Forbidden);
            }
            directories.insert(path);
        }
    }
    Ok(())
}

pub(super) fn restore_blob(
    versions: &Dir,
    parent: &Dir,
    name: &str,
    sha: &str,
    expected_size: Option<u64>,
) -> Result<()> {
    validate_hash(sha)?;
    let bytes = read_regular_limit(versions, sha, MAX_SNAPSHOT_FILE)?.ok_or(Error::NotFound)?;
    if hash(&bytes) != sha || expected_size.is_some_and(|size| size != bytes.len() as u64) {
        return Err(Error::Validation(
            "Retained content integrity check failed".into(),
        ));
    }
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    let mut file = parent.open_with(name, &options).map_err(storage_error)?;
    file.write_all(&bytes).map_err(storage_error)?;
    file.sync_all().map_err(storage_error)?;
    #[cfg(unix)]
    sync_directory(parent)?;
    Ok(())
}

pub(super) fn materialize_entries(
    versions: &Dir,
    files: &Dir,
    entries: &[WorkspaceEntry],
) -> Result<()> {
    validate_entries(entries)?;
    for entry in entries {
        let path = relative_path(&entry.path, false)?;
        let parent = open_directory(files, path.parent().unwrap_or(Path::new(".")))?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or(Error::Forbidden)?;
        if let Some(sha) = &entry.sha256 {
            restore_blob(versions, &parent, name, sha, Some(entry.size))?;
        } else {
            parent.create_dir(name).map_err(storage_error)?;
            #[cfg(unix)]
            sync_directory(&parent)?;
        }
    }
    Ok(())
}
