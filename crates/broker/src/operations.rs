use super::delivery::permissions;
use super::*;

#[derive(Clone, Debug)]
pub enum RetainedContent {
    File {
        sha256: String,
    },
    Directory {
        sha256: String,
        entries: Vec<WorkspaceEntry>,
    },
}

/// Bytes produced by a built-in document tool; a directory holds relative file paths.
#[derive(Clone, Debug)]
pub enum GeneratedContent {
    File(Vec<u8>),
    Directory(Vec<(String, Vec<u8>)>),
}

pub struct PreparedOperation {
    pub operation: FileOperation,
    root: Arc<Root>,
    versions: Arc<Dir>,
    source_parent: Dir,
    source_name: String,
    target_parent: Dir,
    target_name: String,
    before_permissions: Option<permissions::PermissionState>,
    _lock: ResourceLock,
}

impl FileBroker {
    /// The runtime resolves retained content from an authorized immutable receipt;
    /// callers never supply a private version-store path.
    pub fn prepare_restore(
        &self,
        run: &RunRecord,
        tool: &ToolCall,
        content: &RetainedContent,
    ) -> Result<PreparedOperation> {
        if tool.name != "restore_file" {
            return Err(Error::Forbidden);
        }
        let input: RestoreFile =
            serde_json::from_value(tool.arguments.clone()).map_err(storage_error)?;
        let root = self.root(run, &input.folder_id, true)?;
        let lock = self.lock(&root, true)?;
        run_workspace_limits::validate_path(&root, &input.path)?;
        if inspect(&root.dir, &input.path, &self.versions)?.0.is_some() {
            return Err(Error::FileConflict);
        }
        let (bytes, entries) = match content {
            RetainedContent::File { sha256 } => {
                validate_hash(sha256)?;
                let bytes =
                    read_regular_limit(&self.versions, sha256, workspaces::MAX_SNAPSHOT_FILE)?
                        .ok_or(Error::NotFound)?;
                if hash(&bytes) != *sha256 {
                    return Err(Error::FileConflict);
                }
                (bytes.len() as u64, 1)
            }
            RetainedContent::Directory { sha256, entries } => {
                workspaces::validate_entries(entries)?;
                if canonical_hash(entries) != *sha256 {
                    return Err(Error::FileConflict);
                }
                for entry in entries {
                    run_workspace_limits::validate_path(
                        &root,
                        &format!("{}/{}", input.path, entry.path),
                    )?;
                }
                let bytes = entries.iter().try_fold(0u64, |total, e| {
                    total.checked_add(e.size).ok_or(Error::Forbidden)
                })?;
                (bytes, entries.len().checked_add(1).ok_or(Error::Forbidden)?)
            }
        };
        let children = match content {
            RetainedContent::Directory { entries, .. } => entries.as_slice(),
            _ => &[],
        };
        self.check_run_growth(
            &root,
            run_workspace_limits::Growth::tree(bytes, entries).node_at(
                &input.path,
                matches!(content, RetainedContent::File { .. }),
                if children.is_empty() { bytes } else { 0 },
                children,
            ),
        )?;
        let (parent, name) = resolve_parent(&root.dir, &input.path)?;
        let stage = format!(".cowork-restore-{}", uuid::Uuid::new_v4());
        let expected_hash = match content {
            RetainedContent::File { sha256 } => {
                workspaces::restore_blob(&self.versions, &parent, &stage, sha256, None)?;
                sha256
            }
            RetainedContent::Directory { sha256, entries } => {
                workspaces::validate_entries(entries)?;
                if canonical_hash(entries) != *sha256 {
                    return Err(Error::Validation(
                        "Retained manifest integrity check failed".into(),
                    ));
                }
                parent.create_dir(&stage).map_err(storage_error)?;
                let directory = open_directory(&parent, Path::new(&stage))?;
                workspaces::materialize_entries(&self.versions, &directory, entries)?;
                #[cfg(unix)]
                sync_directory(&directory)?;
                sha256
            }
        };
        let (after, entries) = inspect_name(&parent, &stage, &self.versions)?;
        if after.as_ref().map(|n| &n.sha256) != Some(expected_hash) {
            return Err(Error::FileConflict);
        }
        let operation = FileOperation {
            run_id: run.id.clone(),
            tool_call_id: tool.id.clone(),
            folder_id: input.folder_id,
            kind: FileOperationKind::Restore,
            path: input.path,
            destination: None,
            before: None,
            after,
            entries,
            restored_from: Some(input.source),
            imported_from: None,
            generated_from: None,
        };
        Ok(PreparedOperation {
            operation,
            root,
            versions: self.versions.clone(),
            source_parent: parent.try_clone().map_err(storage_error)?,
            source_name: stage,
            target_parent: parent,
            target_name: name,
            before_permissions: None,
            _lock: lock,
        })
    }

