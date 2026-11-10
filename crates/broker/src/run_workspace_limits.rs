//! Limits for the trusted, process-free broker adapter. This is not a process
//! sandbox or a replacement for the runtime's reserved filesystem volumes.
use super::*;
use cowork_domain::workspace_transfer as transfer;

#[derive(Default)]
pub(super) struct Growth {
    pub old_bytes: u64,
    pub new_bytes: u64,
    pub old_entries: usize,
    pub new_entries: usize,
    pub staging_bytes: u64,
    pub staging_entries: usize,
    remove: String,
    projection: Vec<WorkspaceEntry>,
}
impl Growth {
    pub fn file(before: Option<u64>, after: u64) -> Self {
        Self {
            old_bytes: before.unwrap_or(0),
            new_bytes: after,
            old_entries: usize::from(before.is_some()),
            new_entries: 1,
            staging_bytes: after,
            staging_entries: 1,
            ..Self::default()
        }
    }
    pub fn node_at(
        mut self,
        path: &str,
        file: bool,
        size: u64,
        children: &[WorkspaceEntry],
    ) -> Self {
        self.remove = path.into();
        self.projection = vec![WorkspaceEntry {
            path: path.into(),
            sha256: file.then(|| "0".repeat(64)),
            size,
        }];
        self.projection
            .extend(children.iter().map(|entry| WorkspaceEntry {
                path: format!("{path}/{}", entry.path),
                sha256: entry.sha256.clone(),
                size: entry.size,
            }));
        self
    }
    pub fn remove_at(mut self, path: &str) -> Self {
        self.remove = path.into();
        self
    }
    pub fn tree(bytes: u64, entries: usize) -> Self {
        Self {
            new_bytes: bytes,
            new_entries: entries,
            staging_bytes: bytes,
            staging_entries: entries,
            ..Self::default()
        }
    }
}

#[derive(Default)]
struct Usage {
    visible_bytes: u64,
    visible_entries: usize,
    bytes: u64,
    entries: usize,
    catalog: BTreeMap<String, (bool, u64)>,
}
fn limit() -> Error {
    Error::Validation("run_workspace_capacity_exceeded".into())
}

fn scan(dir: &Dir, prefix: &str, hidden: bool, depth: usize, usage: &mut Usage) -> Result<()> {
    if depth > 65 {
        return Err(limit());
    }
    for entry in dir.entries().map_err(storage_error)? {
        let entry = entry.map_err(storage_error)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| Error::Forbidden)?;
        let path = format!("{prefix}/{name}");
        let hidden = hidden || name.to_ascii_lowercase().starts_with(".cowork-");
        if !hidden {
            transfer::validate_path(&path)?;
        }
        let meta = dir.symlink_metadata(&name).map_err(storage_error)?;
        if meta.is_symlink() || !(meta.is_file() || meta.is_dir()) {
            return Err(Error::Forbidden);
        }
        usage.entries = usage.entries.checked_add(1).ok_or_else(limit)?;
        if !hidden {
            usage.visible_entries = usage.visible_entries.checked_add(1).ok_or_else(limit)?;
            usage.catalog.insert(
                path.clone(),
                (meta.is_file(), if meta.is_file() { meta.len() } else { 0 }),
            );
        }
        if usage.entries > transfer::MAX_ENTRIES * 2 + 64 {
            return Err(limit());
        }
        if meta.is_dir() {
            let child = open_directory(dir, Path::new(&name))?;
            scan(&child, &path, hidden, depth + 1, usage)?;
        } else {
            if meta.nlink() != 1 || meta.len() > transfer::MAX_FILE_BYTES {
                return Err(limit());
            }
            usage.bytes = usage.bytes.checked_add(meta.len()).ok_or_else(limit)?;
            if !hidden {
                usage.visible_bytes = usage
                    .visible_bytes
                    .checked_add(meta.len())
                    .ok_or_else(limit)?;
            }
            if usage.bytes > transfer::MAX_TOTAL_BYTES + transfer::MAX_FILE_BYTES {
                return Err(limit());
            }
        }
    }
    Ok(())
}

