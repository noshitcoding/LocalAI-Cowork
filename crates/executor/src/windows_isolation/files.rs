use super::*;
use std::{
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    ptr,
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
    System::Threading::*,
};

pub(super) fn wide(value: &str) -> Vec<u16> {
    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain([0])
        .collect()
}
pub(super) fn system_root() -> Result<String> {
    let mut buffer = vec![0u16; 32768];
    let size = unsafe {
        windows_sys::Win32::System::SystemInformation::GetSystemWindowsDirectoryW(
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    } as usize;
    if size == 0 {
        return Err(io::Error::last_os_error().into());
    }
    if size >= buffer.len() {
        return Err(Error::Invalid("Windows system path is too long"));
    }
    Ok(String::from_utf16(&buffer[..size]).map_err(io::Error::other)?)
}
pub(super) struct Descriptor(PSECURITY_DESCRIPTOR);
impl Descriptor {
    pub(super) fn new(sddl: &str) -> Result<Self> {
        let mut value = ptr::null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide(sddl).as_ptr(),
                1,
                &mut value,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Self(value))
    }
    pub(super) fn pointer(&self) -> PSECURITY_DESCRIPTOR {
        self.0
    }
}
impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

pub(super) fn token(process: HANDLE) -> Result<OwnedHandle> {
    let mut token = ptr::null_mut();
    if unsafe { OpenProcessToken(process, TOKEN_QUERY | TOKEN_DUPLICATE, &mut token) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(token) })
}
pub(super) fn token_info(token: HANDLE, class: TOKEN_INFORMATION_CLASS) -> Result<Vec<usize>> {
    let mut bytes = 0;
    unsafe {
        GetTokenInformation(token, class, ptr::null_mut(), 0, &mut bytes);
    }
    if bytes == 0 || bytes > 65536 {
        return Err(io::Error::other(format!(
            "token information {class}: {}",
            io::Error::last_os_error()
        ))
        .into());
    }
    let mut data = vec![0usize; (bytes as usize).div_ceil(std::mem::size_of::<usize>())];
    if unsafe { GetTokenInformation(token, class, data.as_mut_ptr().cast(), bytes, &mut bytes) }
        == 0
    {
        return Err(io::Error::last_os_error().into());
    }
    Ok(data)
}
pub(super) fn sid_string(sid: PSID) -> Result<String> {
    let mut value = ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut value) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    let mut len = 0;
    unsafe {
        while *value.add(len) != 0 {
            len += 1;
        }
    }
    let output = String::from_utf16(unsafe { std::slice::from_raw_parts(value, len) })
        .map_err(io::Error::other);
    unsafe {
        LocalFree(value.cast());
    }
    Ok(output?)
}
pub(super) fn owner_sid() -> Result<String> {
    let token = token(unsafe { GetCurrentProcess() })?;
    let info = token_info(token.as_raw_handle(), TokenUser)?;
    sid_string(unsafe { (*info.as_ptr().cast::<TOKEN_USER>()).User.Sid })
}
/// Behavioral pre-resume check, not a claim that token class 46 is supported.
/// The package SID must grant access. With `lpac` (the mandatory LPAC creation attribute),
/// ALL APPLICATION PACKAGES must not; a plain AppContainer must match it.
pub(super) fn check_package_access(token: HANDLE, package: &str, lpac: bool) -> Result<()> {
    let mut duplicate = ptr::null_mut();
    if unsafe { DuplicateToken(token, SecurityImpersonation, &mut duplicate) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    let duplicate = unsafe { OwnedHandle::from_raw_handle(duplicate) };
    let owner = owner_sid()?;
    for (principal, expected) in [(package, true), ("AC", !lpac)] {
        let descriptor = Descriptor::new(&format!(
            "O:SYG:SYD:P(A;;FR;;;{owner})(A;;FR;;;{principal})"
        ))?;
        let mapping = GENERIC_MAPPING {
            GenericRead: FILE_GENERIC_READ,
            GenericWrite: FILE_GENERIC_WRITE,
            GenericExecute: FILE_GENERIC_EXECUTE,
            GenericAll: FILE_ALL_ACCESS,
        };
        let mut privileges = vec![0usize; 8192];
        let mut size = std::mem::size_of_val(privileges.as_slice()) as u32;
        let (mut granted, mut status) = (0, 0);
        if unsafe {
            AccessCheck(
                descriptor.pointer(),
                duplicate.as_raw_handle(),
                FILE_READ_DATA,
                &mapping,
                privileges.as_mut_ptr().cast(),
                &mut size,
                &mut granted,
                &mut status,
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        if (status != 0) != expected {
            return Err(Error::Invalid(
                "package access boundary verification failed",
            ));
        }
    }
    Ok(())
}
pub(super) fn owner_only(root: &Path) -> Result<()> {
    set(
        root,
        &format!("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;{})", owner_sid()?),
    )
}
pub(super) fn grant_capsule(root: &Path, sid: &str) -> Result<()> {
    let owner = owner_sid()?;
    let base = format!("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;{owner})");
    // The capsule's journal/lock never inherit the application SID.
    set(root, &format!("{base}(A;;0x1200a0;;;{sid})"))?;
    set(
        &root.join("bundle"),
        &format!("{base}(A;OICI;0x1200a9;;;{sid})"),
    )?;
    set(
        &root.join("work"),
        &format!("{base}(A;OICI;0x1301bf;;;{sid})"),
    )
}
/// Generic read/execute and modify file rights, as in `grant_capsule`.
pub(super) const READ_EXECUTE: u32 = 0x1200a9;
pub(super) const MODIFY: u32 = 0x1301bf;
/// ALL APPLICATION PACKAGES.
pub(super) const ALL_PACKAGES: &str = "S-1-15-2-1";

/// Opens an existing directory for a DACL change without following a final reparse point.
fn directory(root: &Path) -> Result<File> {
    let directory = OpenOptions::new()
        .access_mode(READ_CONTROL | WRITE_DAC | FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(root)?;
    let metadata = directory.metadata()?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(Error::Invalid(
            "capsule ACL target is not an ordinary directory",
        ));
    }
    Ok(directory)
}
struct Local<T>(*mut T);
impl<T> Drop for Local<T> {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                LocalFree(self.0.cast());
            }
        }
    }
}
/// The current DACL (never NULL: a NULL DACL grants everyone everything and is refused)
/// and the descriptor's control bits.
fn dacl(directory: &File) -> Result<(Local<core::ffi::c_void>, *mut ACL, u16)> {
    let (mut acl, mut descriptor) = (ptr::null_mut(), ptr::null_mut());
    let code = unsafe {
        GetSecurityInfo(
            directory.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut acl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code as i32).into());
    }
    let descriptor = Local(descriptor);
    let (mut control, mut revision) = (0u16, 0u32);
    if unsafe { GetSecurityDescriptorControl(descriptor.0, &mut control, &mut revision) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    if acl.is_null() {
        return Err(Error::Invalid("ACL target has no DACL"));
    }
    Ok((descriptor, acl, control))
}
fn sid(text: &str) -> Result<Local<core::ffi::c_void>> {
    let mut value = ptr::null_mut();
    if unsafe { ConvertStringSidToSidW(wide(text).as_ptr(), &mut value) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(Local(value))
}
/// Visits the explicit (not inherited) allow ACEs of `acl` as (flags, mask, SID).
fn explicit_allows(acl: *mut ACL, mut visit: impl FnMut(u8, u32, PSID)) -> Result<()> {
    for index in 0..u32::from(unsafe { (*acl).AceCount }) {
        let mut ace = ptr::null_mut();
        if unsafe { GetAce(acl, index, &mut ace) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        let ace = ace.cast::<ACCESS_ALLOWED_ACE>();
        let header = unsafe { (*ace).Header };
        // ACCESS_ALLOWED_ACE_TYPE
        if header.AceType == 0 && u32::from(header.AceFlags) & INHERITED_ACE == 0 {
            visit(header.AceFlags, unsafe { (*ace).Mask }, unsafe {
                ptr::addr_of_mut!((*ace).SidStart).cast()
            });
        }
    }
    Ok(())
}

/// True when `root` carries an explicit, inheritable allow ACE for `principal` with at
/// least `mask`.
pub(super) fn granted(root: &Path, principal: &str, mask: u32) -> Result<bool> {
    let directory = directory(root)?;
    let (_descriptor, acl, _) = dacl(&directory)?;
    let wanted = sid(principal)?;
    let mut found = false;
    explicit_allows(acl, |flags, granted, sid| {
        found |= u32::from(flags) & SUB_CONTAINERS_AND_OBJECTS_INHERIT
            == SUB_CONTAINERS_AND_OBJECTS_INHERIT
            && granted & mask == mask
            && unsafe { EqualSid(sid, wanted.0) } != 0;
    })?;
    Ok(found)
}

/// Merges an explicit, inheritable allow ACE for `principal` into the existing DACL of an
/// ordinary directory, keeping its protection state. Windows propagates it to the existing
/// tree, so callers grant only trees whose content the host wrote.
pub(super) fn grant(root: &Path, principal: &str, mask: u32) -> Result<()> {
    set_entry(root, principal, mask, GRANT_ACCESS)
}
/// Removes `principal`'s explicit allow ACEs and lets Windows propagate the change. Only for
/// trees the confined process could not write (read/execute grants).
pub(super) fn revoke_tree(root: &Path, principal: &str) -> Result<()> {
    set_entry(root, principal, 0, REVOKE_ACCESS)
}
fn set_entry(root: &Path, principal: &str, mask: u32, mode: ACCESS_MODE) -> Result<()> {
    let directory = directory(root)?;
    let (_descriptor, acl, control) = dacl(&directory)?;
    let trustee = sid(principal)?;
    let access = EXPLICIT_ACCESS_W {
        grfAccessPermissions: mask,
        grfAccessMode: mode,
        grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: ptr::null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: trustee.0.cast(),
        },
    };
    let mut merged = ptr::null_mut();
    let code = unsafe { SetEntriesInAclW(1, &access, acl, &mut merged) };
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code as i32).into());
    }
    let merged = Local(merged);
    let code = unsafe {
        SetSecurityInfo(
            directory.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION
                | if control & SE_DACL_PROTECTED != 0 {
                    PROTECTED_DACL_SECURITY_INFORMATION
                } else {
                    UNPROTECTED_DACL_SECURITY_INFORMATION
                },
            ptr::null_mut(),
            ptr::null_mut(),
            merged.0,
            ptr::null(),
        )
    };
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code as i32).into());
    }
    Ok(())
}