    pub fn prepare_import(
        &self,
        run: &RunRecord,
        tool: &ToolCall,
        upload: &FileUpload,
    ) -> Result<PreparedOperation> {
        if tool.name != "import_file" {
            return Err(Error::Forbidden);
        }
        let input: ImportFile =
            serde_json::from_value(tool.arguments.clone()).map_err(storage_error)?;
        if upload.id != input.upload_id
            || upload.order_id != run.spec.order_id
            || upload.project_id != run.spec.project_id
            || upload.state != UploadState::Ready
            || canonical_hash(&upload.file) != canonical_hash(&input.file)
        {
            return Err(Error::Forbidden);
        }
        let root = self.root(run, &input.folder_id, true)?;
        // Initial import scope is the order's managed workspace, never a configured host root.
        if root.grant.order_id.as_deref() != Some(run.spec.order_id.as_str()) {
            return Err(Error::Forbidden);
        }
        let lock = self.lock(&root, true)?;
        run_workspace_limits::validate_path(&root, &input.path)?;
        self.verify_upload(upload)?;
        if let Some(expected) = &input.expected_sha256 {
            validate_hash(expected)?;
        }
        let (before, _) = inspect(&root.dir, &input.path, &self.versions)?;
        if before.as_ref().map(|n| &n.sha256) != input.expected_sha256.as_ref()
            || before
                .as_ref()
                .is_some_and(|n| n.kind != FileNodeKind::File)
        {
            return Err(Error::FileConflict);
        }
        self.check_run_growth(
            &root,
            run_workspace_limits::Growth::file(
                before.as_ref().map(|node| node.size),
                upload.file.size,
            )
            .node_at(&input.path, true, upload.file.size, &[]),
        )?;
        let (parent, name) = resolve_parent(&root.dir, &input.path)?;
        let before_permissions = if before.is_some() {
            let mut options = OpenOptions::new();
            options.read(true).follow(FollowSymlinks::No);
            let file = parent.open_with(&name, &options).map_err(storage_error)?;
            let metadata = file.metadata().map_err(storage_error)?;
            if metadata.permissions().readonly() {
                return Err(Error::Forbidden);
            }
            #[cfg(windows)]
            {
                use cap_std::fs::MetadataExt;
                if metadata.file_attributes()
                    & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_ENCRYPTED
                    != 0
                {
                    return Err(Error::Forbidden);
                }
            }
            Some(permissions::read(&file)?)
        } else {
            None
        };
        let stage = format!(".cowork-import-{}", uuid::Uuid::new_v4());
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        #[cfg(windows)]
        {
            use cap_std::fs::OpenOptionsExt;
            use windows_sys::Win32::{
                Foundation::{GENERIC_READ, GENERIC_WRITE},
                Storage::FileSystem::WRITE_DAC,
            };
            options.access_mode(GENERIC_READ | GENERIC_WRITE | WRITE_DAC);
        }
        let mut file = parent.open_with(&stage, &options).map_err(storage_error)?;
        if let Some(state) = &before_permissions {
            permissions::apply(&file, state)?;
        }
        let bytes = read_regular_limit(&self.versions, &upload.file.sha256, MAX_DOWNLOAD_BYTES)?
            .ok_or(Error::NotFound)?;
        // Recheck after opening the retained source; a previous verification is not authorization to publish changed bytes.
        if hash(&bytes) != upload.file.sha256 || bytes.len() as u64 != upload.file.size {
            return Err(Error::FileConflict);
        }
        file.write_all(&bytes).map_err(storage_error)?;
        file.sync_all().map_err(storage_error)?;
        drop(file);
        let (after, entries) = inspect_name(&parent, &stage, &self.versions)?;
        if after.as_ref().map(|n| (&n.sha256, n.size))
            != Some((&upload.file.sha256, upload.file.size))
        {
            return Err(Error::FileConflict);
        }
        Ok(PreparedOperation {
            operation: FileOperation {
                run_id: run.id.clone(),
                tool_call_id: tool.id.clone(),
                folder_id: input.folder_id,
                kind: FileOperationKind::Import,
                path: input.path,
                destination: None,
                before,
                after,
                entries,
                restored_from: None,
                imported_from: Some(upload.clone()),
                generated_from: None,
            },
            root,
            versions: self.versions.clone(),
            source_parent: parent.try_clone().map_err(storage_error)?,
            source_name: stage,
            target_parent: parent,
            target_name: name,
            before_permissions,
            _lock: lock,
        })
    }

