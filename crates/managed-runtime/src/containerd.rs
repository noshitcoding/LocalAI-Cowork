//! Private containerd transport. This verifies the actual local endpoint, not
//! execution policy or physical workload isolation. No public dispatch route.
pub mod backend;
pub mod bootstrap;
pub mod cgroups;
mod daemon;
pub mod deadline_process;
pub mod gvisor;
pub mod input;
pub mod output;
pub mod profile;
pub mod receive;
pub mod result_snapshot;
pub mod retirement;
pub mod rootfs;
pub mod runsc_driver;
mod task_journal;
pub mod tasks;
pub use daemon::{DaemonExecutable, DaemonExit, PrivateDaemon};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Read},
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::net::UnixStream;
use tonic::{
    Request,
    transport::{Channel, Endpoint},
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("runtime_containerd_peer_changed")]
    Peer,
    #[error("runtime_containerd_socket_changed")]
    Socket,
    #[error("runtime_containerd_transport_failed")]
    Transport,
    #[error("runtime_containerd_rpc_failed")]
    Rpc,
    #[error("runtime_task_rpc_failed: {operation}: {code}: {detail}")]
    TaskRpc {
        operation: &'static str,
        code: String,
        detail: String,
    },
    #[error("runtime_containerd_io_failed")]
    Io(#[from] io::Error),
    #[error("runtime_containerd_lifecycle_uncertain")]
    Lifecycle,
    #[error("runtime_containerd_startup_failed: {detail}")]
    Startup { detail: String },
    #[error("runtime_cgroup_controls_unavailable_or_changed")]
    Cgroup,
    #[error("runtime_task_boundary_not_ready: {reason}")]
    TaskBoundary { reason: &'static str },
    #[error("runtime_output_capture_unavailable_or_changed")]
    Output,
    #[error("runtime_result_export_rejected: {reason}")]
    ResultExport { reason: &'static str },
    #[error("runtime_gvisor_toolset_unavailable_or_changed")]
    Tools,
    #[error("runtime_package_rootfs_unavailable_or_changed")]
    Rootfs,
    #[error("runtime_confirmed_input_grant_invalid_or_changed")]
    Input,
    #[error("runtime_confirmed_input_rejected_at_{stage}: {detail}")]
    InputStage { stage: &'static str, detail: String },
    #[error("runtime_physical_task_journal_failed")]
    TaskJournal,
    #[error("runtime_guest_exit_status_unproven")]
    GuestExit,
    #[error("runtime_task_admission_invalid")]
    TaskAdmission,
    #[error("runtime_managed_receiver_busy")]
    ManagedBusy,
    #[error("runtime_managed_receiver_retains_unresolved_effect")]
    ManagedUncertain,
    #[error("runtime_retirement_{stage}: {detail}")]
    Retirement { stage: &'static str, detail: String },
}
pub type Result<T> = std::result::Result<T, Error>;

/// Comes only from the supervisor's *newly spawned* child callback. A PID from
/// a renderer, stale pidfile, discovery listing or request is not this authority.
#[derive(Clone)]
pub struct ExpectedPeer {
    pid: u32,
    uid: u32,
    start_ticks: u64,
    executable_sha256: String,
    executable: Arc<File>,
    executable_stamp: gvisor::Stamp,
}
impl ExpectedPeer {
    pub fn capture_new_child(pid: u32, uid: u32, executable_sha256: &str) -> Result<Self> {
        if pid == 0 || pid > i32::MAX as u32 || !crate::valid_hash(executable_sha256) {
            return Err(Error::Peer);
        }
        let executable = File::open(format!("/proc/{pid}/exe"))?;
        let metadata = executable.metadata()?;
        // A writable host binary cannot use a cached identity. Installed control
        // and fence ELFs are root-owned, single-linked and read-only; hash once,
        // retain the original file and reject every subsequent stamp change.
        if !metadata.is_file()
            || metadata.uid() != 0
            || metadata.nlink() != 1
            || metadata.mode() & 0o222 != 0
            || metadata.mode() & 0o100 == 0
            || metadata.len() > 128 * 1024 * 1024
        {
            return Err(Error::Peer);
        }
        let executable_stamp = gvisor::Stamp::of(&metadata);
        let mut hash = Sha256::new();
        io::copy(&mut (&executable).take(128 * 1024 * 1024 + 1), &mut hash)?;
        if format!("{:x}", hash.finalize()) != executable_sha256
            || gvisor::Stamp::of(&executable.metadata()?) != executable_stamp
        {
            return Err(Error::Peer);
        }
        let expected = Self {
            pid,
            uid,
            start_ticks: start_ticks(pid)?,
            executable_sha256: executable_sha256.into(),
            executable: Arc::new(executable),
            executable_stamp,
        };
        expected.verify()?;
        Ok(expected)
    }
    fn verify(&self) -> Result<()> {
        if start_ticks(self.pid)? != self.start_ticks
            || std::fs::metadata(format!("/proc/{}", self.pid))?.uid() != self.uid
        {
            return Err(Error::Peer);
        }
        let file = File::open(format!("/proc/{}/exe", self.pid))?;
        if gvisor::Stamp::of(&file.metadata()?) != self.executable_stamp
            || gvisor::Stamp::of(&self.executable.metadata()?) != self.executable_stamp
            || start_ticks(self.pid)? != self.start_ticks
        {
            return Err(Error::Peer);
        }
        Ok(())
    }
}

struct SocketBinding {
    path: PathBuf,
    dev: u64,
    ino: u64,
    uid: u32,
    parent_dev: u64,
    parent_ino: u64,
    peer: ExpectedPeer,
}
impl SocketBinding {
    fn capture(path: &Path, peer: ExpectedPeer) -> Result<Self> {
        if !path.is_absolute() {
            return Err(Error::Socket);
        }
        for parent in path.ancestors().skip(1) {
            if std::fs::symlink_metadata(parent)?.file_type().is_symlink() {
                return Err(Error::Socket);
            }
        }
        Self::capture_native(path, peer, None)
    }
    /// Only an original owned daemon may use this fixed kernel-procfs route.
    /// Its native parent must be the exact directory pinned before spawn.
    /// The generic path connector above keeps its symlink rejection unchanged.
    fn capture_namespace(peer: ExpectedPeer, directory: &File) -> Result<Self> {
        peer.verify()?;
        let path = PathBuf::from(format!("/proc/{}/root/run/cowork-daemon/c.sock", peer.pid));
        let m = directory.metadata()?;
        Self::capture_native(&path, peer, Some((m.dev(), m.ino())))
    }
    fn capture_native(
        path: &Path,
        peer: ExpectedPeer,
        expected_parent: Option<(u64, u64)>,
    ) -> Result<Self> {
        let m = std::fs::symlink_metadata(path)?;
        if !m.file_type().is_socket() || m.uid() != peer.uid || m.mode() & 0o077 != 0 {
            return Err(Error::Socket);
        }
        let parent = std::fs::symlink_metadata(path.parent().ok_or(Error::Socket)?)?;
        if !parent.is_dir() || parent.uid() != peer.uid || parent.mode() & 0o077 != 0 {
            return Err(Error::Socket);
        }
        if expected_parent.is_some_and(|id| id != (parent.dev(), parent.ino())) {
            return Err(Error::Socket);
        }
        peer.verify()?;
        Ok(Self {
            path: path.into(),
            dev: m.dev(),
            ino: m.ino(),
            uid: m.uid(),
            parent_dev: parent.dev(),
            parent_ino: parent.ino(),
            peer,
        })
    }
    fn verify(&self) -> Result<()> {
        let m = std::fs::symlink_metadata(&self.path)?;
        if !m.file_type().is_socket()
            || m.dev() != self.dev
            || m.ino() != self.ino
            || m.uid() != self.uid
            || m.mode() & 0o077 != 0
        {
            return Err(Error::Socket);
        }
        let parent = std::fs::symlink_metadata(self.path.parent().ok_or(Error::Socket)?)?;
        if !parent.is_dir()
            || parent.dev() != self.parent_dev
            || parent.ino() != self.parent_ino
            || parent.uid() != self.uid
            || parent.mode() & 0o077 != 0
        {
            return Err(Error::Socket);
        }
        self.peer.verify()
    }
    async fn connect(&self) -> io::Result<hyper_util::rt::TokioIo<UnixStream>> {
        self.verify()
            .map_err(|_| io::Error::other("private runtime binding changed"))?;
        let stream = UnixStream::connect(&self.path).await?;
        let actual = stream.peer_cred()?;
        if actual.pid() != Some(self.peer.pid as i32) || actual.uid() != self.peer.uid {
            return Err(io::Error::other("private runtime peer changed"));
        }
        self.verify()
            .map_err(|_| io::Error::other("private runtime binding changed"))?;
        Ok(hyper_util::rt::TokioIo::new(stream))
    }
}

pub struct ContainerdConnection {
    channel: Channel,
    binding: Arc<SocketBinding>,
    namespace: String,
}
#[derive(Debug, Serialize)]
pub struct DaemonVersion {
    pub version: String,
    pub revision: String,
}
impl ContainerdConnection {
    /// The namespace is derived from the trusted runner binding hash and is
    /// never selected by an untrusted workload. Namespaces alone are not auth.
    pub async fn connect(
        path: &Path,
        peer: ExpectedPeer,
        runner_binding_sha256: &str,
    ) -> Result<Self> {
        if !crate::valid_hash(runner_binding_sha256) {
            return Err(Error::Peer);
        }
        let binding = Arc::new(SocketBinding::capture(path, peer)?);
        Self::connect_binding(binding, runner_binding_sha256).await
    }
    pub(super) async fn connect_owned_namespace(
        peer: ExpectedPeer,
        runner_binding_sha256: &str,
        directory: &File,
    ) -> Result<Self> {
        if !crate::valid_hash(runner_binding_sha256) {
            return Err(Error::Peer);
        }
        let binding = Arc::new(SocketBinding::capture_namespace(peer, directory)?);
        Self::connect_binding(binding, runner_binding_sha256).await
    }
    async fn connect_binding(
        binding: Arc<SocketBinding>,
        runner_binding_sha256: &str,
    ) -> Result<Self> {
        let connector = binding.clone();
        // The connector never dials this dummy URI. Every connect/reconnect is
        // the same private Unix socket and repeats native/PID/executable checks.
        let channel = Endpoint::from_static("http://[::1]:1")
            .connect_timeout(Duration::from_secs(5))
            .connect_with_connector(tower::service_fn(move |_| {
                let binding = connector.clone();
                async move { binding.connect().await }
            }))
            .await
            .map_err(|_| Error::Transport)?;
        Ok(Self {
            channel,
            binding,
            namespace: format!("cowork-{}", &runner_binding_sha256[..32]),
        })
    }
    pub fn namespace(&self) -> &str {
        &self.namespace
    }
    pub async fn version(&self) -> Result<DaemonVersion> {
        self.binding.verify()?;
        let mut request = Request::new(());
        request.set_timeout(Duration::from_secs(5));
        let result = containerd_client::services::v1::version_client::VersionClient::new(
            self.channel.clone(),
        )
        .version(request)
        .await
        .map_err(|_| Error::Rpc)?
        .into_inner();
        self.binding.verify()?;
        Ok(DaemonVersion {
            version: result.version,
            revision: result.revision,
        })
    }
}

fn start_ticks(pid: u32) -> Result<u64> {
    let path = format!("/proc/{pid}/stat");
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let mut text = String::new();
    Read::by_ref(&mut file)
        .take(8193)
        .read_to_string(&mut text)?;
    if text.len() > 8192 {
        return Err(Error::Peer);
    }
    // The parenthesized comm field may contain spaces or ')'. Remaining fields
    // begin with field 3; Linux starttime is field 22 (index 19 in this suffix).
    text.rsplit_once(") ")
        .and_then(|(_, rest)| rest.split_ascii_whitespace().nth(19))
        .and_then(|v| v.parse().ok())
        .ok_or(Error::Peer)
}
