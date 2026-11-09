use anyhow::{Context, Result, ensure};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[cfg(windows)]
#[path = "../../daemon/src/private_secret_file.rs"]
#[allow(dead_code)]
mod private_secret_file;

pub fn pins(path: &Path, private: bool) -> Result<Vec<File>> {
    ensure!(path.is_absolute(), "Absolute path required");
    let mut files = Vec::new();
    for directory in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let metadata = std::fs::symlink_metadata(directory)?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "Ordinary directories required"
        );
        #[cfg(windows)]
        let file = {
            use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
            ensure!(
                metadata.file_attributes() & 0x400 == 0,
                "Reparse directory refused"
            );
            OpenOptions::new()
                .access_mode(0x0002_0080)
                .share_mode(3)
                .custom_flags(0x0220_0000)
                .open(directory)?
        };
        #[cfg(unix)]
        let file = {
            use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
            ensure!(
                (metadata.uid() == 0 || metadata.uid() == unsafe { libc::geteuid() })
                    && (metadata.mode() & 0o022 == 0 || metadata.mode() & 0o1000 != 0),
                "Untrusted writable ancestor refused"
            );
            OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
                .open(directory)?
        };
        let opened = file.metadata()?;
        ensure!(opened.is_dir(), "Directory handle required");
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            ensure!(
                opened.file_attributes() & 0x400 == 0,
                "Reparse handle refused"
            );
        }
        if directory == path && private {
            #[cfg(windows)]
            private_secret_file::validate(&file)
                .context("Witness state directory is not private")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                ensure!(
                    opened.uid() == unsafe { libc::geteuid() } && opened.mode() & 0o077 == 0,
                    "Witness state directory is not private"
                );
            }
        }
        files.push(file);
    }
    Ok(files)
}
pub fn create_directory(path: &Path) -> Result<()> {
    #[cfg(windows)]
    private_secret_file::create_private_directory(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(path)?;
    }
    Ok(())
}
pub fn open(path: &Path, private: bool, write: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(write);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options
            .access_mode(if write { 0xc002_0000 } else { 0x8002_0000 })
            .share_mode(if write { 3 } else { 1 })
            .custom_flags(0x0020_0000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file(), "Ordinary file required");
    #[cfg(windows)]
    {
        use std::os::windows::{fs::MetadataExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        ensure!(
            metadata.file_attributes() & 0x400 == 0
                && unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } != 0
                && info.nNumberOfLinks == 1,
            "Linked witness file refused"
        );
        if private {
            private_secret_file::validate(&file)?;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(metadata.nlink() == 1, "Linked witness file refused");
        if private {
            ensure!(
                metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o077 == 0,
                "Witness file is not private"
            );
        }
    }
    Ok(file)
}
pub fn read(path: &Path, limit: u64, private: bool) -> Result<Vec<u8>> {
    let _pins = pins(path.parent().context("Missing parent")?, private)?;
    let file = open(path, private, false)?;
    let before = file.metadata()?;
    ensure!(before.len() <= limit, "Input limit exceeded");
    let mut bytes = Vec::new();
    (&file).take(limit + 1).read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    ensure!(
        bytes.len() as u64 == before.len()
            && after.len() == before.len()
            && before.modified().ok() == after.modified().ok(),
        "Input changed while reading"
    );
    Ok(bytes)
}
fn create(path: &Path) -> Result<File> {
    #[cfg(windows)]
    let file = private_secret_file::create_private_file(path)?;
    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .read(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?
    };
    Ok(file)
}
pub fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()?;
    }
    #[cfg(windows)]
    let _ = path;
    Ok(())
}
pub fn no_replace(from: &Path, to: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
        let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
        ensure!(
            unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH) } != 0,
            "No-replace publication failed"
        );
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        let from = std::ffi::CString::new(from.as_os_str().as_bytes())?;
        let to = std::ffi::CString::new(to.as_os_str().as_bytes())?;
        ensure!(
            unsafe {
                libc::renameat2(
                    libc::AT_FDCWD,
                    from.as_ptr(),
                    libc::AT_FDCWD,
                    to.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            } == 0,
            "No-replace publication failed"
        );
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    anyhow::bail!("Witness publication unsupported on this platform");
    Ok(())
}
pub fn publish(folder: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf> {
    ensure!(
        !name.is_empty() && !name.contains(['/', '\\']) && bytes.len() <= 64 * 1024 * 1024,
        "Invalid publication"
    );
    let _pins = pins(folder, false)?;
    let temporary = folder.join(format!(".publish-{}.tmp", Uuid::new_v4()));
    let mut file = create(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    let target = folder.join(name);
    no_replace(&temporary, &target)?;
    sync_directory(folder)?;
    Ok(target)
}
pub fn lock(folder: &Path) -> Result<File> {
    let path = folder.join("witness.lock");
    if !path.try_exists()? {
        match create(&path) {
            Ok(file) => {
                file.sync_all()?;
                sync_directory(folder)?;
            }
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::AlreadyExists) => {}
            Err(e) => return Err(e),
        }
    }
    let file = open(&path, true, true)?;
    file.try_lock()
        .context("Another original witness process owns this state")?;
    Ok(file)
}