    /// Publishes document-engine output as one new file or directory. The runtime
    /// supplies bytes it staged and hash-checked itself; an existing target is never replaced.
    pub fn prepare_generated(
        &self,
        run: &RunRecord,
        tool: &ToolCall,
        folder_id: &str,
        path: &str,
        content: &GeneratedContent,
        provenance: DocumentProvenance,
    ) -> Result<PreparedOperation> {
        if !cowork_domain::document_write_tool(&tool.name) {
            return Err(Error::Forbidden);
        }
        let root = self.root(run, folder_id, true)?;
        let lock = self.lock(&root, true)?;
        run_workspace_limits::validate_path(&root, path)?;
        let (parent, name) = resolve_parent(&root.dir, path).map_err(|_| {
            Error::Validation("The output folder is missing or not accessible; create it with create_directory first".into())
        })?;
        if parent.symlink_metadata(&name).is_ok()
            || inspect_name(&parent, &name, &self.versions)?.0.is_some()
        {
            return Err(Error::FileConflict);
        }
        let growth = match content {
            GeneratedContent::File(bytes) => run_workspace_limits::Growth::tree(
                bytes.len() as u64,
                1,
            )
            .node_at(path, true, bytes.len() as u64, &[]),
            GeneratedContent::Directory(files) => {
                let mut paths = std::collections::BTreeSet::new();
                let mut total = 0u64;
                for (relative, bytes) in files {
                    let relative = relative_path(relative, false)?;
                    if bytes.len() as u64 > cowork_domain::workspace_transfer::MAX_FILE_BYTES {
                        return Err(Error::Forbidden);
                    }
                    run_workspace_limits::validate_path(
                        &root,
                        &format!("{path}/{}", relative.display()),
                    )?;
                    total = total
                        .checked_add(bytes.len() as u64)
                        .ok_or(Error::Forbidden)?;
                    let mut current = Some(relative.as_path());
                    while let Some(part) = current.filter(|p| !p.as_os_str().is_empty()) {
                        paths.insert(part.to_path_buf());
                        current = part.parent();
                    }
                }
                let entries: Vec<_> = paths
                    .iter()
                    .map(|part| {
                        let logical = part
                            .components()
                            .map(|c| c.as_os_str().to_string_lossy())
                            .collect::<Vec<_>>()
                            .join("/");
                        let file = files.iter().find(|(relative, _)| relative == &logical);
                        WorkspaceEntry {
                            path: logical,
                            sha256: file.map(|(_, bytes)| hash(bytes)),
                            size: file.map_or(0, |(_, bytes)| bytes.len() as u64),
                        }
                    })
                    .collect();
                run_workspace_limits::Growth::tree(
                    total,
                    paths.len().checked_add(1).ok_or(Error::Forbidden)?,
                )
                .node_at(path, false, 0, &entries)
            }
        };
        self.check_run_growth(&root, growth)?;
        let stage = format!(".cowork-generate-{}", uuid::Uuid::new_v4());
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        let expected = match content {
            GeneratedContent::File(bytes) => {
                let mut file = parent.open_with(&stage, &options).map_err(storage_error)?;
                file.write_all(bytes).map_err(storage_error)?;
                file.sync_all().map_err(storage_error)?;
                vec![(String::new(), hash(bytes))]
            }
            GeneratedContent::Directory(files) => {
                if files.is_empty() || files.len() > 256 {
                    return Err(Error::Validation(
                        "A generated folder needs 1-256 files".into(),
                    ));
                }
                parent.create_dir(&stage).map_err(storage_error)?;
                let directory = open_directory(&parent, Path::new(&stage))?;
                let mut expected = Vec::new();
                for (relative, bytes) in files {
                    let relative_file = relative_path(relative, false)?;
                    let mut current = directory.try_clone().map_err(storage_error)?;
                    if let Some(folders) = relative_file.parent() {
                        for component in folders.components() {
                            let std::path::Component::Normal(part) = component else {
                                continue;
                            };
                            if current.symlink_metadata(part).is_err() {
                                current.create_dir(part).map_err(storage_error)?;
                            }
                            current = open_directory(&current, Path::new(part))?;
                        }
                    }
                    let file_name = relative_file.file_name().ok_or(Error::Forbidden)?;
                    let mut file = current
                        .open_with(file_name, &options)
                        .map_err(storage_error)?;
                    file.write_all(bytes).map_err(storage_error)?;
                    file.sync_all().map_err(storage_error)?;
                    expected.push((relative.clone(), hash(bytes)));
                }
                #[cfg(unix)]
                sync_directory(&directory)?;
                expected
            }
        };
        let (after, entries) = inspect_name(&parent, &stage, &self.versions)?;
        let staged_matches = match content {
            GeneratedContent::File(_) => after
                .as_ref()
                .is_some_and(|n| n.kind == FileNodeKind::File && n.sha256 == expected[0].1),
            GeneratedContent::Directory(_) => {
                let mut observed: Vec<_> = entries
                    .iter()
                    .filter_map(|e| e.sha256.as_ref().map(|sha| (e.path.clone(), sha.clone())))
                    .collect();
                let mut wanted = expected.clone();
                observed.sort();
                wanted.sort();
                after
                    .as_ref()
                    .is_some_and(|n| n.kind == FileNodeKind::Directory)
                    && observed == wanted
            }
        };
        if !staged_matches {
            return Err(Error::FileConflict);
        }
        Ok(PreparedOperation {
            operation: FileOperation {
                run_id: run.id.clone(),
                tool_call_id: tool.id.clone(),
                folder_id: folder_id.into(),
                kind: FileOperationKind::Generate,
                path: path.into(),
                destination: None,
                before: None,
                after,
                entries,
                restored_from: None,
                imported_from: None,
                generated_from: Some(provenance),
            },
            root,
            versions: self.versions.clone(),
            source_parent: parent.try_clone().map_err(storage_error)?,
            source_name: stage,
            target_parent: parent,
            target_name: name,
            before_permissions: None,
            _lock: lock,
        })
    }

