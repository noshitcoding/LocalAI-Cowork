use super::files::{Descriptor, wide};
use super::*;
use std::{
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr,
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Isolation::*, *},
    System::{JobObjects::*, StationsAndDesktops::*},
};

pub(crate) struct Identity {
    name: String,
    sid: PSID,
    desktop: HDESK,
    desktop_name: Vec<u16>,
    deleted: bool,
}
impl Identity {
    pub(super) fn create(name: &str) -> Result<Self> {
        let _gate = profile_gate(name)?;
        let name_w = wide(name);
        let mut sid = ptr::null_mut();
        let result = unsafe {
            CreateAppContainerProfile(
                name_w.as_ptr(),
                name_w.as_ptr(),
                wide("LocalAI Cowork V2 isolated process").as_ptr(),
                ptr::null(),
                0,
                &mut sid,
            )
        };
        if result < 0 {
            return Err(io::Error::other(format!(
                "AppContainer profile creation failed: 0x{:08x}",
                result as u32
            ))
            .into());
        }
        let mut value = Self {
            name: name.into(),
            sid,
            desktop: ptr::null_mut(),
            desktop_name: wide(name),
            deleted: false,
        };
        let owner = files::owner_sid()?;
        let sid_string = value.sid_string()?;
        let descriptor = Descriptor::new(&format!(
            "D:P(A;;GA;;;SY)(A;;GA;;;{owner})(A;;0x01ff;;;{sid_string})"
        ))?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.pointer(),
            bInheritHandle: 0,
        };
        value.desktop = unsafe {
            CreateDesktopW(
                value.desktop_name.as_ptr(),
                ptr::null(),
                ptr::null(),
                0,
                0x000f01ff,
                &attributes,
            )
        };
        if value.desktop.is_null() {
            return Err(io::Error::last_os_error().into());
        }
        Ok(value)
    }
    pub(super) fn name(&self) -> &str {
        &self.name
    }
    pub(super) fn sid_string(&self) -> Result<String> {
        files::sid_string(self.sid)
    }
    pub(super) fn profile_folder(&self) -> Result<String> {
        let sid = wide(&self.sid_string()?);
        let mut path = ptr::null_mut();
        let result = unsafe { GetAppContainerFolderPath(sid.as_ptr(), &mut path) };
        if result < 0 {
            return Err(io::Error::other(format!(
                "AppContainer folder unavailable: 0x{:08x}",
                result as u32
            ))
            .into());
        }
        if path.is_null() {
            return Err(Error::Invalid("missing AppContainer folder"));
        }
        let mut length = 0;
        unsafe {
            while *path.add(length) != 0 {
                length += 1;
            }
        }
        let folder = String::from_utf16(unsafe { std::slice::from_raw_parts(path, length) })
            .map_err(io::Error::other);
        unsafe {
            windows_sys::Win32::System::Com::CoTaskMemFree(path.cast());
        }
        Ok(folder?)
    }
    pub(crate) fn capabilities(&self) -> SECURITY_CAPABILITIES {
        SECURITY_CAPABILITIES {
            AppContainerSid: self.sid,
            Capabilities: ptr::null_mut(),
            CapabilityCount: 0,
            Reserved: 0,
        }
    }
    pub(crate) fn desktop(&self) -> *mut u16 {
        self.desktop_name.as_ptr().cast_mut()
    }
    pub(crate) fn job_name(&self) -> Vec<u16> {
        wide(&format!("Local\\{}.job", self.name))
    }
    /// `lpac`: the less-privileged capsule profile; otherwise a plain AppContainer.
    pub(crate) fn verify_process(&self, process: HANDLE, lpac: bool) -> Result<()> {
        let token = files::token(process)?;
        let value = files::token_info(token.as_raw_handle(), TokenIsAppContainer)?;
        if unsafe { *value.as_ptr().cast::<u32>() } != 1 {
            return Err(Error::Invalid(
                "child did not receive an AppContainer token",
            ));
        }
        let value = files::token_info(token.as_raw_handle(), TokenAppContainerSid)?;
        let sid =
            unsafe { (*value.as_ptr().cast::<TOKEN_APPCONTAINER_INFORMATION>()).TokenAppContainer };
        if unsafe { EqualSid(sid, self.sid) } == 0 {
            return Err(Error::Invalid(
                "child has a different AppContainer identity",
            ));
        }
        let value = files::token_info(token.as_raw_handle(), TokenCapabilities)?;
        if unsafe { (*value.as_ptr().cast::<TOKEN_GROUPS>()).GroupCount } != 0 {
            return Err(Error::Invalid("unexpected child capabilities"));
        }
        let value = files::token_info(token.as_raw_handle(), TokenIntegrityLevel)?;
        let sid = unsafe { (*value.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()).Label.Sid };
        if unsafe { IsValidSid(sid) } == 0 {
            return Err(Error::Invalid("invalid integrity label"));
        }
        let count = unsafe { *GetSidSubAuthorityCount(sid) };
        if count == 0 || unsafe { *GetSidSubAuthority(sid, u32::from(count - 1)) } > 0x1000 {
            return Err(Error::Invalid("child integrity level exceeds Low"));
        }
        files::check_package_access(token.as_raw_handle(), &self.sid_string()?, lpac)?;
        Ok(())
    }
    pub(super) fn close(&mut self) -> Result<()> {
        let _gate = profile_gate(&self.name)?;
        ensure_job_closed(&self.name)?;
        if !self.desktop.is_null() {
            if unsafe { CloseDesktop(self.desktop) } == 0 {
                return Err(io::Error::last_os_error().into());
            }
            self.desktop = ptr::null_mut();
        }
        if !self.deleted {
            delete_profile(&self.name)?;
            self.deleted = true;
        }
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn close_own_preownership_fixture_handles(&mut self) -> Result<()> {
        ensure_job_closed(&self.name)?;
        if !self.desktop.is_null() {
            if unsafe { CloseDesktop(self.desktop) } == 0 {
                return Err(io::Error::last_os_error().into());
            }
            self.desktop = ptr::null_mut();
        }
        // The original fresh test identity remains a cleanup guard even if its
        // admin fixture fails. It owns no native desktop/process handle now.
        Ok(())
    }
}
impl Drop for Identity {
    fn drop(&mut self) {
        let _ = self.close();
        if !self.sid.is_null() {
            unsafe {
                FreeSid(self.sid);
            }
        }
    }
}
pub(super) fn recover_profile(name: &str) -> Result<()> {
    let _gate = profile_gate(name)?;
    ensure_job_closed(name)?;
    delete_profile(name)
}
fn delete_profile(name: &str) -> Result<()> {
    let result = unsafe { DeleteAppContainerProfile(wide(name).as_ptr()) };
    if result < 0 {
        return Err(io::Error::other(format!(
            "AppContainer cleanup unconfirmed: 0x{:08x}",
            result as u32
        ))
        .into());
    }
    Ok(())
}
pub(super) fn ensure_job_closed(name: &str) -> Result<()> {
    const JOB_OBJECT_QUERY: u32 = 0x0004;
    let handle = unsafe {
        OpenJobObjectW(
            JOB_OBJECT_QUERY,
            0,
            wide(&format!("Local\\{name}.job")).as_ptr(),
        )
    };
    if handle.is_null() {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) {
            Ok(())
        } else {
            Err(error.into())
        };
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe {
        QueryInformationJobObject(
            handle.as_raw_handle(),
            JobObjectBasicAccountingInformation,
            (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
            std::mem::size_of_val(&info) as u32,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error().into());
    }
    if info.ActiveProcesses != 0 {
        return Err(Error::Invalid("capsule job still has active processes"));
    }
    Ok(())
}

/// Serialize every product creation/deletion of the exact per-user profile.
/// This is a lifecycle gate, not proof that a name belongs to this journal.
pub(super) struct ProfileGate(OwnedHandle, std::marker::PhantomData<std::rc::Rc<()>>);
pub(super) fn profile_gate(name: &str) -> Result<ProfileGate> {
    use windows_sys::Win32::System::Threading::*;
    let owner = files::owner_sid()?;
    let descriptor = Descriptor::new(&format!("D:P(A;;GA;;;SY)(A;;GA;;;{owner})"))?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.pointer(),
        bInheritHandle: 0,
    };
    let handle = unsafe {
        CreateMutexW(
            &attributes,
            0,
            wide(&format!("Local\\{name}.{owner}.profile-gate")).as_ptr(),
        )
    };
    if handle.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    let status = unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) };
    if status != WAIT_OBJECT_0 && status != WAIT_ABANDONED {
        return Err(Error::Invalid(
            "original profile lifecycle is active or unavailable",
        ));
    }
    Ok(ProfileGate(handle, std::marker::PhantomData))
}
impl Drop for ProfileGate {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Threading::ReleaseMutex(self.0.as_raw_handle());
        }
    }
}

