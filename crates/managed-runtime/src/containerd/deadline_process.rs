//! Fixed-FD physical watchdog, independent of the runner's lifetime. No socket
//! listener, paths from requests, generic command execution or discovery adoption.
use super::*;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::{
    io::Write,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::process::CommandExt,
    },
    process::{Child, Command, Stdio},
};

pub struct FenceExecutable {
    executable: super::daemon::DaemonExecutable,
}
impl FenceExecutable {
    pub fn open(path: &Path, expected_sha256: &str) -> Result<Self> {
        Ok(Self {
            executable: super::daemon::DaemonExecutable::open(path, expected_sha256)?,
        })
    }
    pub(super) fn verify(&self) -> Result<()> {
        self.executable.verify()
    }
}
pub(super) struct DeadlineProcess {
    child: Option<Child>,
    channel: StdUnixStream,
    peer: ExpectedPeer,
    finished: bool,
}
impl DeadlineProcess {
    pub fn start(
        executable: &FenceExecutable,
        directory: &cap_std::fs::Dir,
        deadline: std::time::Instant,
    ) -> Result<Self> {
        ensure_reaper()?;
        executable.executable.verify()?;
        let dir = directory.open(".")?.into_std();
        let m = dir.metadata()?;
        let owner = unsafe { libc::syscall(libc::SYS_pidfd_open, libc::getpid(), 0) };
        if owner < 0 {
            return Err(io::Error::last_os_error().into());
        }
        let owner = unsafe { OwnedFd::from_raw_fd(owner as i32) };
        let (channel, child_channel) = StdUnixStream::pair()?;
        let duplicate = |fd: i32| -> Result<OwnedFd> {
            let n = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 64) };
            if n < 0 {
                return Err(io::Error::last_os_error().into());
            }
            Ok(unsafe { OwnedFd::from_raw_fd(n) })
        };
        let descriptors = [
            duplicate(dir.as_raw_fd())?,
            duplicate(owner.as_raw_fd())?,
            duplicate(child_channel.as_raw_fd())?,
        ];
        let raw = descriptors.each_ref().map(|fd| fd.as_raw_fd());
        let remaining = deadline
            .saturating_duration_since(std::time::Instant::now())
            .as_nanos();
        let target = boottime()?
            .checked_add(remaining.try_into().map_err(|_| Error::TaskAdmission)?)
            .ok_or(Error::TaskAdmission)?;
        let program = PathBuf::from(format!(
            "/proc/{}/fd/{}",
            std::process::id(),
            executable.executable.file.as_raw_fd()
        ));
        let mut command = Command::new(program);
        command
            .args([target.to_string(), m.dev().to_string(), m.ino().to_string()])
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0
                    || libc::prctl(libc::PR_SET_PDEATHSIG, 0, 0, 0, 0) != 0
                {
                    return Err(io::Error::last_os_error());
                }
                for (source, target) in raw.into_iter().zip([3, 4, 5]) {
                    if libc::dup2(source, target) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        let mut child = command.spawn()?;
        drop(descriptors);
        drop(child_channel);
        // The original owned child must execute the exact pinned watchdog before
        // readiness. A PID or a stale pidfile is never accepted for adoption.
        let peer =
            match ExpectedPeer::capture_new_child(child.id(), 0, &executable.executable.sha256) {
                Ok(peer) => peer,
                Err(error) => {
                    // No task can have been submitted yet. Closing the inherited
                    // channel also triggers any helper that reached its fixed ABI.
                    drop(channel);
                    let end = std::time::Instant::now() + Duration::from_secs(3);
                    while std::time::Instant::now() < end {
                        if matches!(child.try_wait(), Ok(Some(_))) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    retain_for_reaping(child);
                    return Err(error);
                }
            };
        let mut result = Self {
            child: Some(child),
            channel,
            peer,
            finished: false,
        };
        result
            .channel
            .set_read_timeout(Some(Duration::from_secs(3)))?;
        let mut ready = [0];
        result.channel.read_exact(&mut ready)?;
        if ready != [b'R'] {
            return Err(Error::Cgroup);
        }
        Ok(result)
    }
    pub fn verify_alive(&self) -> Result<()> {
        self.peer.verify()
    }
    pub fn disarm(mut self) -> Result<()> {
        // Called only after all creators acknowledged, TaskDelete acknowledged
        // and the exact native subtree confirmed empty. An already-fired helper may
        // have exited successfully; its exit never served as that task proof.
        let _ = self.channel.write_all(b"D");
        let end = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(status) = self.child.as_mut().ok_or(Error::Lifecycle)?.try_wait()? {
                self.finished = true;
                return if status.success() {
                    Ok(())
                } else {
                    Err(Error::Cgroup)
                };
            }
            if std::time::Instant::now() >= end {
                return Err(Error::Cgroup);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for DeadlineProcess {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let _ = self.channel.write_all(b"K");
        let end = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < end {
            match self.child.as_mut().expect("live fence child").try_wait() {
                Ok(Some(_)) => {
                    self.finished = true;
                    return;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(_) => break,
            }
        }
        // Never kill a delayed fence before it has enforced its boundary. Its
        // private channel closes and the original pidfd/deadline stay armed.
        // Uncertain completion retains the reservation, even after owner death.
        if let Some(child) = self.child.take() {
            retain_for_reaping(child);
        }
    }
}

// Initialized before spawning any helper. Even if the bounded Drop wait ends,
// the original Child stays owned until a process-wide reaper receives its exit.
// The reaper never signals a helper or releases any reservation.
type Children = Arc<std::sync::Mutex<Vec<Child>>>;
static REAPER: std::sync::OnceLock<std::result::Result<Children, ()>> = std::sync::OnceLock::new();
fn ensure_reaper() -> Result<&'static Children> {
    REAPER
        .get_or_init(|| {
            let children: Children = Arc::new(std::sync::Mutex::new(Vec::new()));
            let worker = children.clone();
            std::thread::Builder::new()
                .name("cowork-fence-reaper".into())
                .spawn(move || {
                    loop {
                        {
                            let mut children = worker.lock().unwrap_or_else(|p| p.into_inner());
                            children.retain_mut(|child| !matches!(child.try_wait(), Ok(Some(_))));
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                })
                .map_err(|_| ())?;
            Ok(children)
        })
        .as_ref()
        .map_err(|_| Error::Lifecycle)
}
fn retain_for_reaping(child: Child) {
    // Successfully initialized before every original child spawn.
    if let Some(Ok(children)) = REAPER.get() {
        children
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(child);
    }
}
pub(super) fn boottime() -> Result<u64> {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut value) } < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let sec: u64 = value.tv_sec.try_into().map_err(|_| Error::Cgroup)?;
    sec.checked_mul(1_000_000_000)
        .and_then(|v| v.checked_add(value.tv_nsec as u64))
        .ok_or(Error::Cgroup)
}

/// Standalone helper ABI: descriptors 3=exact cgroup directory, 4=original owner
/// pidfd, 5=private control socket. Arguments are only deadline/dev/inode integers.
/// No renderer/API exposes this command or descriptors.
pub fn entrypoint() -> i32 {
    let result = (|| -> Result<()> {
        if unsafe { libc::geteuid() } != 0 {
            return Err(Error::Cgroup);
        }
        let args: Vec<String> = std::env::args().collect();
        if args.len() != 4 {
            return Err(Error::Cgroup);
        }
        let deadline = args[1].parse::<u64>().map_err(|_| Error::Cgroup)?;
        let dev = args[2].parse::<u64>().map_err(|_| Error::Cgroup)?;
        let ino = args[3].parse::<u64>().map_err(|_| Error::Cgroup)?;
        let current = boottime()?;
        if deadline < current || deadline - current > 86_400_000_000_000 {
            return Err(Error::Cgroup);
        }
        let directory = unsafe { File::from_raw_fd(3) };
        let m = directory.metadata()?;
        let mut fs = std::mem::MaybeUninit::<libc::statfs>::zeroed();
        if unsafe { libc::fstatfs(directory.as_raw_fd(), fs.as_mut_ptr()) } < 0
            || unsafe { fs.assume_init() }.f_type != 0x63677270
            || m.dev() != dev
            || m.ino() != ino
            || m.uid() != 0
            || !m.is_dir()
            || m.mode() & 0o022 != 0
        {
            return Err(Error::Cgroup);
        }
        // A real pidfd must answer signal-0; no numeric PID is interpreted.
        let status = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                4,
                0,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        if status < 0 && io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
            return Err(Error::Cgroup);
        }
        let directory = cap_std::fs::Dir::from_std_file(directory);
        let mut write_options = cap_std::fs::OpenOptions::new();
        write_options.write(true);
        let mut freeze = directory
            .open_with("cgroup.freeze", &write_options)?
            .into_std();
        let mut kill = directory
            .open_with("cgroup.kill", &write_options)?
            .into_std();
        let mut channel = unsafe { StdUnixStream::from_raw_fd(5) };
        channel.write_all(b"R")?;
        loop {
            let clock = boottime()?;
            let timeout = ((deadline.saturating_sub(clock) / 1_000_000).min(50)) as i32;
            let mut polls = [
                libc::pollfd {
                    fd: 4,
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: channel.as_raw_fd(),
                    events: libc::POLLIN | libc::POLLHUP,
                    revents: 0,
                },
            ];
            let events = unsafe { libc::poll(polls.as_mut_ptr(), 2, timeout) };
            if events < 0 {
                if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(io::Error::last_os_error().into());
            }
            let owner_dead = polls[0].revents != 0;
            let clock = boottime()?;
            if !owner_dead && clock < deadline && polls[1].revents & libc::POLLIN != 0 {
                let mut command = [0];
                match channel.read(&mut command) {
                    Ok(1) if command == [b'D'] => {
                        channel.write_all(b"D")?;
                        return Ok(());
                    }
                    Ok(1) if command == [b'K'] => {}
                    _ => {}
                }
            } else if !owner_dead && clock < deadline && polls[1].revents == 0 {
                continue;
            }
            // Always freeze the ancestor before killing the current population.
            // It remains linked/frozen after this helper exits; a late creator's
            // sandbox leaf cannot thaw or escape it.
            freeze.write_all(b"1")?;
            kill.write_all(b"1")?;
            let _ = channel.write_all(b"F");
            return Ok(());
        }
    })();
    if result.is_ok() { 0 } else { 2 }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Only the bounded own-generation physical task fixture"]
    fn delayed_original_fence_is_reaped_after_drop() {
        use cap_std::{ambient_authority, fs::Dir};
        let parent = Dir::open_ambient_dir(
            std::env::var_os("COWORK_RUNTIME_CGROUP_PARENT").unwrap(),
            ambient_authority(),
        )
        .unwrap();
        let name = format!("reaper-{}", uuid::Uuid::new_v4().simple());
        parent.create_dir(&name).unwrap();
        let directory = parent.open_dir(&name).unwrap();
        let native = directory.open(".").unwrap().into_std().metadata().unwrap();
        let executable = FenceExecutable::open(
            &PathBuf::from(std::env::var_os("COWORK_TEST_FENCE_BINARY").unwrap()),
            &std::env::var("COWORK_TEST_FENCE_SHA256").unwrap(),
        )
        .unwrap();
        let helper = DeadlineProcess::start(
            &executable,
            &directory,
            std::time::Instant::now() + Duration::from_secs(20),
        )
        .unwrap();
        let pid = helper.child.as_ref().unwrap().id();
        let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        assert!(raw >= 0);
        let pidfd = unsafe { OwnedFd::from_raw_fd(raw as i32) };
        let signal = |value: libc::c_int| {
            assert_eq!(
                unsafe {
                    libc::syscall(
                        libc::SYS_pidfd_send_signal,
                        pidfd.as_raw_fd(),
                        value,
                        std::ptr::null::<libc::siginfo_t>(),
                        0,
                    )
                },
                0
            )
        };
        signal(libc::SIGSTOP);
        let status = PathBuf::from(format!("/proc/{pid}/status"));
        let limit = std::time::Instant::now() + Duration::from_secs(2);
        while !std::fs::read_to_string(&status)
            .unwrap()
            .lines()
            .any(|l| l.starts_with("State:\tT"))
        {
            assert!(std::time::Instant::now() < limit);
            std::thread::sleep(Duration::from_millis(5));
        }
        let began = std::time::Instant::now();
        drop(helper);
        assert!(began.elapsed() >= Duration::from_secs(3));
        assert!(
            ensure_reaper()
                .unwrap()
                .lock()
                .unwrap()
                .iter()
                .any(|c| c.id() == pid)
        );
        assert!(
            std::fs::read_to_string(&status)
                .unwrap()
                .lines()
                .any(|l| l.starts_with("State:\tT"))
        );
        signal(libc::SIGCONT);
        let until = std::time::Instant::now() + Duration::from_secs(3);
        while status.try_exists().unwrap() {
            assert!(
                std::time::Instant::now() < until,
                "original fence child must be reaped while runner remains alive"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !ensure_reaper()
                .unwrap()
                .lock()
                .unwrap()
                .iter()
                .any(|c| c.id() == pid)
        );
        assert_eq!(
            directory.read_to_string("cgroup.freeze").unwrap().trim(),
            "1"
        );
        let current = directory.open(".").unwrap().into_std().metadata().unwrap();
        assert_eq!((native.dev(), native.ino()), (current.dev(), current.ino()));
        parent.remove_dir(&name).unwrap();
        let observation = serde_json::json!({"original_fence_pid":pid,"runner_pid":std::process::id(),"paused_past_drop_window":true,"resumed_and_fenced":true,"original_child_reaped_while_runner_alive":true,"reservation_released":false});
        let base = PathBuf::from(std::env::var_os("COWORK_RUNTIME_VOLUME_PROOF_DIR").unwrap());
        std::fs::write(
            base.join("reaper-observation.json"),
            serde_json::to_vec(&observation).unwrap(),
        )
        .unwrap();
    }
}