    pub fn stat(&self, run: &RunRecord, location: &FileLocation) -> Result<FileStat> {
        let root = self.root(run, &location.folder_id, false)?;
        let _lock = self.lock(&root, false)?;
        let (node, _) = inspect(&root.dir, &location.path, &self.versions)?;
        Ok(FileStat {
            folder_id: location.folder_id.clone(),
            path: location.path.clone(),
            node,
        })
    }

    pub fn prepare_operation(&self, run: &RunRecord, tool: &ToolCall) -> Result<PreparedOperation> {
        let (kind, folder, path, destination, expected) = match tool.name.as_str() {
            "create_directory" => {
                let input: FileLocation =
                    serde_json::from_value(tool.arguments.clone()).map_err(storage_error)?;
                (
                    FileOperationKind::CreateDirectory,
                    input.folder_id,
                    input.path,
                    None,
                    None,
                )
            }
            "rename_file" => {
                let input: RenameFile =
                    serde_json::from_value(tool.arguments.clone()).map_err(storage_error)?;
                (
                    FileOperationKind::Rename,
                    input.folder_id,
                    input.path,
                    Some(input.destination),
                    Some(input.expected_sha256),
                )
            }
            "remove_file" => {
                let input: RemoveFile =
                    serde_json::from_value(tool.arguments.clone()).map_err(storage_error)?;
                (
                    FileOperationKind::Remove,
                    input.folder_id,
                    input.path,
                    None,
                    Some(input.expected_sha256),
                )
            }
            _ => return Err(Error::Forbidden),
        };
        let root = self.root(run, &folder, true)?;
        let lock = self.lock(&root, true)?;
        run_workspace_limits::validate_path(&root, &path)?;
        if let Some(hash) = &expected {
            validate_hash(hash)?;
        }
        let (before, entries) = inspect(&root.dir, &path, &self.versions)?;
        if before.as_ref().map(|n| &n.sha256) != expected.as_ref() {
            return Err(Error::FileConflict);
        }
        let (parent, name) = resolve_parent(&root.dir, &path)?;
        let mut operation = FileOperation {
            run_id: run.id.clone(),
            tool_call_id: tool.id.clone(),
            folder_id: folder,
            kind: kind.clone(),
            path,
            destination,
            before: before.clone(),
            after: before,
            entries,
            restored_from: None,
            imported_from: None,
            generated_from: None,
        };
        let (source_parent, source_name, target_parent, target_name) = match kind {
            FileOperationKind::CreateDirectory => {
                self.check_run_growth(
                    &root,
                    run_workspace_limits::Growth::tree(0, 1).node_at(
                        &operation.path,
                        false,
                        0,
                        &[],
                    ),
                )?;
                let stage = format!(".cowork-directory-{}", uuid::Uuid::new_v4());
                parent.create_dir(&stage).map_err(storage_error)?;
                operation.after = inspect_name(&parent, &stage, &self.versions)?.0;
                (
                    parent.try_clone().map_err(storage_error)?,
                    stage,
                    parent,
                    name,
                )
            }
            FileOperationKind::Rename => {
                let destination = operation.destination.as_ref().ok_or(Error::Forbidden)?;
                run_workspace_limits::validate_path(&root, destination)?;
                for entry in &operation.entries {
                    run_workspace_limits::validate_path(
                        &root,
                        &format!("{destination}/{}", entry.path),
                    )?;
                }
                let node = operation.before.as_ref().ok_or(Error::FileConflict)?;
                self.check_run_growth(
                    &root,
                    run_workspace_limits::Growth::default()
                        .node_at(
                            destination,
                            node.kind == FileNodeKind::File,
                            node.size,
                            &operation.entries,
                        )
                        .remove_at(&operation.path),
                )?;
                let source_path = relative_path(&operation.path, false)?;
                let destination_path = relative_path(destination, false)?;
                if destination_path.starts_with(&source_path) {
                    return Err(Error::Forbidden);
                }
                let (target, target_name) = resolve_parent(&root.dir, destination)?;
                if target.symlink_metadata(&target_name).is_ok() {
                    return Err(Error::FileConflict);
                }
                // Inspect also distinguishes a missing target from access/IO errors.
                if inspect_name(&target, &target_name, &self.versions)?
                    .0
                    .is_some()
                {
                    return Err(Error::FileConflict);
                }
                (parent, name, target, target_name)
            }
            FileOperationKind::Remove => {
                operation.after = None;
                let tombstone = tombstone(&operation);
                if parent.symlink_metadata(&tombstone).is_ok() {
                    return Err(Error::FileConflict);
                }
                (
                    parent.try_clone().map_err(storage_error)?,
                    name,
                    parent,
                    tombstone,
                )
            }
            FileOperationKind::Restore
            | FileOperationKind::Import
            | FileOperationKind::Generate => {
                return Err(Error::Forbidden);
            }
        };
        Ok(PreparedOperation {
            operation,
            root,
            versions: self.versions.clone(),
            source_parent,
            source_name,
            target_parent,
            target_name,
            before_permissions: None,
            _lock: lock,
        })
    }

