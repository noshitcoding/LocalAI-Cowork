//! Retain the exact matching daemon process while requesting its orderly exit.
//! A port disappearing or a reused PID never proves that this process stopped.
use std::path::Path;

#[cfg(windows)]
pub struct DaemonProcess(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
unsafe impl Send for DaemonProcess {}
#[cfg(windows)]
impl Drop for DaemonProcess {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}
#[cfg(windows)]
impl DaemonProcess {
    pub fn current_application() -> Result<Self, String> {
        use windows_sys::Win32::{
            Foundation::DuplicateHandle,
            System::Threading::{GetCurrentProcess, PROCESS_SYNCHRONIZE},
        };
        let mut handle = std::ptr::null_mut();
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                GetCurrentProcess(),
                GetCurrentProcess(),
                &mut handle,
                PROCESS_SYNCHRONIZE,
                0,
                0,
            )
        } == 0
        {
            return Err("application_process_observation_failed".into());
        }
        Ok(Self(handle))
    }

    pub fn open(pid: u32, expected: &Path) -> Result<Self, String> {
        use windows_sys::Win32::System::Threading::*;
        let process = Self(unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        });
        if process.0.is_null() {
            return Err("matching_daemon_process_unavailable".into());
        }
        let mut buffer = vec![0u16; 32768];
        let mut count = buffer.len() as u32;
        if unsafe { QueryFullProcessImageNameW(process.0, 0, buffer.as_mut_ptr(), &mut count) } == 0
        {
            return Err("daemon_image_unavailable".into());
        }
        use std::os::windows::ffi::OsStringExt;
        let actual =
            std::path::PathBuf::from(std::ffi::OsString::from_wide(&buffer[..count as usize]));
        let actual = actual
            .canonicalize()
            .map_err(|_| "daemon_image_unavailable")?;
        let expected = expected
            .canonicalize()
            .map_err(|_| "daemon_image_unavailable")?;
        if !actual
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&expected.as_os_str().to_string_lossy())
        {
            return Err("daemon_image_does_not_match_installed_sidecar".into());
        }
        Ok(process)
    }
    pub fn wait_for_exit(&self, timeout_ms: u32) -> Result<bool, String> {
        use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::WaitForSingleObject;
        match unsafe { WaitForSingleObject(self.0, timeout_ms) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err("daemon_process_observation_failed".into()),
        }
    }
    pub fn duplicate_into(&self, child: &std::process::Child) -> Result<usize, String> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::DuplicateHandle,
            System::Threading::{GetCurrentProcess, PROCESS_SYNCHRONIZE},
        };
        let mut target = std::ptr::null_mut();
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                self.0,
                child.as_raw_handle(),
                &mut target,
                PROCESS_SYNCHRONIZE,
                0,
                0,
            )
        } == 0
        {
            return Err("daemon_process_handoff_failed".into());
        }
        Ok(target as usize)
    }
    /// Only a helper started by this application receives this process handle.
    pub unsafe fn from_transfer(value: usize) -> Result<Self, String> {
        if value == 0 || value == usize::MAX {
            return Err("invalid_process_handoff".into());
        }
        Ok(Self(value as windows_sys::Win32::Foundation::HANDLE))
    }
}

#[cfg(target_os = "linux")]
pub struct DaemonProcess(std::os::fd::OwnedFd);
#[cfg(target_os = "linux")]
impl DaemonProcess {
    pub fn open(pid: u32, expected: &Path) -> Result<Self, String> {
        use std::os::fd::FromRawFd;
        if pid <= 1 || pid > i32::MAX as u32 {
            return Err("matching_daemon_process_unavailable".into());
        }
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) };
        if fd < 0 {
            return Err("matching_daemon_process_unavailable".into());
        }
        let process = Self(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd as i32) });
        let actual = std::fs::read_link(format!("/proc/{pid}/exe"))
            .map_err(|_| "daemon_image_unavailable")?;
        if actual
            .canonicalize()
            .map_err(|_| "daemon_image_unavailable")?
            != expected
                .canonicalize()
                .map_err(|_| "daemon_image_unavailable")?
        {
            return Err("daemon_image_does_not_match_installed_sidecar".into());
        }
        Ok(process)
    }
    pub fn transfer_fd(&self) -> i32 {
        use std::os::fd::AsRawFd;
        self.0.as_raw_fd()
    }
    pub unsafe fn from_transfer(value: usize) -> Result<Self, String> {
        use std::os::fd::FromRawFd;
        if value < 3 || value > i32::MAX as usize {
            return Err("invalid_process_handoff".into());
        }
        // A regular file/pipe must never be mistaken for an exited process.
        let info = std::fs::read_to_string(format!("/proc/self/fdinfo/{value}"))
            .map_err(|_| "invalid_process_handoff")?;
        if !info.lines().any(|line| line.starts_with("Pid:\t")) {
            return Err("invalid_process_handoff".into());
        }
        Ok(Self(unsafe {
            std::os::fd::OwnedFd::from_raw_fd(value as i32)
        }))
    }
    pub fn wait_for_exit(&self, timeout_ms: u32) -> Result<bool, String> {
        use std::os::fd::AsRawFd;
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms as u64);
        loop {
            let left = deadline
                .saturating_duration_since(std::time::Instant::now())
                .as_millis()
                .min(i32::MAX as u128) as i32;
            let mut event = libc::pollfd {
                fd: self.0.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let result = unsafe { libc::poll(&mut event, 1, left) };
            if result > 0 && event.revents & libc::POLLIN != 0 {
                return Ok(true);
            }
            if result < 0
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR)
                && left > 0
            {
                continue;
            }
            if result == 0 {
                return Ok(false);
            }
            return Err("daemon_process_observation_failed".into());
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
pub struct DaemonProcess;
#[cfg(not(any(windows, target_os = "linux")))]
impl DaemonProcess {
    pub fn open(_: u32, _: &Path) -> Result<Self, String> {
        Err("unsupported_update_platform".into())
    }
    pub fn wait_for_exit(&self, _: u32) -> Result<bool, String> {
        Err("unsupported_update_platform".into())
    }
    pub unsafe fn from_transfer(_: usize) -> Result<Self, String> {
        Err("unsupported_update_platform".into())
    }
}
