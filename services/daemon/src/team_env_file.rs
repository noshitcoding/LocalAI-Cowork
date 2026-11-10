//! Private ephemeral Docker env-file. No secret enters argv or tool journals.
//! Removal follows confirmed cleanup of its exact owned container only.
#[cfg(windows)]
use crate::private_secret_file;
use cowork_domain::{Error, Result};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
use uuid::Uuid;
use zeroize::Zeroizing;

pub(super) struct EnvFile {
    path: PathBuf,
    file: Option<File>,
    _namespace: Vec<File>,
}
fn unavailable() -> Error {
    Error::NativeKeyUnavailable
}
fn ordinary(file: &File) -> Result<()> {
    let meta = file.metadata().map_err(|_| unavailable())?;
    if !meta.is_file() || meta.is_symlink() {
        return Err(unavailable());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err(unavailable());
        }
        private_secret_file::validate(file).map_err(|_| unavailable())?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() != 1 || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0
        {
            return Err(unavailable());
        }
    }
    Ok(())
}
pub(super) fn path(root: &Path, nonce: Uuid) -> PathBuf {
    root.join(format!("gh-{nonce}.env"))
}
impl EnvFile {
    pub(super) fn create(root: &Path, nonce: Uuid, hostname: &str, token: &str) -> Result<Self> {
        cowork_secrets::validate_bearer(token).map_err(|_| unavailable())?;
        if hostname.is_empty()
            || !hostname
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
        {
            return Err(unavailable());
        }
        #[cfg(windows)]
        let mut namespace = Vec::new();
        #[cfg(not(windows))]
        let namespace = Vec::new();
        for ancestor in root.ancestors() {
            let meta = std::fs::symlink_metadata(ancestor).map_err(|_| unavailable())?;
            if !meta.is_dir() || meta.is_symlink() {
                return Err(unavailable());
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
                if meta.file_attributes() & 0x400 != 0 {
                    return Err(unavailable());
                }
                let file = OpenOptions::new()
                    .read(true)
                    .access_mode(0x0002_0081)
                    .share_mode(3)
                    .custom_flags(0x0220_0000)
                    .open(ancestor)
                    .map_err(|_| unavailable())?;
                if ancestor == root {
                    private_secret_file::validate_directory(&file).map_err(|_| unavailable())?;
                }
                namespace.push(file);
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let uid = unsafe { libc::geteuid() };
                if (meta.uid() != uid && meta.uid() != 0)
                    || (meta.mode() & 0o022 != 0 && meta.mode() & 0o1000 == 0)
                {
                    return Err(unavailable());
                }
            }
        }
        let path = path(root, nonce);
        #[cfg(windows)]
        let mut file =
            private_secret_file::create_private_file(&path).map_err(|_| unavailable())?;
        #[cfg(unix)]
        let mut file = {
            use std::os::unix::fs::OpenOptionsExt;
            OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&path)
                .map_err(|_| unavailable())?
        };
        ordinary(&file)?;
        let content = Zeroizing::new(format!("GH_TOKEN={token}\nGH_HOST={hostname}\n"));
        file.write_all(content.as_bytes())
            .map_err(|_| unavailable())?;
        file.sync_all().map_err(|_| unavailable())?;
        Ok(Self {
            path,
            file: Some(file),
            _namespace: namespace,
        })
    }
    pub(super) fn remove(mut self) -> Result<()> {
        ordinary(self.file.as_ref().ok_or_else(unavailable)?)?;
        // The private owner directory and held file deny foreign replacement.
        // Windows requires releasing its delete-denying handle before unlink.
        self.file.take();
        let meta = std::fs::symlink_metadata(&self.path).map_err(|_| unavailable())?;
        if !meta.is_file() || meta.is_symlink() {
            return Err(unavailable());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & 0x400 != 0 {
                return Err(unavailable());
            }
        }
        std::fs::remove_file(&self.path).map_err(|_| unavailable())
    }
}