    pub fn reconcile_operation(
        &self,
        run: &RunRecord,
        operation: &FileOperation,
        completed: bool,
    ) -> Result<ResourceLock> {
        let root = self.reconciliation_root(run, &operation.folder_id)?;
        let lock = self.lock_named(&root.lock_name, true)?;
        if !matches_state(&root.dir, &self.versions, operation, completed)? {
            return Err(Error::FileConflict);
        }
        Ok(lock)
    }
}

impl PreparedOperation {
    /// Caller must have persisted the exact in-flight operation before invoking this.
    pub fn apply(&self) -> Result<()> {
        if !matches_state(&self.root.dir, &self.versions, &self.operation, false)? {
            return Err(Error::FileConflict);
        }
        if matches!(
            self.operation.kind,
            FileOperationKind::Import
                | FileOperationKind::Restore
                | FileOperationKind::CreateDirectory
                | FileOperationKind::Generate
        ) && inspect_name(&self.source_parent, &self.source_name, &self.versions)?.0
            != self.operation.after
        {
            return Err(Error::FileConflict);
        }
        if let Some(expected) = &self.before_permissions {
            for (parent, name) in [
                (&self.target_parent, &self.target_name),
                (&self.source_parent, &self.source_name),
            ] {
                let mut options = OpenOptions::new();
                options.read(true).follow(FollowSymlinks::No);
                let file = parent.open_with(name, &options).map_err(storage_error)?;
                if permissions::read(&file)? != *expected {
                    return Err(Error::FileConflict);
                }
            }
        }
        if self.operation.kind == FileOperationKind::Import && self.operation.before.is_some() {
            self.source_parent
                .rename(&self.source_name, &self.target_parent, &self.target_name)
                .map_err(|_| Error::UncertainEffect)?;
        } else {
            rename::no_replace(
                &self.source_parent,
                &self.source_name,
                &self.target_parent,
                &self.target_name,
            )?;
        }
        #[cfg(unix)]
        {
            sync_directory(&self.source_parent).map_err(|_| Error::UncertainEffect)?;
            sync_directory(&self.target_parent).map_err(|_| Error::UncertainEffect)?;
        }
        if !matches_state(&self.root.dir, &self.versions, &self.operation, true)
            .map_err(|_| Error::UncertainEffect)?
        {
            return Err(Error::UncertainEffect);
        }
        Ok(())
    }
}

