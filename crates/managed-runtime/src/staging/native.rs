use super::*;
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
#[cfg(windows)]
use std::os::windows::io::AsRawHandle;

#[derive(PartialEq, Eq)]
pub(super) struct Identity {
    volume: u64,
    object: [u8; 16],
}

#[cfg(target_os = "linux")]
pub(super) fn directory_identity(dir: &Dir) -> Result<Identity> {
    let meta = dir.dir_metadata()?;
    if !meta.is_dir() {
        return Err(Error::Staging);
    }
    Ok(unix_identity(&meta))
}
#[cfg(target_os = "linux")]
pub(super) fn file_identity(file: &File) -> Result<Identity> {
    let meta = file.metadata()?;
    if !meta.is_file() || meta.nlink() != 1 {
        return Err(Error::Staging);
    }
    Ok(unix_identity(&meta))
}
#[cfg(target_os = "linux")]
fn unix_identity(meta: &cap_std::fs::Metadata) -> Identity {
    let mut object = [0; 16];
    object[..8].copy_from_slice(&meta.ino().to_le_bytes());
    Identity {
        volume: meta.dev(),
        object,
    }
}
#[cfg(target_os = "linux")]
pub(super) fn leaf_identity(dir: &Dir, name: &str) -> Result<Identity> {
    Ok(unix_identity(&dir.symlink_metadata(name)?))
}
#[cfg(target_os = "linux")]
pub(super) fn pin_link(dir: &Dir, name: &str) -> Result<File> {
    use std::os::fd::FromRawFd;
    let name = std::ffi::CString::new(name).map_err(|_| Error::Staging)?;
    // SAFETY: retained parent and one validated leaf. Open the link inode,
    // never the guest link target, with close-on-exec protection.
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let file = File::from_std(unsafe { std::fs::File::from_raw_fd(fd) });
    link_identity(&file)?;
    Ok(file)
}
#[cfg(target_os = "linux")]
pub(super) fn link_identity(file: &File) -> Result<Identity> {
    let meta = file.metadata()?;
    if !meta.is_symlink() || meta.nlink() != 1 || cap_std::fs::MetadataExt::uid(&meta) != 0 {
        return Err(Error::Staging);
    }
    Ok(unix_identity(&meta))
}
#[cfg(target_os = "linux")]
pub(super) fn read_pinned_link(file: &File) -> Result<String> {
    link_identity(file)?;
    let mut bytes = [0u8; 1025];
    // SAFETY: O_PATH link descriptor, empty path and bounded output buffer.
    let count = unsafe {
        libc::readlinkat(
            file.as_raw_fd(),
            c"".as_ptr(),
            bytes.as_mut_ptr().cast(),
            bytes.len(),
        )
    };
    if count < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if count == 0 || count as usize > 1024 {
        return Err(Error::Staging);
    }
    String::from_utf8(bytes[..count as usize].to_vec()).map_err(|_| Error::Staging)
}
#[cfg(target_os = "linux")]
pub(super) fn private_directory(dir: &Dir) -> Result<()> {
    use cap_std::fs::MetadataExt;
    let metadata = dir.dir_metadata()?;
    // The management user's authority is separate from workload authority. A
    // privileged installer will additionally require root before activation.
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != 0o700
    {
        return Err(Error::Staging);
    }
    Ok(())
}

#[cfg(windows)]
pub(super) fn directory_identity(dir: &Dir) -> Result<Identity> {
    windows_identity(dir.as_raw_handle(), true)
}
#[cfg(windows)]
pub(super) fn file_identity(file: &File) -> Result<Identity> {
    windows_identity(file.as_raw_handle(), false)
}
#[cfg(windows)]
fn windows_identity(
    handle: windows_sys::Win32::Foundation::HANDLE,
    directory: bool,
) -> Result<Identity> {
    use windows_sys::Win32::Storage::FileSystem::*;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    let mut id = FILE_ID_INFO::default();
    // SAFETY: live borrowed handle and SDK-sized initialized output structures.
    if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0
        || unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileIdInfo,
                (&mut id as *mut FILE_ID_INFO).cast(),
                std::mem::size_of::<FILE_ID_INFO>() as u32,
            )
        } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
        || (!directory
            && (info.nNumberOfLinks != 1
                || info.dwFileAttributes
                    & (FILE_ATTRIBUTE_SPARSE_FILE | FILE_ATTRIBUTE_COMPRESSED)
                    != 0))
    {
        return Err(Error::Staging);
    }
    // ReFS uses 128-bit IDs; never truncate to the convenience 64-bit ino API.
    Ok(Identity {
        volume: id.VolumeSerialNumber,
        object: id.FileId.Identifier,
    })
}

