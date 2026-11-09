use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PermissionState {
    standard: cap_std::fs::Permissions,
    #[cfg(windows)]
    dacl: Option<Vec<u8>>,
    #[cfg(windows)]
    protected: bool,
}

pub(crate) fn read(file: &cap_std::fs::File) -> Result<PermissionState> {
    let standard = file.metadata().map_err(storage_error)?.permissions();
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::LocalFree,
            Security::{
                Authorization::{GetSecurityInfo, SE_FILE_OBJECT},
                *,
            },
        };
        let mut descriptor = std::ptr::null_mut();
        let mut acl = std::ptr::null_mut();
        // SAFETY: a live file handle and initialized output pointers are supplied. Windows owns
        // the returned descriptor; the ACL is copied before that allocation is released.
        let status = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut acl,
                std::ptr::null_mut(),
                &mut descriptor,
            )
        };
        if status != 0 {
            return Err(storage_error(std::io::Error::from_raw_os_error(
                status as i32,
            )));
        }
        let mut control = 0;
        let mut revision = 0;
        let ok = unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) };
        let dacl = if acl.is_null() {
            None
        } else {
            // SAFETY: GetSecurityInfo returned a valid ACL whose own size bounds the copy.
            Some(
                unsafe { std::slice::from_raw_parts(acl.cast::<u8>(), (*acl).AclSize as usize) }
                    .to_vec(),
            )
        };
        unsafe {
            LocalFree(descriptor);
        }
        if ok == 0 {
            return Err(Error::Forbidden);
        }
        Ok(PermissionState {
            standard,
            dacl,
            protected: control & SE_DACL_PROTECTED != 0,
        })
    }
    #[cfg(not(windows))]
    Ok(PermissionState { standard })
}

pub(crate) fn apply(file: &cap_std::fs::File, state: &PermissionState) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Security::{
            Authorization::{SE_FILE_OBJECT, SetSecurityInfo},
            *,
        };
        // Vec<usize> supplies native alignment for the opaque ACL copied from Windows.
        let mut aligned = state.dacl.as_ref().map(|bytes| {
            let mut words = vec![0usize; bytes.len().div_ceil(std::mem::size_of::<usize>())];
            unsafe {
                std::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    words.as_mut_ptr().cast::<u8>(),
                    bytes.len(),
                );
            }
            words
        });
        let acl = aligned
            .as_mut()
            .map_or(std::ptr::null(), |words| words.as_ptr().cast::<ACL>());
        let flags = DACL_SECURITY_INFORMATION
            | if state.protected {
                PROTECTED_DACL_SECURITY_INFORMATION
            } else {
                UNPROTECTED_DACL_SECURITY_INFORMATION
            };
        // SAFETY: the handle has WRITE_DAC; the ACL allocation remains aligned and live during
        // the call. A null ACL is used only when the selected original also had a null ACL.
        let status = unsafe {
            SetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                flags,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                acl,
                std::ptr::null(),
            )
        };
        if status != 0 {
            return Err(storage_error(std::io::Error::from_raw_os_error(
                status as i32,
            )));
        }
    }
    file.set_permissions(state.standard.clone())
        .map_err(storage_error)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn replacement_preserves_a_protected_windows_dacl() {
        use cap_std::fs::OpenOptionsExt;
        use windows_sys::Win32::{
            Foundation::{GENERIC_READ, GENERIC_WRITE},
            Storage::FileSystem::WRITE_DAC,
        };
        let temp = tempfile::tempdir().unwrap();
        let dir = Dir::open_ambient_dir(temp.path(), ambient_authority()).unwrap();
        let mut options = OpenOptions::new();
        options
            .write(true)
            .read(true)
            .create_new(true)
            .access_mode(GENERIC_READ | GENERIC_WRITE | WRITE_DAC);
        let mut file = dir.open_with("private.bin", &options).unwrap();
        file.write_all(b"original").unwrap();
        let mut state = read(&file).unwrap();
        state.protected = true;
        apply(&file, &state).unwrap();
        let expected = read(&file).unwrap();
        assert!(expected.protected);
        drop(file);
        let metadata = DownloadMetadata {
            file_name: "private.bin".into(),
            size: 3,
            sha256: hash(b"new"),
        };
        PreparedDelivery::prepare(&temp.path().join("private.bin"), b"new", &metadata, &[])
            .unwrap()
            .commit()
            .unwrap();
        assert_eq!(read(&dir.open("private.bin").unwrap()).unwrap(), expected);
    }
}