impl Drop for PreparedOperation {
    fn drop(&mut self) {
        if self.operation.kind == FileOperationKind::Generate {
            // Only this operation's own unique hidden stage is ever removed.
            if self.source_name.starts_with(".cowork-generate-")
                && let Ok(metadata) = self.source_parent.symlink_metadata(&self.source_name)
                && !metadata.is_symlink()
            {
                let _ = if metadata.is_dir() {
                    self.source_parent.remove_dir_all(&self.source_name)
                } else {
                    self.source_parent.remove_file(&self.source_name)
                };
            }
            return;
        }
        if self.operation.kind != FileOperationKind::Import {
            return;
        }
        if self
            .source_parent
            .symlink_metadata(&self.source_name)
            .is_ok_and(|m| {
                m.is_file()
                    && !m.is_symlink()
                    && m.nlink() == 1
                    && self.operation.after.as_ref().is_some_and(|a| {
                        a.resource_id == hash(format!("{}-{}", m.dev(), m.ino()).as_bytes())
                    })
            })
        {
            let _ = self.source_parent.remove_file(&self.source_name);
        }
    }
}

fn matches_state(
    root: &Dir,
    versions: &Dir,
    operation: &FileOperation,
    completed: bool,
) -> Result<bool> {
    let (original, _) = inspect(root, &operation.path, versions)?;
    let expected_original = if completed
        && !matches!(
            operation.kind,
            FileOperationKind::CreateDirectory
                | FileOperationKind::Restore
                | FileOperationKind::Import
                | FileOperationKind::Generate
        ) {
        None
    } else if completed {
        operation.after.clone()
    } else {
        operation.before.clone()
    };
    if original != expected_original {
        return Ok(false);
    }
    if let Some(destination) = &operation.destination {
        let target = inspect(root, destination, versions)?.0;
        if target
            != if completed {
                operation.after.clone()
            } else {
                None
            }
        {
            return Ok(false);
        }
    }
    if operation.kind == FileOperationKind::Remove {
        let (parent, _) = resolve_parent(root, &operation.path)?;
        let retained = inspect_name(&parent, &tombstone(operation), versions)?.0;
        if retained
            != if completed {
                operation.before.clone()
            } else {
                None
            }
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn tombstone(operation: &FileOperation) -> String {
    format!(
        ".cowork-deleted-{}",
        canonical_hash(&json!([operation.run_id, operation.tool_call_id]))
    )
}
fn resolve_parent(root: &Dir, path: &str) -> Result<(Dir, String)> {
    let path = relative_path(path, false)?;
    let parent = open_directory(root, path.parent().unwrap_or(Path::new(".")))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(Error::Forbidden)?
        .to_owned();
    Ok((parent, name))
}
pub(super) fn inspect(
    root: &Dir,
    path: &str,
    versions: &Dir,
) -> Result<(Option<FileNode>, Vec<WorkspaceEntry>)> {
    let (parent, name) = resolve_parent(root, path)?;
    inspect_name(&parent, &name, versions)
}
fn inspect_name(
    parent: &Dir,
    name: &str,
    versions: &Dir,
) -> Result<(Option<FileNode>, Vec<WorkspaceEntry>)> {
    let metadata = match parent.symlink_metadata(name) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((None, vec![])),
        Err(e) => return Err(storage_error(e)),
    };
    if metadata.is_symlink() {
        return Err(Error::Forbidden);
    }
    let resource_id = hash(format!("{}-{}", metadata.dev(), metadata.ino()).as_bytes());
    let mut entries = Vec::new();
    let (kind, sha256, size) = if metadata.is_dir() {
        let directory = open_directory(parent, Path::new(name))?;
        let mut size = 0;
        workspaces::capture_directory(versions, &directory, "", 0, &mut size, &mut entries)?;
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        (FileNodeKind::Directory, canonical_hash(&entries), size)
    } else {
        let bytes = read_regular_limit(parent, name, workspaces::MAX_SNAPSHOT_FILE)?
            .ok_or(Error::FileConflict)?;
        save_version(versions, &bytes)?;
        (FileNodeKind::File, hash(&bytes), bytes.len() as u64)
    };
    Ok((
        Some(FileNode {
            kind,
            sha256,
            size,
            resource_id,
        }),
        entries,
    ))
}
