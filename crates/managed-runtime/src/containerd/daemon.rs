//! Private control process only. Daemon exit never proves that workload shims
//! stopped, and does not free workspace/control reservations or mint authority.
use super::*;
use std::{
    collections::BTreeMap,
    io::Write,
    os::fd::{AsRawFd, RawFd},
    sync::atomic::{AtomicBool, Ordering},
};

pub struct DaemonExecutable {
    pub(super) file: File,
    pub(super) sha256: String,
}
impl DaemonExecutable {
    /// Hash comes from trusted package/operator configuration, never this file.
    /// A pinned read-only descriptor is retained through the original spawn.
    pub fn open(path: &Path, expected_sha256: &str) -> Result<Self> {
        if !path.is_absolute() || !crate::valid_hash(expected_sha256) {
            return Err(Error::Peer);
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != 0
            || metadata.nlink() != 1
            || metadata.mode() & 0o222 != 0
            || metadata.mode() & 0o100 == 0
            || metadata.len() > 128 * 1024 * 1024
        {
            return Err(Error::Peer);
        }
        let result = Self {
            file,
            sha256: expected_sha256.into(),
        };
        result.verify()?;
        Ok(result)
    }
    pub(super) fn verify(&self) -> Result<()> {
        use std::os::unix::fs::FileExt;
        let metadata = self.file.metadata()?;
        if metadata.len() < 4
            || metadata.len() > 128 * 1024 * 1024
            || metadata.mode() & 0o222 != 0
            || metadata.uid() != 0
            || metadata.nlink() != 1
        {
            return Err(Error::Peer);
        }
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 65536];
        let mut offset = 0;
        loop {
            let count = self.file.read_at(&mut buffer, offset)?;
            if count == 0 {
                break;
            }
            if offset == 0 && (count < 4 || &buffer[..4] != b"\x7fELF") {
                return Err(Error::Peer);
            }
            offset += count as u64;
            if offset > 128 * 1024 * 1024 {
                return Err(Error::Peer);
            }
            hash.update(&buffer[..count]);
        }
        if offset != metadata.len() || format!("{:x}", hash.finalize()) != self.sha256 {
            return Err(Error::Peer);
        }
        Ok(())
    }
}

struct OwnedWorker {
    cancel: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<cowork_executor::Result<cowork_executor::Outcome>>>,
}
impl OwnedWorker {
    fn join(&mut self) -> Result<cowork_executor::Outcome> {
        self.cancel.store(true, Ordering::Release);
        self.thread
            .take()
            .ok_or(Error::Lifecycle)?
            .join()
            .map_err(|_| Error::Lifecycle)?
            .map_err(|error| Error::Startup {
                detail: format!("{error:?}"),
            })
    }
}
impl Drop for OwnedWorker {
    fn drop(&mut self) {
        if self.thread.is_some() {
            let _ = self.join();
        }
        // No directory/fence is removed here. A daemon's shims can outlive it.
    }
}

