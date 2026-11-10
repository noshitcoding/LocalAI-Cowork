use super::*;
use std::{
    fs::File,
    io::{self, Read},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    ptr,
};
use windows_sys::Win32::{
    Foundation::*,
    Security::SECURITY_ATTRIBUTES,
    System::{JobObjects::*, Pipes::*, Threading::*},
};

pub(super) struct Child {
    process: OwnedHandle,
    job: OwnedHandle,
    pid: u32,
    out: Option<File>,
    err: Option<File>,
}
fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain([0]).collect()
}
fn handle(h: HANDLE) -> io::Result<OwnedHandle> {
    if h.is_null() || h == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedHandle::from_raw_handle(h) })
    }
}
fn raw(h: &OwnedHandle) -> HANDLE {
    h.as_raw_handle()
}
fn pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let security = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: 1,
    };
    let (mut read, mut write) = (ptr::null_mut(), ptr::null_mut());
    if unsafe { CreatePipe(&mut read, &mut write, &security, 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((handle(read)?, handle(write)?))
}
struct Attributes {
    memory: Vec<usize>,
    initialized: bool,
}
impl Attributes {
    fn new(count: u32) -> io::Result<Self> {
        let mut bytes = 0;
        unsafe {
            InitializeProcThreadAttributeList(ptr::null_mut(), count, 0, &mut bytes);
        }
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut value = Self {
            memory: vec![0; bytes.div_ceil(std::mem::size_of::<usize>())],
            initialized: false,
        };
        if unsafe { InitializeProcThreadAttributeList(value.pointer(), count, 0, &mut bytes) } == 0
        {
            return Err(io::Error::last_os_error());
        }
        value.initialized = true;
        Ok(value)
    }
    fn pointer(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.memory.as_mut_ptr().cast()
    }
    fn handles(&mut self, attribute: usize, handles: &mut [HANDLE]) -> io::Result<()> {
        if unsafe {
            UpdateProcThreadAttribute(
                self.pointer(),
                0,
                attribute,
                handles.as_mut_ptr().cast(),
                std::mem::size_of_val(handles),
                ptr::null_mut(),
                ptr::null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    fn value<T>(&mut self, attribute: usize, value: &mut T) -> io::Result<()> {
        if unsafe {
            UpdateProcThreadAttribute(
                self.pointer(),
                0,
                attribute,
                (value as *mut T).cast(),
                std::mem::size_of::<T>(),
                ptr::null_mut(),
                ptr::null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        if self.initialized {
            unsafe {
                DeleteProcThreadAttributeList(self.pointer());
            }
        }
    }
}

// Win32 CRT argv quoting. This layer never implicitly selects cmd.exe or PowerShell.
fn quote(value: &str) -> String {
    let mut output = String::from("\"");
    let mut slashes = 0;
    for c in value.chars() {
        if c == '\\' {
            slashes += 1;
            continue;
        }
        if c == '\"' {
            output.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
        } else {
            output.extend(std::iter::repeat_n('\\', slashes));
        }
        slashes = 0;
        output.push(c);
    }
    output.extend(std::iter::repeat_n('\\', slashes * 2));
    output.push('\"');
    output
}
impl Child {
    pub fn spawn(launch: &TrustedLaunch) -> Result<Self> {
        Self::spawn_with(launch, None)
    }
    pub(crate) fn spawn_isolated(
        launch: &TrustedLaunch,
        identity: &windows_isolation::Identity,
        limits: windows_isolation::Limits,
        profile: windows_isolation::Profile,
    ) -> Result<Self> {
        limits.validate()?;
        Self::spawn_with(launch, Some((identity, limits, profile)))
    }
    fn spawn_with(
        launch: &TrustedLaunch,
        isolation: Option<(
            &windows_isolation::Identity,
            windows_isolation::Limits,
            windows_isolation::Profile,
        )>,
    ) -> Result<Self> {
        let headless = matches!(
            isolation,
            Some((_, _, windows_isolation::Profile::Headless))
        );
        let program = launch
            .program
            .to_str()
            .ok_or(Error::Invalid("program path must be valid Unicode"))?;
        let mut command = wide(std::ffi::OsStr::new(
            &std::iter::once(quote(program))
                .chain(launch.arguments.iter().map(|s| quote(s)))
                .collect::<Vec<_>>()
                .join(" "),
        ));
        if command.len() > 32_767 {
            return Err(Error::Invalid("Windows command line is too long"));
        }
        let program = wide(launch.program.as_os_str());
        let directory = wide(launch.directory.as_os_str());
        let mut entries: Vec<_> = launch.environment.iter().collect();
        entries.sort_by_cached_key(|(key, _)| key.to_ascii_uppercase());
        let mut environment: Vec<u16> = entries
            .into_iter()
            .flat_map(|(k, v)| {
                format!("{k}={v}")
                    .encode_utf16()
                    .chain([0])
                    .collect::<Vec<_>>()
            })
            .collect();
        environment.push(0);
        if environment.len() == 1 {
            environment.push(0);
        }
        let job_name = isolation.map(|(identity, _, _)| identity.job_name());
        let job = handle(unsafe {
            CreateJobObjectW(
                ptr::null(),
                job_name.as_ref().map_or(ptr::null(), |name| name.as_ptr()),
            )
        })?;
        if isolation.is_some() && unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            return Err(Error::Invalid("capsule Job Object already exists"));
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if let Some((_, bound, _)) = isolation {
            limits.BasicLimitInformation.LimitFlags |=
                JOB_OBJECT_LIMIT_JOB_MEMORY | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
            limits.BasicLimitInformation.ActiveProcessLimit = bound.processes;
            limits.JobMemoryLimit = bound.memory_bytes;
        }
        if unsafe {
            SetInformationJobObject(
                raw(&job),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        if let Some((_, bound, _)) = isolation {
            let processors = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) };
            if processors == 0 {
                return Err(io::Error::last_os_error().into());
            }
            // Floor rather than round up: a small requested allocation may fail closed.
            let rate =
                (u64::from(bound.cpu_millis) * 10 / u64::from(processors)).min(10_000) as u32;
            if rate == 0 {
                return Err(Error::Invalid(
                    "CPU allocation is below Windows rate resolution",
                ));
            }
            let cpu = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION {
                ControlFlags: JOB_OBJECT_CPU_RATE_CONTROL_ENABLE
                    | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
                Anonymous: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0 { CpuRate: rate },
            };
            if unsafe {
                SetInformationJobObject(
                    raw(&job),
                    JobObjectCpuRateControlInformation,
                    (&cpu as *const JOBOBJECT_CPU_RATE_CONTROL_INFORMATION).cast(),
                    std::mem::size_of_val(&cpu) as u32,
                )
            } == 0
            {
                return Err(io::Error::last_os_error().into());
            }
            let ui = JOBOBJECT_BASIC_UI_RESTRICTIONS {
                UIRestrictionsClass: JOB_OBJECT_UILIMIT_DESKTOP
                    | JOB_OBJECT_UILIMIT_HANDLES
                    | JOB_OBJECT_UILIMIT_READCLIPBOARD
                    | JOB_OBJECT_UILIMIT_WRITECLIPBOARD
                    | JOB_OBJECT_UILIMIT_GLOBALATOMS
                    | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS
                    | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
                    | JOB_OBJECT_UILIMIT_EXITWINDOWS,
            };
            if unsafe {
                SetInformationJobObject(
                    raw(&job),
                    JobObjectBasicUIRestrictions,
                    (&ui as *const JOBOBJECT_BASIC_UI_RESTRICTIONS).cast(),
                    std::mem::size_of_val(&ui) as u32,
                )
            } == 0
            {
                return Err(io::Error::last_os_error().into());
            }
        }
        let (input_read, input_write) = pipe()?;
        drop(input_write); // stdin is EOF from the start.
        let (out_read, out_write) = pipe()?;
        let (err_read, err_write) = pipe()?;
        for reader in [&out_read, &err_read] {
            if unsafe { SetHandleInformation(raw(reader), HANDLE_FLAG_INHERIT, 0) } == 0 {
                return Err(io::Error::last_os_error().into());
            }
        }
        let mut handles = [raw(&input_read), raw(&out_write), raw(&err_write)];
        let mut jobs = [raw(&job)];
        // UpdateProcThreadAttribute retains these addresses until attributes is dropped.
        let mut capabilities = isolation.map(|(identity, _, _)| identity.capabilities());
        let mut lpac_opt_out = 1u32;
        // PROCESS_CREATION_MITIGATION_POLICY_WIN32K_SYSTEM_CALL_DISABLE_ALWAYS_ON.
        // The headless profile never falls back to GUI access; only the separately
        // reviewed plain-AppContainer profile keeps Win32k (see WINDOWS-ISOLATION.md).
        let mut mitigation = 1u64 << 28;
        let mut attributes = Attributes::new(match (isolation.is_some(), headless) {
            (false, _) => 2,
            (true, true) => 5,
            (true, false) => 3,
        })?;
        attributes.handles(PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize, &mut handles)?;
        // Atomic job association during process creation closes the suspended-orphan
        // window between CreateProcess and AssignProcessToJobObject on parent death.
        attributes.handles(PROC_THREAD_ATTRIBUTE_JOB_LIST as usize, &mut jobs)?;
        if let Some(capabilities) = capabilities.as_mut() {
            attributes.value(
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                capabilities,
            )?;
        }
        if headless {
            attributes.value(
                PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY as usize,
                &mut lpac_opt_out,
            )?;
            attributes.value(
                PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY as usize,
                &mut mitigation,
            )?;
        }
        let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
        startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = raw(&input_read);
        startup.StartupInfo.hStdOutput = raw(&out_write);
        startup.StartupInfo.hStdError = raw(&err_write);
        if let Some((identity, _, _)) = isolation {
            startup.StartupInfo.lpDesktop = identity.desktop();
        }
        startup.lpAttributeList = attributes.pointer();
        let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe {
            CreateProcessW(
                program.as_ptr(),
                command.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                1,
                CREATE_NO_WINDOW
                    | CREATE_SUSPENDED
                    | CREATE_UNICODE_ENVIRONMENT
                    | EXTENDED_STARTUPINFO_PRESENT,
                environment.as_ptr().cast(),
                directory.as_ptr(),
                &startup.StartupInfo,
                &mut info,
            )
        } == 0
        {
            return Err(io::Error::other(format!(
                "CreateProcessW: {}",
                io::Error::last_os_error()
            ))
            .into());
        }
        let process = handle(info.hProcess)?;
        let thread = handle(info.hThread)?;
        let child = Self {
            process,
            job,
            pid: info.dwProcessId,
            out: Some(File::from(out_read)),
            err: Some(File::from(err_read)),
        };
        if let Some((identity, _, _)) = isolation {
            let mut syscall_policy = 0u32;
            if headless
                && unsafe {
                    GetProcessMitigationPolicy(
                        raw(&child.process),
                        ProcessSystemCallDisablePolicy,
                        (&mut syscall_policy as *mut u32).cast(),
                        std::mem::size_of_val(&syscall_policy),
                    )
                } == 0
            {
                return Err(io::Error::last_os_error().into());
            }
            if headless && syscall_policy & 1 == 0 {
                return Err(Error::Invalid("Win32k calls are not disabled"));
            }
            identity
                .verify_process(raw(&child.process), headless)
                .map_err(|e| io::Error::other(format!("verify isolated child: {e:?}")))?;
            let mut in_job = 0;
            if unsafe { IsProcessInJob(raw(&child.process), raw(&child.job), &mut in_job) } == 0 {
                return Err(io::Error::last_os_error().into());
            }
            if in_job != 1 {
                return Err(Error::Invalid("isolated child is outside its job"));
            }
        }
        if unsafe { ResumeThread(raw(&thread)) } == u32::MAX {
            return Err(io::Error::last_os_error().into());
        }
        Ok(child)
    }
    pub fn id(&self) -> u32 {
        self.pid
    }
    pub fn try_wait(&mut self) -> Result<Option<Exit>> {
        match unsafe { WaitForSingleObject(raw(&self.process), 0) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                let mut code = 0;
                if unsafe { GetExitCodeProcess(raw(&self.process), &mut code) } == 0 {
                    return Err(io::Error::last_os_error().into());
                }
                Ok(Some(Exit {
                    code: Some(code as i64),
                    signal: None,
                }))
            }
            _ => Err(io::Error::last_os_error().into()),
        }
    }
    pub fn terminate(&mut self) -> Result<()> {
        if unsafe { TerminateJobObject(raw(&self.job), 130) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(())
    }
    pub fn wait(&mut self, timeout: Duration) -> Result<Exit> {
        let deadline = Instant::now() + timeout;
        if unsafe {
            WaitForSingleObject(
                raw(&self.process),
                timeout.as_millis().min(u32::MAX as u128) as u32,
            )
        } != WAIT_OBJECT_0
        {
            return Err(
                io::Error::new(io::ErrorKind::TimedOut, "process termination unconfirmed").into(),
            );
        }
        // Root exit alone is insufficient evidence that every managed process stopped.
        loop {
            let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION =
                unsafe { std::mem::zeroed() };
            if unsafe {
                QueryInformationJobObject(
                    raw(&self.job),
                    JobObjectBasicAccountingInformation,
                    (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                    std::mem::size_of_val(&accounting) as u32,
                    ptr::null_mut(),
                )
            } == 0
            {
                return Err(io::Error::last_os_error().into());
            }
            if accounting.ActiveProcesses == 0 {
                break;
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "job process termination unconfirmed",
                )
                .into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        self.try_wait()?
            .ok_or_else(|| io::Error::other("process exit missing").into())
    }
    pub fn drain(&mut self, out: &mut Capture, err: &mut Capture, bound: usize) -> Result<bool> {
        read(&mut self.out, out, bound)?;
        read(&mut self.err, err, bound)?;
        Ok(self.output_complete())
    }
    pub fn output_complete(&self) -> bool {
        self.out.is_none() && self.err.is_none()
    }
}
fn read(pipe: &mut Option<File>, capture: &mut Capture, bound: usize) -> io::Result<()> {
    let mut data = [0; 8192];
    for _ in 0..8 {
        let Some(reader) = pipe.as_mut() else { break };
        let mut available = 0;
        if unsafe {
            PeekNamedPipe(
                reader.as_raw_handle(),
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                &mut available,
                ptr::null_mut(),
            )
        } == 0
        {
            let e = io::Error::last_os_error();
            if e.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) {
                *pipe = None;
                break;
            }
            return Err(e);
        }
        if available == 0 {
            break;
        }
        let n = reader.read(&mut data[..(available as usize).min(8192)])?;
        if n == 0 {
            *pipe = None;
            break;
        }
        capture.append(&data[..n], bound);
    }
    Ok(())
}
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.terminate();
        let _ = self.wait(Duration::from_secs(1));
    }
}
