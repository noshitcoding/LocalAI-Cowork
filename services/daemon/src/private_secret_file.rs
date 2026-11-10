//! Validate the already-opened secret handle, not a path ACL snapshot.
use std::{
    fs::File,
    io,
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr,
};
use windows_sys::Win32::{
    Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_NO_TOKEN, LocalFree},
    Security::{
        Authorization::{GetSecurityInfo, SE_FILE_OBJECT},
        *,
    },
    System::Threading::{GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken},
};
struct Descriptor(PSECURITY_DESCRIPTOR);
impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "Tenant installation secret file is not private",
    )
}
fn last() -> io::Error {
    io::Error::last_os_error()
}
fn current_user() -> io::Result<Vec<usize>> {
    let mut raw = ptr::null_mut();
    // Prefer the effective thread identity if this process is impersonating.
    if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut raw) } == 0 {
        if last().raw_os_error() != Some(ERROR_NO_TOKEN as i32) {
            return Err(last());
        }
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } == 0 {
            return Err(last());
        }
    }
    let token = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut bytes = 0;
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            ptr::null_mut(),
            0,
            &mut bytes,
        );
    }
    if last().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
        || !(size_of::<TOKEN_USER>() as u32..=65_536).contains(&bytes)
    {
        return Err(denied());
    }
    // Native alignment for TOKEN_USER and the inline SID allocated by Windows.
    let mut buffer = vec![0usize; (bytes as usize).div_ceil(size_of::<usize>())];
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            bytes,
            &mut bytes,
        )
    } == 0
    {
        return Err(last());
    }
    Ok(buffer)
}
pub(super) fn validate(file: &File) -> io::Result<()> {
    validate_access(file, false)
}
/// Directories may expose traversal/listing, but cannot delegate replacement or
/// security-descriptor mutation of the authority namespace.
pub(super) fn validate_directory(file: &File) -> io::Result<()> {
    validate_access(file, true)
}
fn validate_access(file: &File, directory: bool) -> io::Result<()> {
    let mut descriptor = ptr::null_mut();
    let mut owner = ptr::null_mut();
    let mut acl = ptr::null_mut();
    // Owner and ACL borrow the one allocated descriptor released by this guard.
    let status = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut acl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let descriptor = Descriptor(descriptor);
    let user = current_user()?;
    let user_sid = unsafe { (*(user.as_ptr().cast::<TOKEN_USER>())).User.Sid };
    if owner.is_null()
        || unsafe { IsValidSid(owner) } == 0
        || unsafe { EqualSid(owner, user_sid) } == 0
    {
        return Err(denied());
    }
    let mut present = 0;
    let mut defaulted = 0;
    if unsafe { GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut acl, &mut defaulted) }
        == 0
        || present == 0
        || acl.is_null()
        || unsafe { IsValidAcl(acl) } == 0
    {
        return Err(denied());
    }
    let mut info = ACL_SIZE_INFORMATION::default();
    if unsafe {
        GetAclInformation(
            acl,
            (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    } == 0
    {
        return Err(last());
    }
    for index in 0..info.AceCount {
        let mut ace = ptr::null_mut();
        if unsafe { GetAce(acl, index, &mut ace) } == 0 {
            return Err(last());
        }
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        if u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0 {
            continue;
        }
        // Conservative: unknown object/callback/conditional ACEs establish no
        // private authority. Ordinary deny ACEs grant no rights.
        match header.AceType {
            1 => continue,
            0 => {}
            _ => return Err(denied()),
        }
        let offset = size_of::<ACE_HEADER>() + size_of::<u32>();
        if usize::from(header.AceSize) < offset + 8 {
            return Err(denied());
        }
        let sid = unsafe { ace.cast::<u8>().add(offset) };
        let count = unsafe { *sid.add(1) } as usize;
        if count > 15 || offset + 8 + 4 * count > usize::from(header.AceSize) {
            return Err(denied());
        }
        let sid = sid.cast();
        if unsafe { IsValidSid(sid) } == 0 {
            return Err(denied());
        }
        let trusted = unsafe {
            EqualSid(sid, user_sid) != 0
                // OWNER RIGHTS identifies this object's owner, already verified
                // against the effective user above. Python 3.13+ uses it for
                // private temporary directories. CREATOR OWNER is not equivalent.
                || IsWellKnownSid(sid, WinCreatorOwnerRightsSid) != 0
                || IsWellKnownSid(sid, WinLocalSystemSid) != 0
                || IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0
        };
        // SYSTEM and local administrators are explicitly privileged OS principals.
        // All other allow trustees, including narrow grants, are rejected.
        let mask = unsafe { *ace.cast::<u8>().add(size_of::<ACE_HEADER>()).cast::<u32>() };
        // FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY, FILE_DELETE_CHILD, DELETE,
        // WRITE_DAC, WRITE_OWNER and unmapped GENERIC_WRITE/GENERIC_ALL.
        let directory_mutation = 0x0000_0156 | 0x000d_0000 | 0x5000_0000;
        if !trusted && (!directory || mask & directory_mutation != 0) {
            return Err(denied());
        }
    }
    Ok(())
}

fn creation_descriptor() -> io::Result<Descriptor> {
    creation_descriptor_with_inheritance(false)
}
fn creation_descriptor_with_inheritance(inherit_children: bool) -> io::Result<Descriptor> {
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    };
    let user = current_user()?;
    let sid = unsafe { (*(user.as_ptr().cast::<TOKEN_USER>())).User.Sid };
    let mut text = ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(last());
    }
    let allocation = Descriptor(text.cast());
    let mut length = 0;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
        if length >= 256 {
            return Err(denied());
        }
    }
    let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(text, length) })
        .map_err(|_| denied())?;
    drop(allocation);
    // Existing token/file creation keeps its original non-inheriting descriptor.
    // Workspace directories alone propagate the same private trustees to engine files.
    let flags = if inherit_children { "OICI" } else { "" };
    let sddl = format!("O:{sid}D:P(A;{flags};FA;;;{sid})(A;{flags};FA;;;SY)(A;{flags};FA;;;BA)");
    let wide: Vec<_> = sddl.encode_utf16().chain(Some(0)).collect();
    let mut descriptor = ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide.as_ptr(),
            1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(last());
    }
    Ok(Descriptor(descriptor))
}

