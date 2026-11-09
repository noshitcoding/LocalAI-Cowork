use super::*;
use std::{
    io::{self, Read},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{ffi::OsStrExt, process::CommandExt},
    },
    path::Path,
    process::{Command, Stdio},
};

pub(super) struct Child {
    process: std::process::Child,
    out: Option<std::process::ChildStdout>,
    err: Option<std::process::ChildStderr>,
    group_killed: bool,
}
/// Trusted native cgroup capability for placing a newly forked control child
/// before exec. It never moves the supervisor or adopts an existing process.
pub struct CgroupPlacement {
    _directory: std::fs::File,
    procs: std::fs::File,
}
impl CgroupPlacement {
    pub fn from_directory(directory: std::fs::File) -> Result<Self> {
        use std::os::unix::fs::MetadataExt;
        let m = directory.metadata()?;
        let mut fs = std::mem::MaybeUninit::<libc::statfs>::zeroed();
        if unsafe { libc::fstatfs(directory.as_raw_fd(), fs.as_mut_ptr()) } < 0 {
            return Err(io::Error::last_os_error().into());
        }
        if unsafe { fs.assume_init() }.f_type != 0x63677270
            || !m.is_dir()
            || m.uid() != 0
            || m.mode() & 0o022 != 0
        {
            return Err(Error::Invalid("invalid native cgroup capability"));
        }
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                c"cgroup.procs".as_ptr(),
                libc::O_WRONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Self {
            _directory: directory,
            procs: unsafe { std::fs::File::from_raw_fd(fd) },
        })
    }
}
impl Child {
    pub fn spawn(launch: &TrustedLaunch) -> Result<Self> {
        Self::spawn_with(launch, None, None)
    }
    pub(crate) fn spawn_confined(launch: &TrustedLaunch, sandbox: &Sandbox) -> Result<Self> {
        Self::spawn_with(launch, Some(sandbox), None)
    }
    pub(super) fn spawn_placed(
        launch: &TrustedLaunch,
        placement: &CgroupPlacement,
    ) -> Result<Self> {
        Self::spawn_with(launch, None, Some(placement))
    }
    fn spawn_with(
        launch: &TrustedLaunch,
        sandbox: Option<&Sandbox>,
        placement: Option<&CgroupPlacement>,
    ) -> Result<Self> {
        let parent = unsafe { libc::getpid() };
        // Prepared before fork; the child only patches its own PID into its copy.
        let ruleset = sandbox.map(|s| s.ruleset.as_raw_fd());
        let mut filter = sandbox.map(|s| s.filter.clone()).unwrap_or_default();
        let pid_check = sandbox.map_or(0, |s| s.pid_check);
        let memory = sandbox.map_or(0, |s| s.memory) as libc::rlim_t;
        let cgroup_procs = placement.map(|p| p.procs.as_raw_fd());
        let mut command = Command::new(&launch.program);
        command
            .args(&launch.arguments)
            .current_dir(&launch.directory)
            .env_clear()
            .envs(&launch.environment)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        // Only async-signal-safe syscalls execute between fork and exec.
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0
                    || libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) != 0
                {
                    return Err(io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    return Err(io::Error::other("supervisor exited before child admission"));
                }
                if let Some(fd) = cgroup_procs {
                    let mut pid = libc::getpid() as u32;
                    let mut bytes = [0u8; 20];
                    let mut at = bytes.len();
                    loop {
                        at -= 1;
                        bytes[at] = b'0' + (pid % 10) as u8;
                        pid /= 10;
                        if pid == 0 {
                            break;
                        }
                    }
                    let count = bytes.len() - at;
                    if libc::write(fd, bytes[at..].as_ptr().cast(), count) != count as isize {
                        return Err(io::Error::last_os_error());
                    }
                }
                if let Some(ruleset) = ruleset {
                    let memory = libc::rlimit {
                        rlim_cur: memory,
                        rlim_max: memory,
                    };
                    let core = libc::rlimit {
                        rlim_cur: 0,
                        rlim_max: 0,
                    };
                    if libc::setrlimit(libc::RLIMIT_AS, &memory) != 0
                        || libc::setrlimit(libc::RLIMIT_CORE, &core) != 0
                        || libc::syscall(libc::SYS_landlock_restrict_self, ruleset, 0u32) != 0
                    {
                        return Err(io::Error::last_os_error());
                    }
                    filter[pid_check].k = libc::getpid() as u32;
                    let program = Program {
                        len: filter.len() as u16,
                        filter: filter.as_ptr(),
                    };
                    // SECCOMP_MODE_FILTER; needs no_new_privs (set above) and survives exec.
                    if libc::prctl(libc::PR_SET_SECCOMP, 2, &program as *const Program) != 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        let mut process = command.spawn()?;
        let out = process.stdout.take();
        let err = process.stderr.take();
        let child = Self {
            process,
            out,
            err,
            group_killed: false,
        };
        for fd in [
            child.out.as_ref().unwrap().as_raw_fd(),
            child.err.as_ref().unwrap().as_raw_fd(),
        ] {
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
            {
                return Err(io::Error::last_os_error().into());
            }
        }
        Ok(child)
    }
    pub fn id(&self) -> u32 {
        self.process.id()
    }
    pub fn try_wait(&mut self) -> Result<Option<Exit>> {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // Keep the root as a waitable child until group termination. Reaping first could
        // allow PID/group-ID reuse and direct a later kill at an unrelated process.
        if unsafe {
            libc::waitid(
                libc::P_PID,
                self.id(),
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        } != 0
        {
            return Err(io::Error::last_os_error().into());
        }
        if unsafe { info.si_pid() } == 0 {
            return Ok(None);
        }
        let status = unsafe { info.si_status() };
        Ok(Some(if info.si_code == libc::CLD_EXITED {
            Exit {
                code: Some(status as i64),
                signal: None,
            }
        } else {
            Exit {
                code: None,
                signal: Some(status),
            }
        }))
    }
    pub fn terminate(&mut self) -> Result<()> {
        if self.group_killed {
            return Ok(());
        }
        if unsafe { libc::kill(-(self.id() as i32), libc::SIGKILL) } != 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() != Some(libc::ESRCH) {
                return Err(e.into());
            }
        }
        self.group_killed = true;
        Ok(())
    }
    pub fn wait(&mut self, timeout: Duration) -> Result<Exit> {
        let until = Instant::now() + timeout;
        loop {
            if let Some(exit) = self.try_wait()? {
                return Ok(exit);
            }
            if Instant::now() >= until {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "process termination unconfirmed",
                )
                .into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
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
fn read<R: Read>(pipe: &mut Option<R>, capture: &mut Capture, bound: usize) -> io::Result<()> {
    let mut data = [0; 8192];
    // Per-tick bound ensures an output flood cannot starve cancellation/deadlines.
    for _ in 0..8 {
        let Some(reader) = pipe.as_mut() else { break };
        match reader.read(&mut data) {
            Ok(0) => {
                *pipe = None;
                break;
            }
            Ok(n) => capture.append(&data[..n], bound),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.terminate();
        if self.wait(Duration::from_secs(1)).is_ok() {
            let _ = self.process.try_wait();
        }
    }
}

// Landlock filesystem access rights, ABI 1-5 (linux/landlock.h).
const EXECUTE: u64 = 1 << 0;
const WRITE_FILE: u64 = 1 << 1;
const READ_FILE: u64 = 1 << 2;
const READ_DIR: u64 = 1 << 3;
const REMOVE_DIR: u64 = 1 << 4;
const REMOVE_FILE: u64 = 1 << 5;
const MAKE_DIR: u64 = 1 << 7;
const MAKE_REG: u64 = 1 << 8;
const REFER: u64 = 1 << 13;
const TRUNCATE: u64 = 1 << 14;
const IOCTL_DEV: u64 = 1 << 15;
/// Rights a rule on a single file may carry; others make landlock_add_rule fail.
const FILE_RIGHTS: u64 = EXECUTE | WRITE_FILE | READ_FILE | TRUNCATE | IOCTL_DEV;
/// Writable trees: no execution, symlinks, devices, sockets or FIFOs.
const WRITE_RIGHTS: u64 = WRITE_FILE
    | READ_FILE
    | READ_DIR
    | REMOVE_DIR
    | REMOVE_FILE
    | MAKE_DIR
    | MAKE_REG
    | REFER
    | TRUNCATE;

#[repr(C)]
struct RulesetAttr {
    handled_access_fs: u64,
    handled_access_net: u64,
    scoped: u64,
}
#[repr(C, packed)]
struct PathBeneath {
    allowed_access: u64,
    parent_fd: i32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct Filter {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}
#[repr(C)]
struct Program {
    len: u16,
    filter: *const Filter,
}

/// Landlock ABI version, or 0 when the kernel lacks Landlock or it is disabled.
pub(crate) fn landlock_abi() -> i64 {
    // LANDLOCK_CREATE_RULESET_VERSION
    let abi = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<RulesetAttr>(),
            0usize,
            1u32,
        )
    };
    abi.max(0) as i64
}
pub(crate) fn sandbox_supported() -> bool {
    filter().is_some()
        && landlock_abi() >= 3
        && unsafe { libc::prctl(libc::PR_GET_SECCOMP, 0, 0, 0, 0) } >= 0
}

/// A Landlock ruleset (ABI >= 3) plus a seccomp filter, applied in the child right before
/// exec. File access is limited to the listed trees; the filter denies sockets, io_uring,
/// new processes (threads stay allowed) and signals or pidfds for other processes.
pub(crate) struct Sandbox {
    ruleset: OwnedFd,
    filter: Vec<Filter>,
    pid_check: usize,
    memory: usize,
}
impl Sandbox {
    pub(crate) fn prepare(
        program: &Path,
        read: &[PathBuf],
        write: &[PathBuf],
        memory: usize,
    ) -> Result<Self> {
        let (filter, pid_check) =
            filter().ok_or(Error::Invalid("seccomp filter unsupported on this CPU"))?;
        let abi = landlock_abi();
        if abi < 3 {
            return Err(Error::Invalid("Landlock ABI 3 or newer is unavailable"));
        }
        let mut handled = (TRUNCATE << 1) - 1; // every filesystem right of ABI 3
        if abi >= 5 {
            handled |= IOCTL_DEV;
        }
        let attr = RulesetAttr {
            handled_access_fs: handled,
            // ABI 4: TCP bind/connect; ABI 6: abstract UNIX sockets and signals.
            handled_access_net: if abi >= 4 { 0b11 } else { 0 },
            scoped: if abi >= 6 { 0b11 } else { 0 },
        };
        let size = match abi {
            3 => 8usize,
            4 | 5 => 16,
            _ => 24,
        };
        let fd = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                &attr as *const RulesetAttr,
                size,
                0u32,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error().into());
        }
        let ruleset = unsafe { OwnedFd::from_raw_fd(fd as i32) };
        for (paths, writable) in [(read, false), (write, true)] {
            for path in paths {
                let name = std::ffi::CString::new(path.as_os_str().as_bytes())
                    .map_err(|_| Error::Invalid("confined path contains NUL"))?;
                let fd = unsafe { libc::open(name.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
                if fd < 0 {
                    return Err(io::Error::last_os_error().into());
                }
                let fd = unsafe { OwnedFd::from_raw_fd(fd) };
                let mut access = if writable {
                    WRITE_RIGHTS
                } else {
                    READ_FILE | READ_DIR
                };
                // Only the program itself (or the tree holding it) may be executed.
                if !writable && program.starts_with(path) {
                    access |= EXECUTE;
                }
                if !std::fs::metadata(path)?.is_dir() {
                    access &= FILE_RIGHTS;
                }
                let rule = PathBeneath {
                    allowed_access: access & handled,
                    parent_fd: fd.as_raw_fd(),
                };
                // LANDLOCK_RULE_PATH_BENEATH
                if unsafe {
                    libc::syscall(
                        libc::SYS_landlock_add_rule,
                        ruleset.as_raw_fd(),
                        1u32,
                        &rule as *const PathBeneath,
                        0u32,
                    )
                } != 0
                {
                    return Err(io::Error::last_os_error().into());
                }
            }
        }
        Ok(Self {
            ruleset,
            filter,
            pid_check,
            memory,
        })
    }
}

#[cfg(target_arch = "x86_64")]
const ARCH: Option<u32> = Some(0xC000_003E);
#[cfg(target_arch = "aarch64")]
const ARCH: Option<u32> = Some(0xC000_00B7);
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
const ARCH: Option<u32> = None;

enum Check {
    Deny,
    Clone,
    NoSys,
    OwnPid,
}

/// Classic BPF over `struct seccomp_data`; jump offsets count from the next instruction.
/// Returns the program and the index of the comparison the child patches with its PID.
fn filter() -> Option<(Vec<Filter>, usize)> {
    let arch = ARCH?;
    const LOAD: u16 = 0x20; // BPF_LD | BPF_W | BPF_ABS
    const EQUAL: u16 = 0x15; // BPF_JMP | BPF_JEQ | BPF_K
    const AT_LEAST: u16 = 0x35; // BPF_JMP | BPF_JGE | BPF_K
    const ANY_BIT: u16 = 0x45; // BPF_JMP | BPF_JSET | BPF_K
    const RETURN: u16 = 0x06; // BPF_RET | BPF_K
    const ALLOW: u32 = 0x7fff_0000;
    const KILL: u32 = 0x8000_0000; // SECCOMP_RET_KILL_PROCESS
    let errno = |e: i32| 0x0005_0000 | e as u32;
    let mut checks: Vec<(libc::c_long, Check)> = vec![
        (libc::SYS_socket, Check::Deny),
        (libc::SYS_io_uring_setup, Check::Deny),
        (libc::SYS_clone, Check::Clone),
        // Callers fall back to clone, which the filter can inspect.
        (libc::SYS_clone3, Check::NoSys),
    ];
    #[cfg(target_arch = "x86_64")]
    checks.extend([
        (libc::SYS_fork, Check::Deny),
        (libc::SYS_vfork, Check::Deny),
    ]);
    for number in [
        libc::SYS_kill,
        libc::SYS_tkill,
        libc::SYS_tgkill,
        libc::SYS_rt_sigqueueinfo,
        libc::SYS_rt_tgsigqueueinfo,
        libc::SYS_pidfd_open,
    ] {
        checks.push((number, Check::OwnPid));
    }
    let x32 = cfg!(target_arch = "x86_64");
    // [head] [x32 guard] [dispatch] [allow] [clone: 2] [own pid: 4] [allow] [EPERM] [ENOSYS]
    let head = 4 + usize::from(x32);
    let clone_at = head + checks.len() + 1;
    let pid_at = clone_at + 2;
    let allow_at = pid_at + 4;
    let deny_at = allow_at + 1;
    let nosys_at = deny_at + 1;
    let jump = |from: usize, to: usize| (to - from - 1) as u8;
    let op = |code, jt, jf, k| Filter { code, jt, jf, k };
    let mut program = vec![
        op(LOAD, 0, 0, 4), // arch
        op(EQUAL, 1, 0, arch),
        op(RETURN, 0, 0, KILL),
        op(LOAD, 0, 0, 0), // syscall number
    ];
    if x32 {
        // __X32_SYSCALL_BIT: the x32 ABI has its own numbers; deny all of it.
        program.push(op(AT_LEAST, jump(4, deny_at), 0, 0x4000_0000));
    }
    for (index, (number, kind)) in checks.iter().enumerate() {
        let target = match kind {
            Check::Deny => deny_at,
            Check::Clone => clone_at,
            Check::NoSys => nosys_at,
            Check::OwnPid => pid_at,
        };
        program.push(op(EQUAL, jump(head + index, target), 0, *number as u32));
    }
    program.push(op(RETURN, 0, 0, ALLOW));
    // clone: threads only (CLONE_THREAD in the low word of the flags argument).
    program.push(op(LOAD, 0, 0, 16));
    program.push(op(
        ANY_BIT,
        jump(clone_at + 1, allow_at),
        jump(clone_at + 1, deny_at),
        libc::CLONE_THREAD as u32,
    ));
    // Signals and pidfds: only the child's own PID, both words of the 64-bit argument.
    program.push(op(LOAD, 0, 0, 16));
    program.push(op(EQUAL, 0, jump(pid_at + 1, deny_at), 0)); // patched with the PID
    program.push(op(LOAD, 0, 0, 20));
    program.push(op(
        EQUAL,
        jump(pid_at + 3, allow_at),
        jump(pid_at + 3, deny_at),
        0,
    ));
    program.push(op(RETURN, 0, 0, ALLOW));
    program.push(op(RETURN, 0, 0, errno(libc::EPERM)));
    program.push(op(RETURN, 0, 0, errno(libc::ENOSYS)));
    debug_assert_eq!(program.len(), nosys_at + 1);
    Some((program, pid_at + 1))
}
