//! Human-selected native file delivery. No agent tool exposes this ambient-path entry point.
use super::*;
use sha2::{Digest, Sha256};
pub(super) mod permissions;

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileState {
    identity: (u64, u64),
    sha256: String,
    size: u64,
    permissions: permissions::PermissionState,
}

fn file_state_limit(parent: &Dir, name: &str, limit: u64) -> Result<Option<FileState>> {
    let before = match parent.symlink_metadata(name) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(storage_error(error)),
    };
    if before.is_symlink()
        || !before.is_file()
        || before.nlink() != 1
        || before.permissions().readonly()
        || before.len() > limit
    {
        return Err(Error::Forbidden);
    }
    #[cfg(windows)]
    {
        use cap_std::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_ENCRYPTED;
        // A plain staged replacement must never remove EFS protection from an existing file.
        if before.file_attributes() & FILE_ATTRIBUTE_ENCRYPTED != 0 {
            return Err(Error::Forbidden);
        }
    }

    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let mut file = parent.open_with(name, &options).map_err(storage_error)?;
    let opened = file.metadata().map_err(storage_error)?;
    if (before.dev(), before.ino()) != (opened.dev(), opened.ino()) {
        return Err(Error::FileConflict);
    }
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut chunk = [0u8; 65536];
    loop {
        let n = file.read(&mut chunk).map_err(storage_error)?;
        if n == 0 {
            break;
        }
        size = size.checked_add(n as u64).ok_or(Error::Forbidden)?;
        if size > limit {
            return Err(Error::Forbidden);
        }
        hasher.update(&chunk[..n]);
    }
    let after = parent.symlink_metadata(name).map_err(storage_error)?;
    if after.is_symlink()
        || (before.dev(), before.ino()) != (after.dev(), after.ino())
        || before.len() != after.len()
        || size != after.len()
        || before.modified().ok() != after.modified().ok()
        || (opened.dev(), opened.ino()) != (after.dev(), after.ino())
    {
        return Err(Error::FileConflict);
    }
    Ok(Some(FileState {
        identity: (after.dev(), after.ino()),
        sha256: format!("{:x}", hasher.finalize()),
        size,
        permissions: permissions::read(&file)?,
    }))
}

pub fn validate_delivery(bytes: &[u8], metadata: &DownloadMetadata) -> Result<()> {
    let name = relative_path(&metadata.file_name, false)?;
    if name.components().count() != 1
        || bytes.len() as u64 > MAX_DOWNLOAD_BYTES
        || metadata.size != bytes.len() as u64
    {
        return Err(Error::Validation("Invalid file delivery metadata".into()));
    }
    validate_hash(&metadata.sha256)?;
    if hash(bytes) != metadata.sha256 {
        return Err(Error::Validation(
            "File delivery integrity check failed".into(),
        ));
    }
    Ok(())
}

/// Keeps the selected directory pinned and records the exact destination observed after selection.
/// Dropping an unpublished delivery removes only its own temporary regular file when still owned.
pub struct PreparedDelivery {
    parent: Dir,
    parent_path: PathBuf,
    parent_identity: (u64, u64),
    name: String,
    stage: String,
    staged: FileState,
    before: Option<FileState>,
    metadata: DownloadMetadata,
    limit: u64,
}