pub(super) fn create_private_directory(path: &std::path::Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;
    let descriptor = creation_descriptor()?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let path: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { CreateDirectoryW(path.as_ptr(), &attributes) } == 0 {
        return Err(last());
    }
    Ok(())
}

/// Create one new child and retain its original NoRename handle atomically.
/// The native parent is a namespace anchor, not an adopted private directory.
/// Unlike CreateDirectoryW followed by reopen, FILE_CREATE returns the original
/// object before a holder of DELETE_CHILD on the parent could substitute it.
#[allow(
    dead_code,
    reason = "Workspace startup lives in the daemon entry point; this authority module is also compiled by the library."
)]
pub(super) fn create_private_directory_handle(parent: &File, name: &str) -> io::Result<File> {
    use std::{
        ffi::c_void,
        path::{Component, Path},
    };
    use windows_sys::Win32::Foundation::HANDLE;
    let mut components = Path::new(name).components();
    if !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
        || name.is_empty()
        || name == "."
        || name == ".."
        || name.ends_with('.')
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        return Err(denied());
    }
    #[repr(C)]
    struct UnicodeString {
        length: u16,
        maximum_length: u16,
        buffer: *mut u16,
    }
    #[repr(C)]
    struct ObjectAttributes {
        length: u32,
        root_directory: HANDLE,
        object_name: *mut UnicodeString,
        attributes: u32,
        security_descriptor: *mut c_void,
        security_quality_of_service: *mut c_void,
    }
    #[repr(C)]
    struct IoStatusBlock {
        status_or_pointer: usize,
        information: usize,
    }
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtCreateFile(
            handle: *mut HANDLE,
            access: u32,
            attributes: *const ObjectAttributes,
            status: *mut IoStatusBlock,
            allocation_size: *const i64,
            file_attributes: u32,
            share_access: u32,
            disposition: u32,
            options: u32,
            ea: *const c_void,
            ea_length: u32,
        ) -> i32;
        fn RtlNtStatusToDosError(status: i32) -> u32;
    }
    let mut name: Vec<u16> = name.encode_utf16().collect();
    let length =
        u16::try_from(name.len().checked_mul(2).ok_or_else(denied)?).map_err(|_| denied())?;
    let mut name = UnicodeString {
        length,
        maximum_length: length,
        buffer: name.as_mut_ptr(),
    };
    let descriptor = creation_descriptor_with_inheritance(true)?;
    let attributes = ObjectAttributes {
        length: size_of::<ObjectAttributes>() as u32,
        root_directory: parent.as_raw_handle(),
        object_name: &mut name,
        // The already-open original parent and one-component FILE_CREATE avoid
        // name traversal/adoption. The broader object-manager OBJ_DONT_REPARSE
        // flag is unnecessary for refusing an existing leaf. The original child
        // is checked directly below. No OPENIF or handle INHERIT is permitted.
        attributes: 0x40, // OBJ_CASE_INSENSITIVE.
        security_descriptor: descriptor.0,
        security_quality_of_service: ptr::null_mut(),
    };
    let mut status = IoStatusBlock {
        status_or_pointer: 0,
        information: 0,
    };
    let mut handle = ptr::null_mut();
    // READ_CONTROL|SYNCHRONIZE|LIST_DIRECTORY|TRAVERSE|READ_ATTRIBUTES.
    // Share read/write, deny delete from the instant this original is created.
    let result = unsafe {
        NtCreateFile(
            &mut handle,
            0x0012_00a1,
            &attributes,
            &mut status,
            ptr::null(),
            0x80,
            3,
            2, // FILE_ATTRIBUTE_NORMAL, FILE_CREATE (never open/replace).
            0x0020_4021,
            ptr::null(),
            0,
        )
    }; // DIRECTORY|SYNCHRONOUS_NONALERT|BACKUP|OPEN_REPARSE.
    if result < 0 {
        return Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(result) } as i32,
        ));
    }
    if handle.is_null() {
        return Err(denied());
    }
    let original = unsafe { File::from_raw_handle(handle) };
    let metadata = original.metadata()?;
    use std::os::windows::fs::MetadataExt;
    if status.information != 2 || !metadata.is_dir() || metadata.file_attributes() & 0x400 != 0 {
        return Err(denied());
    }
    validate(&original)?;
    Ok(original)
}

