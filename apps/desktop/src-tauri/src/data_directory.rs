//! Prepare the Windows data root before any update receipt or daemon startup.
use std::{
    fs::{File, OpenOptions},
    io,
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
    sync::Mutex,
};

#[derive(Default)]
pub(super) struct Root(Mutex<Option<Held>>);
struct Held {
    path: PathBuf,
    // Deny namespace replacement for the desktop lifetime. The daemon retains
    // its own independent checks and custody before creating any authority.
    pins: Vec<File>,
}
fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "V2 data directory is not private",
    )
}
impl Root {
    pub(super) fn prepare(&self, path: &Path) -> io::Result<()> {
        if !path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, Component::ParentDir))
        {
            return Err(invalid());
        }
        let mut held = self.0.lock().map_err(|_| invalid())?;
        if let Some(original) = held.as_ref() {
            if original.path != path {
                return Err(invalid());
            }
            return super::private_secret_file::validate_directory(
                original.pins.last().ok_or_else(invalid)?,
            );
        }
        let mut pins = Vec::new();
        for directory in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
            match std::fs::symlink_metadata(directory) {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    // TokenUser owner and the private ACL are supplied atomically
                    // to CREATE_NEW. Never repair or take ownership of an old root.
                    match super::private_secret_file::create_private_directory(directory) {
                        Ok(()) => {}
                        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                        Err(error) => return Err(error),
                    }
                }
                Err(error) => return Err(error),
            }
            let file = OpenOptions::new()
                // LIST_DIRECTORY is intentional: metadata-only handles do not
                // reliably establish Windows delete-sharing custody.
                .access_mode(0x0002_0081)
                .share_mode(3)
                .custom_flags(0x0220_0000)
                .open(directory)?;
            let metadata = file.metadata()?;
            if !metadata.is_dir() || metadata.file_attributes() & 0x400 != 0 {
                return Err(invalid());
            }
            pins.push(file);
        }
        super::private_secret_file::validate_directory(pins.last().ok_or_else(invalid)?)?;
        *held = Some(Held {
            path: path.into(),
            pins,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn new_nested_root_is_private_before_update_journal_and_stays_pinned() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture
            .path()
            .join("profile parent ä & spaces")
            .join("data");
        let guard = Root::default();
        guard.prepare(&path).unwrap();
        guard.prepare(&path).unwrap();
        std::fs::create_dir_all(path.join("update-receipts")).unwrap();
        assert!(std::fs::rename(&path, fixture.path().join("substituted")).is_err());
        assert!(
            guard
                .prepare(&fixture.path().join("another-profile"))
                .is_err()
        );
        drop(guard);
    }
    #[test]
    fn existing_untrusted_root_is_rejected_without_acl_repair() {
        use std::{os::windows::io::AsRawHandle, ptr};
        use windows_sys::Win32::Security::{
            Authorization::{SE_FILE_OBJECT, SetSecurityInfo},
            DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
        };
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("untrusted-data");
        super::super::private_secret_file::create_private_directory(&path).unwrap();
        let original = OpenOptions::new()
            .access_mode(0x0006_0080)
            .share_mode(3)
            .custom_flags(0x0220_0000)
            .open(&path)
            .unwrap();
        // Only this new disposable directory: a null DACL must remain rejected.
        assert_eq!(
            unsafe {
                SetSecurityInfo(
                    original.as_raw_handle(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            },
            0
        );
        assert!(Root::default().prepare(&path).is_err());
        assert!(super::super::private_secret_file::validate_directory(&original).is_err());
        assert!(!path.join("update-receipts").exists());
        drop(original);
    }
    #[test]
    fn files_and_parent_traversal_never_become_data_roots() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("original-file");
        std::fs::write(&path, b"original").unwrap();
        assert!(Root::default().prepare(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert!(
            Root::default()
                .prepare(&fixture.path().join("..").join("other"))
                .is_err()
        );
    }
}