pub(super) fn observed_registration(name: &str) -> Result<(String, Option<PathBuf>)> {
    let mut sid = ptr::null_mut();
    let result =
        unsafe { DeriveAppContainerSidFromAppContainerName(wide(name).as_ptr(), &mut sid) };
    if result < 0 || sid.is_null() {
        return Err(Error::Invalid("original profile SID unavailable"));
    }
    struct Sid(PSID);
    impl Drop for Sid {
        fn drop(&mut self) {
            unsafe {
                FreeSid(self.0);
            }
        }
    }
    let sid = Sid(sid);
    let text = files::sid_string(sid.0)?;
    let mut folder = ptr::null_mut();
    let result = unsafe { GetAppContainerFolderPath(wide(&text).as_ptr(), &mut folder) };
    // Exact native missing-registration observation. Other HRESULTs, access
    // failures and a successful NULL pointer never establish absence.
    if result as u32 == 0x8007_0002 {
        return Ok((text, None));
    }
    if result < 0 || folder.is_null() {
        return Err(io::Error::other(format!(
            "original profile folder unavailable: 0x{:08x}",
            result as u32
        ))
        .into());
    }
    struct Allocation(*mut u16);
    impl Drop for Allocation {
        fn drop(&mut self) {
            unsafe {
                windows_sys::Win32::System::Com::CoTaskMemFree(self.0.cast());
            }
        }
    }
    let folder = Allocation(folder);
    let mut length = 0;
    while unsafe { *folder.0.add(length) } != 0 {
        length += 1;
        if length >= 32768 {
            return Err(Error::Invalid("original profile folder is oversized"));
        }
    }
    let path = String::from_utf16(unsafe { std::slice::from_raw_parts(folder.0, length) })
        .map_err(io::Error::other)?;
    Ok((text, Some(PathBuf::from(path))))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn own_profile_storage_is_removed_by_explicit_cleanup() {
        let name = format!("LocalAI.Cowork.V2.{}", uuid::Uuid::new_v4().simple());
        let mut identity = Identity::create(&name).unwrap();
        let directory = PathBuf::from(identity.profile_folder().unwrap());
        assert!(directory.is_dir());
        identity.close().unwrap();
        assert!(
            !directory.exists(),
            "Windows profile storage cleanup unconfirmed"
        );
    }
}