/// Apply the private ACL at CREATE_NEW, before any secret byte is written. No
/// existing file or directory ever passes through a SetSecurityInfo repair.
pub(super) fn create_private_file(path: &std::path::Path) -> io::Result<File> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{
        Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{
            CREATE_NEW, CreateFileW, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        },
    };
    let descriptor = creation_descriptor()?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let path: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_READ | GENERIC_WRITE | 0x0001_0000,
            FILE_SHARE_READ,
            &attributes,
            CREATE_NEW,
            FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(last());
    }
    Ok(unsafe { File::from_raw_handle(handle) })
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SetSecurityInfo,
    };
    fn user_sddl() -> String {
        let user = current_user().unwrap();
        let sid = unsafe { (*(user.as_ptr().cast::<TOKEN_USER>())).User.Sid };
        let mut text = ptr::null_mut();
        assert_ne!(unsafe { ConvertSidToStringSidW(sid, &mut text) }, 0);
        let allocation = Descriptor(text.cast());
        let mut length = 0;
        while unsafe { *text.add(length) } != 0 {
            length += 1;
            assert!(length < 256);
        }
        let result =
            String::from_utf16(unsafe { std::slice::from_raw_parts(text, length) }).unwrap();
        drop(allocation);
        result
    }
    fn acl(file: &File, sddl: &str) {
        // Elevated tokens can default new objects to the Administrators owner.
        // These ACL fixtures require the effective user, just like production.
        let user = current_user().unwrap();
        let owner = unsafe { (*(user.as_ptr().cast::<TOKEN_USER>())).User.Sid };
        let text: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
        let mut descriptor = ptr::null_mut();
        assert_ne!(
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    text.as_ptr(),
                    1,
                    &mut descriptor,
                    ptr::null_mut(),
                )
            },
            0
        );
        let descriptor = Descriptor(descriptor);
        let mut present = 0;
        let mut dacl = ptr::null_mut();
        let mut defaulted = 0;
        assert_ne!(
            unsafe {
                GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut dacl, &mut defaulted)
            },
            0
        );
        assert_eq!(
            unsafe {
                SetSecurityInfo(
                    file.as_raw_handle(),
                    SE_FILE_OBJECT,
                    OWNER_SECURITY_INFORMATION
                        | DACL_SECURITY_INFORMATION
                        | PROTECTED_DACL_SECURITY_INFORMATION,
                    owner,
                    ptr::null_mut(),
                    dacl,
                    ptr::null_mut(),
                )
            },
            0
        );
    }
    fn file(directory: &tempfile::TempDir) -> File {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .access_mode(0x0002_0000 | 0x0004_0000 | 0x0008_0000 | 3)
            .open(directory.path().join("owned-synthetic-secret.txt"))
            .unwrap()
    }
    #[test]
    fn exact_opened_handle_accepts_private_acl_and_denies_everyone() {
        let directory = tempfile::tempdir().unwrap();
        let file = file(&directory);
        let user = user_sddl();
        acl(
            &file,
            &format!("D:P(A;;FA;;;{user})(A;;FA;;;SY)(A;;FA;;;BA)"),
        );
        validate(&file).unwrap();
        acl(&file, &format!("D:P(A;;FA;;;{user})(A;;FR;;;WD)"));
        assert_eq!(
            validate(&file).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        acl(&file, &format!("D:P(A;;FA;;;{user})"));
        validate(&file).unwrap();
    }
    #[test]
    fn null_dacl_never_establishes_private_secret_authority() {
        let directory = tempfile::tempdir().unwrap();
        let file = file(&directory);
        let user = user_sddl();
        acl(
            &file,
            &format!("D:P(A;;FA;;;{user})(A;;FA;;;SY)(A;;FA;;;BA)"),
        );
        validate(&file).unwrap();
        assert_eq!(
            unsafe {
                SetSecurityInfo(
                    file.as_raw_handle(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null(),
                    ptr::null_mut(),
                )
            },
            0
        );
        assert_eq!(
            validate(&file).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn owner_rights_acl_accepts_current_owner_but_not_another_trustee() {
        let directory = tempfile::tempdir().unwrap();
        let file = file(&directory);
        acl(&file, "D:P(A;;FA;;;OW)(A;;FA;;;SY)(A;;FA;;;BA)");
        validate(&file).unwrap();
        acl(&file, "D:P(A;;FA;;;OW)(A;;FR;;;AU)");
        assert_eq!(
            validate(&file).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        acl(&file, "D:P(A;;FA;;;OW)");
        validate(&file).unwrap();
    }

    #[test]
    fn owner_rights_directory_acl_does_not_allow_foreign_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let file = std::fs::OpenOptions::new()
            .access_mode(0x0002_0000 | 0x0004_0000 | 0x0008_0000)
            .custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS)
            .open(directory.path())
            .unwrap();
        acl(&file, "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)");
        validate_directory(&file).unwrap();
        acl(&file, "D:P(A;OICI;FA;;;OW)(A;;FW;;;AU)");
        assert_eq!(
            validate_directory(&file).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        acl(&file, "D:P(A;OICI;FA;;;OW)");
        validate_directory(&file).unwrap();
    }
}