/// Removes every explicit allow ACE of `principal` from the directory object itself. Nothing
/// is propagated: the tree may hold content a confined process wrote, which must not steer
/// a host-side ACL walk. Inherited copies below keep naming the (deleted) package SID.
pub(super) fn revoke_root(root: &Path, principal: &str) -> Result<()> {
    let directory = directory(root)?;
    let (_descriptor, acl, control) = dacl(&directory)?;
    let removed = sid(principal)?;
    let mut size = std::mem::size_of::<ACL>() as u32;
    let mut keep = Vec::new();
    for index in 0..u32::from(unsafe { (*acl).AceCount }) {
        let mut ace = ptr::null_mut();
        if unsafe { GetAce(acl, index, &mut ace) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        let header = unsafe { *ace.cast::<ACE_HEADER>() };
        let drop = header.AceType == 0
            && u32::from(header.AceFlags) & INHERITED_ACE == 0
            && unsafe {
                EqualSid(
                    ptr::addr_of_mut!((*ace.cast::<ACCESS_ALLOWED_ACE>()).SidStart).cast(),
                    removed.0,
                )
            } != 0;
        if !drop {
            size += u32::from(header.AceSize);
            keep.push(ace);
        }
    }
    if keep.len() == usize::from(unsafe { (*acl).AceCount }) {
        return Ok(());
    }
    let mut buffer = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
    let rebuilt = buffer.as_mut_ptr().cast::<ACL>();
    if unsafe { InitializeAcl(rebuilt, size, ACL_REVISION) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    for ace in keep {
        let length = u32::from(unsafe { (*ace.cast::<ACE_HEADER>()).AceSize });
        if unsafe { AddAce(rebuilt, ACL_REVISION, u32::MAX, ace, length) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
    }
    let mut descriptor = [0usize; 8]; // SECURITY_DESCRIPTOR_MIN_LENGTH on 32- and 64-bit.
    let pointer: PSECURITY_DESCRIPTOR = descriptor.as_mut_ptr().cast();
    // SECURITY_DESCRIPTOR_REVISION
    if unsafe { InitializeSecurityDescriptor(pointer, 1) } == 0
        || unsafe { SetSecurityDescriptorDacl(pointer, 1, rebuilt, 0) } == 0
        || unsafe {
            SetSecurityDescriptorControl(
                pointer,
                SE_DACL_PROTECTED | SE_DACL_AUTO_INHERITED,
                control & (SE_DACL_PROTECTED | SE_DACL_AUTO_INHERITED),
            )
        } == 0
        || unsafe {
            SetKernelObjectSecurity(
                directory.as_raw_handle(),
                DACL_SECURITY_INFORMATION,
                pointer,
            )
        } == 0
    {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}

fn set(root: &Path, sddl: &str) -> Result<()> {
    let directory = directory(root)?;
    let descriptor = Descriptor::new(sddl)?;
    let (mut present, mut defaulted, mut acl) = (0, 0, ptr::null_mut());
    if unsafe {
        GetSecurityDescriptorDacl(descriptor.pointer(), &mut present, &mut acl, &mut defaulted)
    } == 0
        || present == 0
        || acl.is_null()
    {
        return Err(Error::Invalid("capsule DACL is missing"));
    }
    let code = unsafe {
        SetSecurityInfo(
            directory.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            acl,
            ptr::null(),
        )
    };
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code as i32).into());
    }
    Ok(())
}
