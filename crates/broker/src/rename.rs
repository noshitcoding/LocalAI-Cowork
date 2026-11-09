//! Atomic publication with an OS-enforced absent destination. No replacement fallback.
use super::*;

#[cfg(target_os = "linux")]
pub(super) fn no_replace(source: &Dir, name: &str, target: &Dir, destination: &str) -> Result<()> {
    use std::{ffi::CString, os::fd::AsRawFd};
    let name = CString::new(name).map_err(|_| Error::Forbidden)?;
    let destination = CString::new(destination).map_err(|_| Error::Forbidden)?;
    // SAFETY: both descriptors are live pinned directory capabilities; both names are
    // validated single path components. The syscall never replaces an existing target.
    let result = unsafe {
        libc::renameat2(
            source.as_raw_fd(),
            name.as_ptr(),
            target.as_raw_fd(),
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::AlreadyExists {
        Err(Error::FileConflict)
    } else {
        Err(storage_error(error))
    }
}

#[cfg(windows)]
pub(super) fn no_replace(source: &Dir, name: &str, target: &Dir, destination: &str) -> Result<()> {
    use cap_std::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Wdk::Storage::FileSystem::{
        FILE_RENAME_INFORMATION, FileRenameInformation, NtSetInformationFile,
    };
    use windows_sys::Win32::{
        Foundation::RtlNtStatusToDosError, Storage::FileSystem::*, System::IO::IO_STATUS_BLOCK,
    };
    let mut options = OpenOptions::new();
    options
        .access_mode(DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .follow(FollowSymlinks::No);
    let file = source.open_with(name, &options).map_err(storage_error)?;
    if file.metadata().map_err(storage_error)?.is_symlink() {
        return Err(Error::Forbidden);
    }
    let encoded: Vec<u16> = destination
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let size = std::mem::offset_of!(FILE_RENAME_INFORMATION, FileName) + encoded.len() * 2;
    // usize provides the required native alignment; zeroed padding and trailing NUL are retained.
    let mut buffer = vec![0usize; size.div_ceil(std::mem::size_of::<usize>())];
    // SAFETY: aligned allocation has room for the header and complete UTF-16 filename.
    // The borrowed file and target directory handles remain alive throughout the call.
    let mut io_status = IO_STATUS_BLOCK::default();
    let status = unsafe {
        let allocation = buffer.as_mut_ptr().cast::<u8>();
        let info = allocation.cast::<FILE_RENAME_INFORMATION>();
        (*info).Anonymous.ReplaceIfExists = false;
        (*info).RootDirectory = target.as_raw_handle();
        (*info).FileNameLength = ((encoded.len() - 1) * 2) as u32;
        std::ptr::copy_nonoverlapping(
            encoded.as_ptr(),
            // Derive the flexible tail from the complete allocation. Borrowing
            // the declared [u16; 1] field would cover only its first element.
            allocation
                .add(std::mem::offset_of!(FILE_RENAME_INFORMATION, FileName))
                .cast::<u16>(),
            encoded.len(),
        );
        NtSetInformationFile(
            file.as_raw_handle(),
            &mut io_status,
            info.cast(),
            size as u32,
            FileRenameInformation,
        )
    };
    if status == 0 {
        return Ok(());
    }
    // SAFETY: pure NTSTATUS-to-Win32 conversion; the source handle uses synchronous I/O.
    let error = std::io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(status) } as i32);
    if error.kind() == std::io::ErrorKind::AlreadyExists {
        Err(Error::FileConflict)
    } else {
        Err(storage_error(error))
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
pub(super) fn no_replace(_: &Dir, _: &str, _: &Dir, _: &str) -> Result<()> {
    Err(Error::Forbidden)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publication_preserves_complete_unicode_names_across_path_lengths() {
        let temp = tempfile::tempdir().unwrap();
        for padding in 0..16 {
            let root = temp.path().join(format!("path-{}", "a".repeat(padding)));
            std::fs::create_dir(&root).unwrap();
            let destination = format!("ausgabe-{}-Prüfung-📄.bin", "x".repeat(padding));
            std::fs::write(root.join("source"), b"exact publication").unwrap();
            let dir = Dir::open_ambient_dir(&root, ambient_authority()).unwrap();
            no_replace(&dir, "source", &dir, &destination).unwrap();
            assert_eq!(
                std::fs::read(root.join(&destination)).unwrap(),
                b"exact publication"
            );
            assert!(!root.join("source").exists());
            let entries: Vec<_> = std::fs::read_dir(&root)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            assert_eq!(entries, [std::ffi::OsString::from(&destination)]);
            std::fs::write(root.join("source"), b"retained source").unwrap();
            assert!(matches!(
                no_replace(&dir, "source", &dir, &destination),
                Err(Error::FileConflict)
            ));
            assert_eq!(
                std::fs::read(root.join("source")).unwrap(),
                b"retained source"
            );
            assert_eq!(
                std::fs::read(root.join(&destination)).unwrap(),
                b"exact publication"
            );
        }
    }

    #[test]
    fn operating_system_rejects_replacing_an_existing_target() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("source"), b"source").unwrap();
        std::fs::write(temp.path().join("target"), b"unrelated").unwrap();
        let dir = Dir::open_ambient_dir(temp.path(), ambient_authority()).unwrap();
        assert!(matches!(
            no_replace(&dir, "source", &dir, "target"),
            Err(Error::FileConflict)
        ));
        assert_eq!(
            std::fs::read(temp.path().join("source")).unwrap(),
            b"source"
        );
        assert_eq!(
            std::fs::read(temp.path().join("target")).unwrap(),
            b"unrelated"
        );
    }
}