#[cfg(windows)]
pub(super) fn private_directory(dir: &Dir) -> Result<()> {
    use windows_sys::Win32::{
        Foundation::*,
        Security::{Authorization::*, *},
        System::Threading::*,
    };
    let mut token = std::ptr::null_mut();
    // SAFETY: current process pseudo-handle and initialized owned output slot.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut needed = 0;
    unsafe {
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);
    }
    if needed == 0 || needed > 4096 {
        unsafe {
            CloseHandle(token);
        }
        return Err(Error::Staging);
    }
    // Native alignment for TOKEN_USER and its variable SID allocation.
    let mut user = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            user.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    };
    unsafe {
        CloseHandle(token);
    }
    if ok == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let user_sid = unsafe { (*(user.as_ptr().cast::<TOKEN_USER>())).User.Sid };
    let mut owner = std::ptr::null_mut();
    let mut acl = std::ptr::null_mut();
    let mut descriptor = std::ptr::null_mut();
    let status = unsafe {
        GetSecurityInfo(
            dir.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            &mut acl,
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(std::io::Error::from_raw_os_error(status as i32).into());
    }
    let result = (|| {
        let trusted = |sid: PSID| unsafe {
            !sid.is_null()
                && (EqualSid(sid, user_sid) != 0
                    || IsWellKnownSid(sid, WinLocalSystemSid) != 0
                    || IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0)
        };
        if !trusted(owner) || acl.is_null() || unsafe { IsValidAcl(acl) } == 0 {
            return Err(Error::Staging);
        }
        // Every ACE is interpreted explicitly. Unknown callback/object ACEs are
        // denied rather than guessing their authorization semantics.
        for index in 0..unsafe { (*acl).AceCount } as u32 {
            let mut ace = std::ptr::null_mut();
            if unsafe { GetAce(acl, index, &mut ace) } == 0 || ace.is_null() {
                return Err(Error::Staging);
            }
            let header = unsafe { &*ace.cast::<ACE_HEADER>() };
            if header.AceType != 0 || header.AceSize < 16 {
                return Err(Error::Staging);
            }
            let allow = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
            let sid = (&allow.SidStart as *const u32).cast_mut().cast();
            if unsafe { IsValidSid(sid) } == 0 {
                return Err(Error::Staging);
            }
            if unsafe { GetLengthSid(sid) } > u32::from(header.AceSize) - 8 {
                return Err(Error::Staging);
            }
            // OWNER RIGHTS is safe only because the owner was checked above.
            if !trusted(sid) && unsafe { IsWellKnownSid(sid, WinCreatorOwnerRightsSid) } == 0 {
                return Err(Error::Staging);
            }
        }
        Ok(())
    })();
    unsafe {
        LocalFree(descriptor);
    }
    result
}

pub(super) fn create_file(dir: &Dir, name: &str) -> Result<File> {
    let mut options = OpenOptions::new();
    options
        .write(true)
        .read(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    let file = dir.open_with(name, &options)?;
    file_identity(&file)?;
    Ok(file)
}

pub(super) fn pin_file(dir: &Dir, name: &str) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.share_mode(windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ);
    }
    let file = dir.open_with(name, &options)?;
    file_identity(&file)?;
    Ok(file)
}

#[cfg(target_os = "linux")]
pub(super) fn allocate(file: &File, bytes: u64) -> Result<()> {
    // SAFETY: borrowed open writable file; manifest limits fit off_t. fallocate
    // must succeed physically; no glibc sparse/emulated allocation fallback.
    if unsafe { libc::fallocate(file.as_raw_fd(), 0, 0, bytes as libc::off_t) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    file.set_len(bytes)?;
    file.sync_all()?;
    check_allocation(file, bytes)
}
#[cfg(target_os = "linux")]
pub(super) fn check_allocation(file: &File, bytes: u64) -> Result<()> {
    use cap_std::fs::MetadataExt;
    let meta = file.metadata()?;
    if meta.len() != bytes
        || meta.blocks().saturating_mul(512) < bytes
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o177 != 0
    {
        return Err(Error::Staging);
    }
    Ok(())
}
#[cfg(windows)]
pub(super) fn allocate(file: &File, bytes: u64) -> Result<()> {
    use windows_sys::Win32::Storage::FileSystem::*;
    let info = FILE_ALLOCATION_INFO {
        AllocationSize: bytes as i64,
    };
    // SAFETY: live writable file and SDK-sized input allocation record.
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileAllocationInfo,
            (&info as *const FILE_ALLOCATION_INFO).cast(),
            std::mem::size_of::<FILE_ALLOCATION_INFO>() as u32,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    file.set_len(bytes)?;
    file.sync_all()?;
    check_allocation(file, bytes)
}
#[cfg(windows)]
pub(super) fn check_allocation(file: &File, bytes: u64) -> Result<()> {
    use windows_sys::Win32::Storage::FileSystem::*;
    let mut info = FILE_STANDARD_INFO::default();
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileStandardInfo,
            (&mut info as *mut FILE_STANDARD_INFO).cast(),
            std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    if info.EndOfFile != bytes as i64 || info.AllocationSize < bytes as i64 {
        return Err(Error::Staging);
    }
    Ok(())
}
pub(super) fn sync_directory(dir: &Dir) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        use cap_std::fs::OpenOptionsExt;
        // cap-std directory capabilities may be O_PATH, which cannot be
        // fsynced. Open the same directory for read, never its ambient path.
        let mut options = OpenOptions::new();
        options
            .read(true)
            .follow(FollowSymlinks::No)
            .custom_flags(libc::O_DIRECTORY);
        let readable = dir.open_with(".", &options)?;
        if unix_identity(&readable.metadata()?) != directory_identity(dir)? {
            return Err(Error::Staging);
        }
        readable.sync_all()?;
    }
    // Windows file bytes are flushed independently. This makes no claim of
    // crash-atomic directory publication or a Windows filesystem transaction.
    let _ = dir;
    Ok(())
}