impl PreparedDelivery {
    /// `target` must come from a native user-controlled save dialog, never frontend JSON.
    pub fn prepare(
        target: &Path,
        bytes: &[u8],
        metadata: &DownloadMetadata,
        protected: &[PathBuf],
    ) -> Result<Self> {
        validate_delivery(bytes, metadata)?;
        Self::prepare_stream_inner(
            target,
            &mut std::io::Cursor::new(bytes),
            metadata,
            protected,
            MAX_DOWNLOAD_BYTES,
        )
    }
    /// Backup-only streaming delivery; normal file/tool deliveries retain their
    /// 64 MiB cap. Caller obtains source bytes from a verified owned backup ID.
    pub fn prepare_backup(
        target: &Path,
        source: &mut impl Read,
        metadata: &DownloadMetadata,
        protected: &[PathBuf],
    ) -> Result<Self> {
        if metadata.size == 0
            || metadata.size > MAX_DATABASE_BACKUP_BYTES
            || relative_path(&metadata.file_name, false)?
                .components()
                .count()
                != 1
        {
            return Err(Error::Forbidden);
        }
        validate_hash(&metadata.sha256)?;
        Self::prepare_stream_inner(
            target,
            source,
            metadata,
            protected,
            MAX_DATABASE_BACKUP_BYTES,
        )
    }
    fn prepare_stream_inner(
        target: &Path,
        source: &mut impl Read,
        metadata: &DownloadMetadata,
        protected: &[PathBuf],
        limit: u64,
    ) -> Result<Self> {
        if !target.is_absolute() {
            return Err(Error::Forbidden);
        }
        let name = target
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or(Error::Forbidden)?
            .to_owned();
        if relative_path(&name, false)?.components().count() != 1 {
            return Err(Error::Forbidden);
        }
        let parent_path = std::fs::canonicalize(target.parent().ok_or(Error::Forbidden)?)
            .map_err(storage_error)?;
        let comparison = comparison_path(&parent_path)?.join(if cfg!(windows) {
            name.to_lowercase()
        } else {
            name.clone()
        });
        for root in protected {
            if comparison.starts_with(comparison_path(root)?) {
                return Err(Error::Forbidden);
            }
        }
        let parent =
            Dir::open_ambient_dir(&parent_path, ambient_authority()).map_err(storage_error)?;
        let native = parent.dir_metadata().map_err(storage_error)?;
        let parent_identity = (native.dev(), native.ino());
        let before = file_state_limit(&parent, &name, limit)?;
        let stage = format!(".cowork-save-{}", uuid::Uuid::new_v4());
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
        let staged_identity = file.metadata().map_err(storage_error)?;
        let mut prepared = Self {
            limit,
            parent,
            parent_path,
            parent_identity,
            name: name.clone(),
            stage,
            staged: FileState {
                identity: (staged_identity.dev(), staged_identity.ino()),
                sha256: metadata.sha256.clone(),
                size: metadata.size,
                permissions: permissions::read(&file)?,
            },
            before,
            metadata: DownloadMetadata {
                file_name: name,
                size: metadata.size,
                sha256: metadata.sha256.clone(),
            },
        };
        if let Some(before) = &prepared.before {
            permissions::apply(&file, &before.permissions)?;
            prepared.staged.permissions = permissions::read(&file)?;
            if prepared.staged.permissions != before.permissions {
                return Err(Error::Forbidden);
            }
        }
        let mut hasher = Sha256::new();
        let mut size = 0u64;
        let mut chunk = [0u8; 65536];
        loop {
            let n = source.read(&mut chunk).map_err(storage_error)?;
            if n == 0 {
                break;
            }
            size = size.checked_add(n as u64).ok_or(Error::Forbidden)?;
            if size > metadata.size {
                return Err(Error::Validation("Backup delivery length mismatch".into()));
            }
            hasher.update(&chunk[..n]);
            file.write_all(&chunk[..n]).map_err(storage_error)?;
        }
        if size != metadata.size || format!("{:x}", hasher.finalize()) != metadata.sha256 {
            return Err(Error::Validation(
                "Backup delivery integrity mismatch".into(),
            ));
        }
        file.sync_all().map_err(storage_error)?;
        drop(file);
        if file_state_limit(&prepared.parent, &prepared.stage, prepared.limit)?.as_ref()
            != Some(&prepared.staged)
        {
            return Err(Error::FileConflict);
        }
        Ok(prepared)
    }

    pub fn commit(mut self) -> Result<DownloadMetadata> {
        if std::fs::canonicalize(&self.parent_path).map_err(storage_error)? != self.parent_path {
            return Err(Error::FileConflict);
        }
        let current_parent =
            Dir::open_ambient_dir(&self.parent_path, ambient_authority()).map_err(storage_error)?;
        let current = current_parent.dir_metadata().map_err(storage_error)?;
        if (current.dev(), current.ino()) != self.parent_identity
            || file_state_limit(&self.parent, &self.name, self.limit)? != self.before
            || file_state_limit(&self.parent, &self.stage, self.limit)?.as_ref()
                != Some(&self.staged)
        {
            return Err(Error::FileConflict);
        }
        if self.before.is_none() {
            // The kernel refuses a destination which appeared since selection.
            rename::no_replace(&self.parent, &self.stage, &self.parent, &self.name).map_err(
                |error| {
                    if matches!(error, Error::FileConflict) {
                        error
                    } else {
                        Error::UncertainEffect
                    }
                },
            )?;
        } else {
            // User selected and confirmed an existing destination in the native save dialog.
            self.parent
                .rename(&self.stage, &self.parent, &self.name)
                .map_err(|_| Error::UncertainEffect)?;
        }
        self.stage.clear();
        #[cfg(unix)]
        sync_directory(&self.parent).map_err(|_| Error::UncertainEffect)?;
        let published = file_state_limit(&self.parent, &self.name, self.limit)
            .map_err(|_| Error::UncertainEffect)?;
        if published.as_ref() != Some(&self.staged) {
            return Err(Error::UncertainEffect);
        }
        Ok(self.metadata.clone())
    }
}

impl Drop for PreparedDelivery {
    fn drop(&mut self) {
        if self.stage.is_empty() {
            return;
        }
        if self.parent.symlink_metadata(&self.stage).is_ok_and(|m| {
            m.is_file()
                && !m.is_symlink()
                && m.nlink() == 1
                && (m.dev(), m.ino()) == self.staged.identity
        }) {
            let _ = self.parent.remove_file(&self.stage);
        }
    }
}