impl FileBroker {
    /// Caller holds the namespace-wide OS writer lock. Hidden partial writes
    /// and tombstones stay charged; an uncertain operation cannot free capacity.
    pub(super) fn check_run_growth(&self, root: &Root, growth: Growth) -> Result<()> {
        let Some(binding) = &root.namespace else {
            return Ok(());
        };
        let roots = self.run_roots.read().map_err(|_| Error::Forbidden)?;
        let roots = roots.get(&binding.id).ok_or(Error::Forbidden)?;
        let mut usage = Usage::default();
        let mut target_prefix = None;
        // Include aliases in the portable entry limit. All aliases share this
        // same lock and quota, including those absent from a delegated role.
        for (index, other) in roots.values().enumerate() {
            if other.namespace.as_ref() != Some(binding) {
                return Err(Error::Forbidden);
            }
            usage.entries += 1;
            usage.visible_entries += 1;
            let prefix = format!("input-{:04}", index + 1);
            usage.catalog.insert(prefix.clone(), (false, 0));
            if other.grant.id == root.grant.id {
                target_prefix = Some(prefix.clone());
            }
            scan(&other.dir, &prefix, false, 1, &mut usage)?;
        }
        let bytes = usage
            .visible_bytes
            .checked_sub(growth.old_bytes)
            .and_then(|v| v.checked_add(growth.new_bytes))
            .ok_or_else(limit)?;
        let entries = usage
            .visible_entries
            .checked_sub(growth.old_entries)
            .and_then(|v| v.checked_add(growth.new_entries))
            .ok_or_else(limit)?;
        let peak_bytes = usage
            .bytes
            .checked_add(growth.staging_bytes)
            .ok_or_else(limit)?;
        let peak_entries = usage
            .entries
            .checked_add(growth.staging_entries)
            .ok_or_else(limit)?;
        if bytes > transfer::MAX_TOTAL_BYTES
            || entries > transfer::MAX_ENTRIES
            || peak_bytes > transfer::MAX_TOTAL_BYTES + transfer::MAX_FILE_BYTES
            || peak_entries > transfer::MAX_ENTRIES * 2 + 64
        {
            return Err(limit());
        }
        // Check the complete projected portable metadata before staging. Dummy
        // digests have the exact encoded length of real digests; content is
        // hashed separately at capture/publication. No file bytes are read here.
        let prefix = target_prefix.ok_or(Error::Forbidden)?;
        transfer::validate_path(&growth.remove)?;
        let removed = format!("{prefix}/{}", growth.remove);
        usage
            .catalog
            .retain(|path, _| path != &removed && !path.starts_with(&format!("{removed}/")));
        for entry in &growth.projection {
            let path = format!("{prefix}/{}", entry.path);
            transfer::validate_path(&path)?;
            if usage
                .catalog
                .insert(path, (entry.sha256.is_some(), entry.size))
                .is_some()
            {
                return Err(Error::FileConflict);
            }
        }
        let mut directories = Vec::new();
        let mut files = Vec::new();
        for (path, (file, size)) in usage.catalog {
            if !file {
                directories.push(path);
                continue;
            }
            if size > transfer::MAX_FILE_BYTES {
                return Err(limit());
            }
            let mut chunks = Vec::new();
            let mut offset = 0;
            while offset < size {
                let length = (size - offset).min(transfer::CHUNK_BYTES);
                chunks.push(WorkspaceTransferChunk {
                    offset,
                    size: length,
                    sha256: "0".repeat(64),
                });
                offset += length;
            }
            files.push(WorkspaceTransferFile {
                path,
                size,
                sha256: "0".repeat(64),
                chunks,
            });
        }
        transfer::build(directories, files).map_err(|_| limit())?;
        Ok(())
    }
}

pub(super) fn validate_path(root: &Root, path: &str) -> Result<()> {
    if root.namespace.is_some() {
        // Input aliases use ten bytes; output is shorter. Reserving the longer
        // prefix prevents a later result snapshot from exceeding path limits.
        transfer::validate_path(&format!("input-0001/{path}"))?;
        let path = relative_path(path, false)?;
        let parts: Vec<_> = path.components().collect();
        let mut current = root.dir.try_clone().map_err(storage_error)?;
        for (index, part) in parts.iter().enumerate() {
            let std::path::Component::Normal(name) = part else {
                return Err(Error::Forbidden);
            };
            let name = name.to_str().ok_or(Error::Forbidden)?;
            let key = name.to_lowercase().to_uppercase();
            let mut exists = false;
            for sibling in current.entries().map_err(storage_error)? {
                let sibling = sibling
                    .map_err(storage_error)?
                    .file_name()
                    .into_string()
                    .map_err(|_| Error::Forbidden)?;
                if sibling.to_lowercase().to_uppercase() == key {
                    if sibling != name {
                        return Err(Error::FileConflict);
                    }
                    exists = true;
                }
            }
            if !exists || index + 1 == parts.len() {
                break;
            }
            current = open_directory(&current, Path::new(name))?;
        }
    }
    Ok(())
}
