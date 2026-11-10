//! Trusted shim adapter. A shim's cached sentinel status is never guest-exit
//! evidence. Only the original pinned runsc child's complete successful wait
//! response, durably bound to this fresh task/dispatch, can confirm that status.
use super::*;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Write,
    os::{fd::AsRawFd, unix::process::CommandExt},
};

const CONTEXT_ENV: &str = "COWORK_RUNSC_CONTEXT";
const SLOT_BYTES: usize = 16384;

pub struct RunscDriverExecutable {
    executable: DaemonExecutable,
    path: PathBuf,
    stamp: gvisor::Stamp,
}
impl RunscDriverExecutable {
    pub fn open(path: &Path, sha256: &str) -> Result<Self> {
        gvisor::require_owned_ancestors(path.parent().ok_or(Error::GuestExit)?)?;
        let executable = DaemonExecutable::open(path, sha256)?;
        let stamp = gvisor::Stamp::of(&executable.file.metadata()?);
        executable.verify()?;
        if gvisor::Stamp::of(&executable.file.metadata()?) != stamp {
            return Err(Error::GuestExit);
        }
        Ok(Self {
            executable,
            path: path.into(),
            stamp,
        })
    }
    pub(super) fn verify(&self) -> Result<()> {
        gvisor::require_owned_ancestors(self.path.parent().ok_or(Error::GuestExit)?)?;
        let current = std::fs::symlink_metadata(&self.path)?;
        let original = self.executable.file.metadata()?;
        if !current.is_file()
            || gvisor::Stamp::of(&current) != self.stamp
            || gvisor::Stamp::of(&original) != self.stamp
        {
            return Err(Error::GuestExit);
        }
        Ok(())
    }
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Context {
    format_version: u32,
    request_sha256: String,
    nonce: String,
    real_runsc: PathBuf,
    real_runsc_sha256: String,
    driver_sha256: String,
    driver_stamp: gvisor::Stamp,
    runsc_stamp: gvisor::Stamp,
    workload_id: String,
    sandbox_id: String,
    directory_dev: u64,
    directory_ino: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Evidence {
    format_version: u32,
    request_sha256: String,
    nonce: String,
    container_id: String,
    real_runsc_sha256: String,
    driver_sha256: String,
    child: cowork_executor::Outcome,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitResponse {
    id: String,
    #[serde(rename = "exitStatus")]
    exit_status: u32,
}
/// Not constructible from a renderer value, cached shim status, or bare boolean.
pub(super) struct VerifiedGuestExit {
    status: u32,
    sha256: String,
}
impl VerifiedGuestExit {
    pub fn status(&self) -> u32 {
        self.status
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}
pub(super) struct WaitEvidence {
    directory: Dir,
    context: Context,
    path: PathBuf,
}
impl WaitEvidence {
    pub fn prepare(
        control: &Dir,
        driver: &RunscDriverExecutable,
        tools: &gvisor::GvisorTools,
        request_sha256: &str,
        workload_id: &str,
        sandbox_id: &str,
    ) -> Result<Self> {
        driver.verify()?;
        let (real_runsc, real_runsc_sha256, runsc_stamp) = tools.runsc()?;
        control.create_dir("wait-evidence")?;
        control.set_permissions(
            "wait-evidence",
            cap_std::fs::Permissions::from_std(std::os::unix::fs::PermissionsExt::from_mode(0o700)),
        )?;
        let directory = control.open_dir_nofollow("wait-evidence")?;
        let metadata = directory.open(".")?.into_std().metadata()?;
        let path = super::tasks::native_path(&directory)?;
        let context = Context {
            format_version: 1,
            request_sha256: request_sha256.into(),
            nonce: uuid::Uuid::new_v4().simple().to_string(),
            real_runsc,
            real_runsc_sha256,
            driver_sha256: driver.executable.sha256.clone(),
            driver_stamp: driver.stamp,
            runsc_stamp,
            workload_id: workload_id.into(),
            sandbox_id: sandbox_id.into(),
            directory_dev: metadata.dev(),
            directory_ino: metadata.ino(),
        };
        for id in [workload_id, sandbox_id] {
            let file = new_file(&directory, &format!("{id}.wait"))?;
            file.set_len(SLOT_BYTES as u64)?;
            if unsafe { libc::posix_fallocate(file.as_raw_fd(), 0, SLOT_BYTES as i64) } != 0 {
                return Err(Error::GuestExit);
            }
            file.sync_all()?;
        }
        let mut file = new_file(&directory, "context.json")?;
        file.write_all(&serde_json::to_vec(&context).map_err(|_| Error::GuestExit)?)?;
        file.sync_all()?;
        directory.open(".")?.into_std().sync_all()?;
        Ok(Self {
            directory,
            context,
            path,
        })
    }
    pub fn environment(&self) -> BTreeMap<String, String> {
        BTreeMap::from([(
            CONTEXT_ENV.into(),
            self.path
                .join("context.json")
                .to_string_lossy()
                .into_owned(),
        )])
    }
    pub fn verified_workload_exit(&self) -> Result<VerifiedGuestExit> {
        let bytes = read_slot(&self.directory, &self.context.workload_id)?;
        let evidence: Evidence = serde_json::from_slice(&bytes).map_err(|_| Error::GuestExit)?;
        if evidence.format_version != 1
            || evidence.request_sha256 != self.context.request_sha256
            || evidence.nonce != self.context.nonce
            || evidence.container_id != self.context.workload_id
            || evidence.real_runsc_sha256 != self.context.real_runsc_sha256
            || evidence.driver_sha256 != self.context.driver_sha256
        {
            return Err(Error::GuestExit);
        }
        let response = genuine_wait(&evidence.child, &self.context.workload_id)?;
        Ok(VerifiedGuestExit {
            status: response.exit_status,
            sha256: format!("{:x}", Sha256::digest(bytes)),
        })
    }
}
fn new_file(directory: &Dir, name: &str) -> Result<File> {
    use cap_std::fs::OpenOptionsExt;
    let mut options = cap_std::fs::OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .follow(FollowSymlinks::No);
    Ok(directory.open_with(name, &options)?.into_std())
}
fn genuine_wait(child: &cowork_executor::Outcome, id: &str) -> Result<WaitResponse> {
    if child.termination != cowork_executor::Termination::Exited
        || child.exit_code != Some(0)
        || child.signal.is_some()
        || child.process_id.is_none()
        || !child.output_complete
        || child.stdout.truncated
        || child.stderr.truncated
        || child.stdout.bytes.len() > 4096
        || child.stdout.total_bytes != child.stdout.bytes.len() as u64
    {
        return Err(Error::GuestExit);
    }
    let response: WaitResponse =
        serde_json::from_slice(&child.stdout.bytes).map_err(|_| Error::GuestExit)?;
    if response.id != id || response.exit_status > 255 {
        return Err(Error::GuestExit);
    }
    Ok(response)
}
fn read_slot(directory: &Dir, id: &str) -> Result<Vec<u8>> {
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let mut file = directory
        .open_with(format!("{id}.wait"), &options)?
        .into_std();
    let m = file.metadata()?;
    if !m.is_file()
        || m.uid() != 0
        || m.mode() & 0o077 != 0
        || m.nlink() != 1
        || m.len() != SLOT_BYTES as u64
    {
        return Err(Error::GuestExit);
    }
    let mut slot = vec![0; SLOT_BYTES];
    file.read_exact(&mut slot)?;
    let len = u32::from_le_bytes(slot[..4].try_into().map_err(|_| Error::GuestExit)?) as usize;
    if len == 0
        || len > SLOT_BYTES - 36
        || Sha256::digest(&slot[36..36 + len]).as_slice() != &slot[4..36]
    {
        return Err(Error::GuestExit);
    }
    file.sync_all()?;
    Ok(slot[36..36 + len].to_vec())
}

/// Private runtime-service executable ABI, never an application command API.
pub fn entrypoint() -> i32 {
    drive().unwrap_or(1)
}
fn drive() -> Result<i32> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(Error::GuestExit);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() > 1024 || args.iter().map(String::len).sum::<usize>() > 32768 {
        return Err(Error::GuestExit);
    }
    let context_path = PathBuf::from(std::env::var_os(CONTEXT_ENV).ok_or(Error::GuestExit)?);
    if !context_path.is_absolute()
        || context_path.file_name().and_then(|v| v.to_str()) != Some("context.json")
    {
        return Err(Error::GuestExit);
    }
    let directory = Dir::open_ambient_dir(
        context_path.parent().ok_or(Error::GuestExit)?,
        cap_std::ambient_authority(),
    )?;
    let m = directory.open(".")?.into_std().metadata()?;
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let file = directory.open_with("context.json", &options)?.into_std();
    let fm = file.metadata()?;
    if !m.is_dir()
        || m.uid() != 0
        || m.mode() & 0o077 != 0
        || !fm.is_file()
        || fm.uid() != 0
        || fm.mode() & 0o077 != 0
        || fm.nlink() != 1
        || fm.len() > 8192
    {
        return Err(Error::GuestExit);
    }
    let context: Context = serde_json::from_reader(file).map_err(|_| Error::GuestExit)?;
    if context.format_version != 1
        || context.directory_dev != m.dev()
        || context.directory_ino != m.ino()
        || !crate::valid_hash(&context.request_sha256)
        || context.nonce.len() != 32
        || !valid_id(&context.workload_id, "cw-")
        || !valid_id(&context.sandbox_id, "sb-")
    {
        return Err(Error::GuestExit);
    }
    // Hashes were checked before creating this fresh protected context. Native
    // identities stay local; changed bytes/mtime/ctime/mode/links reject reuse.
    let _driver = pinned_file(&std::fs::read_link("/proc/self/exe")?, context.driver_stamp)?;
    let executable = pinned_file(&context.real_runsc, context.runsc_stamp)?;
    let program = PathBuf::from(format!(
        "/proc/{}/fd/{}",
        std::process::id(),
        executable.as_raw_fd()
    ));
    let environment: BTreeMap<String, String> =
        std::env::vars().filter(|(k, _)| k != CONTEXT_ENV).collect();
    // gVisor builds all global flags as --key=value. Recognize only its exact
    // final wait command, without PID/checkpoint options or ambiguous tokens.
    let wait = args.len() >= 2
        && args[args.len() - 2] == "wait"
        && args[..args.len() - 2].iter().all(|v| v.starts_with("--"));
    if !wait {
        return Err(std::process::Command::new(program)
            .args(&args)
            .env_clear()
            .envs(environment)
            .exec()
            .into());
    }
    let id = args.last().ok_or(Error::GuestExit)?.clone();
    if id != context.workload_id && id != context.sandbox_id {
        return Err(Error::GuestExit);
    }
    // A fresh preallocated slot is single-use; never overwrite old evidence.
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true).write(true).follow(FollowSymlinks::No);
    let mut slot = directory
        .open_with(format!("{id}.wait"), &options)?
        .into_std();
    slot.try_lock().map_err(|_| Error::GuestExit)?;
    let sm = slot.metadata()?;
    let mut initial = vec![0; SLOT_BYTES];
    slot.read_exact(&mut initial)?;
    if sm.len() != SLOT_BYTES as u64
        || sm.uid() != 0
        || sm.mode() & 0o077 != 0
        || sm.nlink() != 1
        || initial.iter().any(|b| *b != 0)
    {
        return Err(Error::GuestExit);
    }
    let launch = cowork_executor::TrustedLaunch {
        program,
        arguments: args,
        directory: std::env::current_dir()?,
        environment,
        timeout: Duration::from_secs(86400),
        capture_bytes_per_stream: 4096,
    };
    // The exact retained ELF descriptor is exec'd, without a pathname reopen.
    // Supervisor owns/reaps this new child; fast successful waits need no
    // racy /proc inspection after the child has already exited.
    let child = cowork_executor::supervise(&launch, &std::sync::atomic::AtomicBool::new(false))
        .map_err(|_| Error::GuestExit)?;
    // Failed CLI waits are also stored, but can never become positive evidence.
    let code = if genuine_wait(&child, &id).is_ok() {
        0
    } else {
        1
    };
    let evidence = Evidence {
        format_version: 1,
        request_sha256: context.request_sha256,
        nonce: context.nonce,
        container_id: id.clone(),
        real_runsc_sha256: context.real_runsc_sha256,
        driver_sha256: context.driver_sha256,
        child,
    };
    let bytes = serde_json::to_vec(&evidence).map_err(|_| Error::GuestExit)?;
    if bytes.len() > SLOT_BYTES - 36 {
        return Err(Error::GuestExit);
    }
    let mut buffer = vec![0; SLOT_BYTES];
    buffer[..4].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
    buffer[4..36].copy_from_slice(&Sha256::digest(&bytes));
    buffer[36..36 + bytes.len()].copy_from_slice(&bytes);
    use std::io::{Seek, SeekFrom};
    slot.seek(SeekFrom::Start(0))?;
    slot.write_all(&buffer)?;
    slot.sync_all()?;
    std::io::stdout().write_all(&evidence.child.stdout.bytes)?;
    std::io::stderr().write_all(&evidence.child.stderr.bytes)?;
    Ok(code)
}
fn valid_id(id: &str, prefix: &str) -> bool {
    id.strip_prefix(prefix)
        .is_some_and(|v| v.len() == 32 && v.bytes().all(|b| b.is_ascii_hexdigit()))
}
fn pinned_file(path: &Path, stamp: gvisor::Stamp) -> Result<File> {
    if !path.is_absolute() {
        return Err(Error::GuestExit);
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
        || gvisor::Stamp::of(&metadata) != stamp
    {
        return Err(Error::GuestExit);
    }
    Ok(file)
}