pub struct PrivateDaemon {
    connection: Option<ContainerdConnection>,
    worker: Option<OwnedWorker>,
    // The capability and exclusive lock stay pinned for the whole generation.
    _directory: File,
    _lock: File,
    peer_pid: u32,
}
pub struct DaemonExit {
    pub outcome: cowork_executor::Outcome,
    /// Only the owned daemon root was reaped. This is not a task stop witness.
    pub daemon_process_reaped: bool,
}
impl PrivateDaemon {
    /// Trusted bootstrap only: fresh root-owned 0700 directory on separately
    /// reserved control storage. Existing start fences are never adopted or
    /// retried, even after an apparently clean daemon exit. No public route.
    pub async fn start(
        executable: DaemonExecutable,
        directory: &Path,
        runner_binding_sha256: &str,
        lifetime: Duration,
    ) -> Result<Self> {
        Self::start_inner(
            executable,
            directory,
            runner_binding_sha256,
            lifetime,
            None,
            BTreeMap::new(),
        )
        .await
    }
    pub(super) async fn start_scoped(
        executable: DaemonExecutable,
        directory: &Path,
        runner_binding_sha256: &str,
        lifetime: Duration,
        placement: cowork_executor::CgroupPlacement,
        environment: BTreeMap<String, String>,
    ) -> Result<Self> {
        Self::start_inner(
            executable,
            directory,
            runner_binding_sha256,
            lifetime,
            Some(placement),
            environment,
        )
        .await
    }
    async fn start_inner(
        executable: DaemonExecutable,
        directory: &Path,
        runner_binding_sha256: &str,
        lifetime: Duration,
        placement: Option<cowork_executor::CgroupPlacement>,
        mut environment: BTreeMap<String, String>,
    ) -> Result<Self> {
        if unsafe { libc::geteuid() } != 0
            || !crate::valid_hash(runner_binding_sha256)
            || lifetime.is_zero()
            || lifetime > Duration::from_secs(86400)
            || !directory.is_absolute()
            || directory.components().any(|component| {
                !matches!(
                    component,
                    std::path::Component::RootDir | std::path::Component::Normal(_)
                )
            })
            || directory.as_os_str().as_encoded_bytes().len() > 4096
        {
            return Err(Error::Lifecycle);
        }
        for ancestor in directory.ancestors() {
            if std::fs::symlink_metadata(ancestor)?
                .file_type()
                .is_symlink()
            {
                return Err(Error::Socket);
            }
        }
        let anchor = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(directory)?;
        let metadata = anchor.metadata()?;
        if metadata.uid() != 0 || metadata.mode() & 0o077 != 0 {
            return Err(Error::Socket);
        }
        // Reject non-empty directories before creating our irreversible marker.
        // Caller owns this namespace; capability-relative paths avoid substitution.
        let capability = cap_std::fs::Dir::from_std_file(anchor.try_clone()?);
        if capability.entries()?.next().is_some() {
            return Err(Error::Lifecycle);
        }
        let mut options = cap_std::fs::OpenOptions::new();
        use cap_std::fs::OpenOptionsExt;
        options.read(true).write(true).create_new(true).mode(0o600);
        let lock = capability
            .open_with("daemon-start.fence", &options)?
            .into_std();
        lock.try_lock().map_err(|_| Error::Lifecycle)?;
        (&lock).write_all(
            format!(
                "cowork-private-containerd-v1\n{runner_binding_sha256}\n{}\n",
                executable.sha256
            )
            .as_bytes(),
        )?;
        lock.sync_all()?;
        anchor.sync_all()?;
        executable.verify()?;
        let fd_path = |fd: RawFd| PathBuf::from(format!("/proc/{}/fd/{fd}", std::process::id()));
        // Keep durable root/state paths stable even if shims outlive a daemon.
        // A /proc/parent/fd path could later refer to a reused descriptor. The
        // private owner-controlled directory stays pinned and checked instead.
        // A fixed private-namespace bind keeps Unix addresses short while all
        // daemon bytes remain in the independently reserved control filesystem.
        let launch_root = PathBuf::from("/run/cowork-daemon");
        let socket = directory.join("c.sock");
        let config = format!(
            "version = 3\ntemp = {}\nimports = [{}]\ndisabled_plugins = [\"io.containerd.cri.v1.images\", \"io.containerd.cri.v1.runtime\", \"io.containerd.grpc.v1.cri\", \"io.containerd.nri.v1.nri\", \"io.containerd.internal.v1.opt\", \"io.containerd.image-verifier.v1.bindir\", \"io.containerd.monitor.container.v1.restart\"]\n[ttrpc]\naddress = {}\n[metrics]\naddress = \"\"\n[debug]\nlevel = \"warn\"\n",
            // gVisor's sandbox bootstrap drops host credentials before its
            // chroot setup. Its temporary parent must be traversable in this
            // private /run, rather than below the daemon's root-only state dir.
            serde_json::to_string("/run/cowork-control/tmp").map_err(|_| Error::Lifecycle)?,
            serde_json::to_string(&launch_root.join("no-imports/*.toml"))
                .map_err(|_| Error::Lifecycle)?,
            serde_json::to_string(&launch_root.join("t.sock")).map_err(|_| Error::Lifecycle)?
        );
        let config_file = capability
            .open_with("containerd.toml", &options)?
            .into_std();
        (&config_file).write_all(config.as_bytes())?;
        config_file.sync_all()?;
        anchor.sync_all()?;
        environment.insert("PATH".into(), "/usr/sbin:/usr/bin:/sbin:/bin".into());
        let launch = cowork_executor::TrustedLaunch {
            program: fd_path(executable.file.as_raw_fd()),
            directory: launch_root.clone(),
            arguments: vec![
                "--config".into(),
                launch_root
                    .join("containerd.toml")
                    .to_string_lossy()
                    .into_owned(),
                "--address".into(),
                launch_root.join("c.sock").to_string_lossy().into_owned(),
                "--root".into(),
                launch_root.join("root").to_string_lossy().into_owned(),
                "--state".into(),
                launch_root.join("state").to_string_lossy().into_owned(),
            ],
            environment,
            timeout: lifetime,
            capture_bytes_per_stream: 65536,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let parent_namespace = std::fs::read_link("/proc/thread-self/ns/mnt")?;
        let namespace_directory = anchor.try_clone()?;
        let thread = std::thread::spawn(move || {
            // Only this new supervisor thread and its newly spawned children
            // see the private /run. No parent/global WSL mount is changed.
            isolate_control_mounts(&parent_namespace)
                .map_err(|_| cowork_executor::Error::Invalid("private control namespace failed"))?;
            bind_control_directory(&namespace_directory).map_err(cowork_executor::Error::Io)?;
            let on_started = |pid| {
                let peer =
                    ExpectedPeer::capture_new_child(pid, 0, &executable.sha256).map_err(|_| {
                        cowork_executor::Error::Invalid("private daemon binding failed")
                    })?;
                tx.send(peer).map_err(|_| {
                    cowork_executor::Error::Invalid("private daemon startup canceled")
                })?;
                Ok(())
            };
            let result = if let Some(placement) = placement {
                cowork_executor::supervise_started_in_cgroup(
                    &launch,
                    &worker_cancel,
                    &placement,
                    on_started,
                )
            } else {
                cowork_executor::supervise_started(&launch, &worker_cancel, on_started)
            };
            drop(executable); // Keep the exact ELF descriptor open through spawn.
            result
        });
        let mut worker = OwnedWorker {
            cancel,
            thread: Some(thread),
        };
        let peer = match tokio::time::timeout(Duration::from_secs(10), rx).await {
            Ok(Ok(peer)) => peer,
            Ok(Err(_)) => {
                let diagnostic = match worker.join() {
                    Ok(outcome) => String::from_utf8_lossy(&outcome.stderr.bytes)
                        .chars()
                        .take(4096)
                        .collect(),
                    Err(error) => error.to_string(),
                };
                return Err(Error::Startup { detail: diagnostic });
            }
            Err(_) => return Err(Error::Lifecycle),
        };
        let peer_pid = peer.pid;
        let connection = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if worker
                    .thread
                    .as_ref()
                    .is_none_or(|thread| thread.is_finished())
                {
                    let diagnostic = match worker.join() {
                        Ok(outcome) => String::from_utf8_lossy(&outcome.stderr.bytes)
                            .chars()
                            .take(4096)
                            .collect(),
                        Err(error) => error.to_string(),
                    };
                    return Err(Error::Startup { detail: diagnostic });
                }
                let current = std::fs::symlink_metadata(directory)?;
                if current.dev() != metadata.dev() || current.ino() != metadata.ino() {
                    return Err(Error::Socket);
                }
                match std::fs::symlink_metadata(&socket) {
                    Ok(m) if m.file_type().is_socket() => {
                        // Server chmod is not policy: enforce owner-only before
                        // we establish our actual peer/native endpoint binding.
                        if m.uid() != 0 {
                            return Err(Error::Socket);
                        }
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
                        let connection = ContainerdConnection::connect_owned_namespace(
                            peer.clone(),
                            runner_binding_sha256,
                            &anchor,
                        )
                        .await?;
                        connection.version().await?;
                        return Ok(connection);
                    }
                    Ok(_) => return Err(Error::Socket),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .map_err(|_| Error::Lifecycle)??;
        Ok(Self {
            connection: Some(connection),
            worker: Some(worker),
            _directory: anchor,
            _lock: lock,
            peer_pid,
        })
    }
    pub fn connection(&self) -> &ContainerdConnection {
        self.connection
            .as_ref()
            .expect("live private daemon connection")
    }
    /// Diagnostic fact only; never use a PID to adopt or terminate a daemon.
    pub fn process_id(&self) -> u32 {
        self.peer_pid
    }
    pub async fn stop(mut self) -> Result<DaemonExit> {
        self.connection.take();
        let mut worker = self.worker.take().ok_or(Error::Lifecycle)?;
        let outcome = tokio::task::spawn_blocking(move || worker.join())
            .await
            .map_err(|_| Error::Lifecycle)??;
        if outcome.process_id != Some(self.peer_pid) {
            return Err(Error::Lifecycle);
        }
        Ok(DaemonExit {
            outcome,
            daemon_process_reaped: true,
        })
    }
}

fn isolate_control_mounts(parent_namespace: &Path) -> io::Result<()> {
    if unsafe { libc::unshare(libc::CLONE_NEWNS) } < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe {
        libc::mount(
            std::ptr::null(),
            c"/".as_ptr(),
            std::ptr::null(),
            libc::MS_REC | libc::MS_PRIVATE,
            std::ptr::null(),
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    if std::fs::read_link("/proc/thread-self/ns/mnt")? == parent_namespace {
        return Err(io::Error::other("control namespace unchanged"));
    }
    if unsafe {
        libc::mount(
            c"tmpfs".as_ptr(),
            c"/run".as_ptr(),
            c"tmpfs".as_ptr(),
            libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
            c"mode=0755,size=8m,nr_inodes=128".as_ptr().cast(),
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    std::fs::create_dir("/run/cowork-control")?;
    std::fs::create_dir("/run/cowork-control/tmp")?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        "/run/cowork-control",
        std::fs::Permissions::from_mode(0o755),
    )?;
    std::fs::set_permissions(
        "/run/cowork-control/tmp",
        std::fs::Permissions::from_mode(0o1777),
    )?;
    Ok(())
}
fn bind_control_directory(directory: &File) -> io::Result<()> {
    use std::os::fd::FromRawFd;
    std::fs::create_dir("/run/cowork-daemon")?;
    let expected = directory.metadata()?;
    // FDs retained across unshare still refer to the previous mount namespace.
    // Reopen here and require the same local native directory before cloning.
    let path = std::fs::read_link(format!("/proc/self/fd/{}", directory.as_raw_fd()))?;
    if !path.is_absolute() || path.as_os_str().as_encoded_bytes().ends_with(b" (deleted)") {
        return Err(io::Error::other("control source detached"));
    }
    let current = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let m = current.metadata()?;
    if expected.dev() != m.dev() || expected.ino() != m.ino() {
        return Err(io::Error::other("control source changed in namespace"));
    }
    let tree = unsafe {
        libc::syscall(
            libc::SYS_open_tree,
            current.as_raw_fd(),
            c"".as_ptr(),
            1u32 | libc::O_CLOEXEC as u32 | libc::AT_EMPTY_PATH as u32,
        )
    };
    if tree < 0 {
        return Err(io::Error::last_os_error());
    }
    let tree = unsafe { File::from_raw_fd(tree as i32) };
    // MOVE_MOUNT_F_EMPTY_PATH: fixed owned target, exact detached source FD.
    if unsafe {
        libc::syscall(
            libc::SYS_move_mount,
            tree.as_raw_fd(),
            c"".as_ptr(),
            libc::AT_FDCWD,
            c"/run/cowork-daemon".as_ptr(),
            4u32,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    let actual = std::fs::metadata("/run/cowork-daemon")?;
    if expected.dev() != actual.dev() || expected.ino() != actual.ino() {
        return Err(io::Error::other("private control bind changed"));
    }
    Ok(())
}
