use std::io;
use std::process::{Child, Command};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
use std::os::windows::io::AsRawHandle;

#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};

#[cfg(target_os = "windows")]
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

pub struct ProcessTreeGuard {
    #[cfg(target_os = "windows")]
    job: HANDLE,
    #[cfg(unix)]
    process_group: i32,
}

// The guard owns an OS handle or process-group identifier and may be moved to
// the monitor thread that owns the corresponding child process.
unsafe impl Send for ProcessTreeGuard {}

#[cfg(target_os = "windows")]
impl Drop for ProcessTreeGuard {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.job);
        }
    }
}

#[cfg(unix)]
impl Drop for ProcessTreeGuard {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-self.process_group, libc::SIGKILL);
        }
    }
}

pub fn configure_process_tree(command: &mut Command) {
    #[cfg(unix)]
    command.process_group(0);

    #[cfg(not(unix))]
    let _ = command;
}

pub fn attach_process_tree(child: &Child) -> io::Result<ProcessTreeGuard> {
    #[cfg(target_os = "windows")]
    {
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }

        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            let error = io::Error::last_os_error();
            unsafe {
                CloseHandle(job);
            }
            return Err(error);
        }

        let process_handle = child.as_raw_handle() as HANDLE;
        if unsafe { AssignProcessToJobObject(job, process_handle) } == 0 {
            let error = io::Error::last_os_error();
            unsafe {
                CloseHandle(job);
            }
            return Err(error);
        }

        Ok(ProcessTreeGuard { job })
    }

    #[cfg(not(target_os = "windows"))]
    {
        #[cfg(unix)]
        {
            Ok(ProcessTreeGuard {
                process_group: child.id() as i32,
            })
        }

        #[cfg(not(unix))]
        {
            let _ = child;
            Ok(ProcessTreeGuard {})
        }
    }
}

#[cfg(target_os = "windows")]
fn terminate_platform_tree(
    process_id: u32,
    process_tree: Option<&ProcessTreeGuard>,
) -> io::Result<()> {
    if let Some(process_tree) = process_tree {
        if unsafe { TerminateJobObject(process_tree.job, 1) } != 0 {
            return Ok(());
        }
    }

    let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let taskkill = std::path::PathBuf::from(system_root)
        .join("System32")
        .join("taskkill.exe");
    let status = Command::new(taskkill)
        .args(["/PID", &process_id.to_string(), "/T", "/F"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "taskkill exited with status {status}"
        )))
    }
}

#[cfg(unix)]
fn terminate_platform_tree(
    process_id: u32,
    _process_tree: Option<&ProcessTreeGuard>,
) -> io::Result<()> {
    let process_group = -(process_id as i32);
    let result = unsafe { libc::kill(process_group, libc::SIGKILL) };
    if result == 0 {
        return Ok(());
    }

    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

#[cfg(not(any(target_os = "windows", unix)))]
fn terminate_platform_tree(
    _process_id: u32,
    _process_tree: Option<&ProcessTreeGuard>,
) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "process-tree termination is not supported on this platform",
    ))
}

pub fn terminate_process_tree(
    child: &mut Child,
    process_tree: Option<&ProcessTreeGuard>,
) -> io::Result<()> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }

    let tree_result = terminate_platform_tree(child.id(), process_tree);
    let terminated = tree_result.is_ok() || child.kill().is_ok();
    if !terminated {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        return tree_result;
    }

    child.wait().map(|_| ())
}
