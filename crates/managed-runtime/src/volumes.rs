//! Reserved ext4 storage for the app-owned Linux/WSL runtime. This low-level
//! component has no project/plan authority; the existing runner admission must
//! authorize an intent before reserving it. Native IDs never leave this journal.
mod loop_device;
#[cfg(feature = "containerd")]
mod published_retirement;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions, Permissions, PermissionsExt};
use loop_device::{LoopDevice, LoopIdentity};
#[cfg(feature = "containerd")]
pub(crate) use published_retirement::{RetiredPublishedVolume, RetiringPublishedVolume};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::CString,
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, atomic::AtomicBool},
    time::Duration,
};

#[derive(Debug, thiserror::Error)]
pub enum VolumeError {
    #[error("runtime_volume_identity_changed")]
    Identity,
    #[error("runtime_volume_limits_invalid")]
    Limits,
    #[error("runtime_volume_pool_full")]
    Capacity,
    #[error("runtime_volume_pool_busy")]
    Busy,
    #[error("runtime_volume_recovery_required")]
    Uncertain,
    #[error("runtime_volume_device_still_attached")]
    StillAttached,
    #[error("runtime_volume_formatter_failed")]
    Formatter,
    #[error("runtime_volume_io_failed")]
    Io(#[from] io::Error),
    #[error("runtime_volume_journal_invalid")]
    Encoding(#[from] serde_json::Error),
}
pub type Result<T> = std::result::Result<T, VolumeError>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PoolLimits {
    pub reserved_bytes: u64,
    pub max_volumes: u16,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VolumeRequest {
    pub request_sha256: String,
    pub bytes: u64,
    pub inodes: u32,
}
impl VolumeRequest {
    fn validate(&self) -> Result<()> {
        if !super::valid_hash(&self.request_sha256)
            || !(16 * 1024 * 1024..=8 * 1024 * 1024 * 1024).contains(&self.bytes)
            || !self.bytes.is_multiple_of(1024 * 1024)
            || !(128..=32768).contains(&self.inodes)
            || !self.inodes.is_power_of_two()
        {
            return Err(VolumeError::Limits);
        }
        Ok(())
    }
}

/// Operator-pinned formatter. A live root-owned executable descriptor is used
/// through /proc, preventing path substitution between verification and exec.
pub struct PinnedFormatter {
    file: File,
    sha256: String,
}
impl PinnedFormatter {
    pub fn open(path: &Path, expected_sha256: &str) -> Result<Self> {
        if !path.is_absolute() || !super::valid_hash(expected_sha256) {
            return Err(VolumeError::Formatter);
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        let m = file.metadata()?;
        if !m.is_file()
            || m.uid() != 0
            || m.mode() & 0o022 != 0
            || m.nlink() != 1
            || m.len() > 32 * 1024 * 1024
        {
            return Err(VolumeError::Formatter);
        }
        let mut hasher = Sha256::new();
        io::copy(&mut (&file).take(32 * 1024 * 1024 + 1), &mut hasher)?;
        if format!("{:x}", hasher.finalize()) != expected_sha256 {
            return Err(VolumeError::Formatter);
        }
        Ok(Self {
            file,
            sha256: expected_sha256.into(),
        })
    }
    fn format(&self, image: &File, dir: &Dir, request: &VolumeRequest, uuid: &str) -> Result<()> {
        use std::os::unix::fs::FileExt;
        let before = self.file.metadata()?;
        if before.uid() != 0
            || before.mode() & 0o022 != 0
            || before.nlink() != 1
            || before.len() > 32 * 1024 * 1024
        {
            return Err(VolumeError::Formatter);
        }
        let mut hasher = Sha256::new();
        let mut offset = 0;
        let mut buffer = [0u8; 65536];
        while offset < before.len() {
            let n = self.file.read_at(&mut buffer, offset)?;
            if n == 0 {
                return Err(VolumeError::Formatter);
            }
            offset += n as u64;
            hasher.update(&buffer[..n]);
        }
        let after = self.file.metadata()?;
        if before.len() != after.len()
            || before.modified()? != after.modified()?
            || format!("{:x}", hasher.finalize()) != self.sha256
        {
            return Err(VolumeError::Formatter);
        }
        let launch = cowork_executor::TrustedLaunch {
            program: fd_path(self.file.as_raw_fd()),
            directory: fd_path(dir.as_raw_fd()),
            arguments: vec![
                "-q".into(),
                "-F".into(),
                "-t".into(),
                "ext4".into(),
                "-m".into(),
                "0".into(),
                "-N".into(),
                request.inodes.to_string(),
                "-E".into(),
                "nodiscard,lazy_itable_init=0,lazy_journal_init=0".into(),
                "-U".into(),
                uuid.into(),
                fd_path(image.as_raw_fd()).to_string_lossy().into_owned(),
            ],
            environment: BTreeMap::from([("PATH".into(), "/usr/sbin:/usr/bin:/sbin:/bin".into())]),
            timeout: Duration::from_secs(30),
            capture_bytes_per_stream: 65536,
        };
        let result = cowork_executor::supervise(&launch, &AtomicBool::new(false))
            .map_err(|_| VolumeError::Formatter)?;
        if result.termination != cowork_executor::Termination::Exited
            || result.exit_code != Some(0)
            || !result.output_complete
        {
            return Err(VolumeError::Formatter);
        }
        check_allocated(image, request.bytes)?;
        image.sync_all()?;
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct NativeId {
    dev: u64,
    ino: u64,
}
impl NativeId {
    fn file(file: &File) -> Result<Self> {
        let m = file.metadata()?;
        if !m.is_file() || m.nlink() != 1 || m.uid() != 0 || m.mode() & 0o077 != 0 {
            return Err(VolumeError::Identity);
        }
        Ok(Self {
            dev: m.dev(),
            ino: m.ino(),
        })
    }
    fn directory(dir: &Dir) -> Result<Self> {
        let m = dir.try_clone()?.into_std_file().metadata()?;
        if !m.is_dir() || m.uid() != 0 || m.mode() & 0o077 != 0 {
            return Err(VolumeError::Identity);
        }
        Ok(Self {
            dev: m.dev(),
            ino: m.ino(),
        })
    }
    fn data_directory(dir: &Dir) -> Result<Self> {
        let m = dir.try_clone()?.into_std_file().metadata()?;
        if !m.is_dir() || m.uid() != 1000 {
            return Err(VolumeError::Identity);
        }
        Ok(Self {
            dev: m.dev(),
            ino: m.ino(),
        })
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PoolRecord {
    format: u32,
    root: NativeId,
    volumes: NativeId,
    lock: NativeId,
    limits: PoolLimits,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Reserved,
    Formatted,
    Attached,
    Mounted,
    Published,
    PublishedRetiring,
    PublishedDrained,
    PublishedRetired,
    Drained,
    Retired,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    format: u32,
    sequence: u64,
    id: String,
    nonce: String,
    request: VolumeRequest,
    namespace: NativeId,
    image: NativeId,
    base: NativeId,
    journal: [NativeId; 2],
    phase: Phase,
    device: Option<LoopIdentity>,
    mounted: Option<NativeId>,
    mount_id: Option<u64>,
    data_root: Option<NativeId>,
}
struct Inner {
    root: Dir,
    volumes: Dir,
    record: PoolRecord,
    formatter: PinnedFormatter,
    transaction: Mutex<()>,
}
pub struct VolumePool(Arc<Inner>);
#[cfg(feature = "containerd")]
pub(crate) struct ReceiverClaim {
    file: File,
    identity: NativeId,
    pool: Arc<Inner>,
}
#[cfg(feature = "containerd")]
impl ReceiverClaim {
    pub(crate) fn verify(&self) -> Result<()> {
        if NativeId::directory(&self.pool.root)? != self.pool.record.root
            || NativeId::file(&self.file)? != self.identity
            || NativeId::file(&open_file(&self.pool.root, "receiver.lock", true)?)? != self.identity
        {
            return Err(VolumeError::Identity);
        }
        Ok(())
    }
}
pub struct MountedVolume {
    pool: Arc<Inner>,
    namespace: Dir,
    record: Record,
    image: File,
    device: LoopDevice,
    mounted: Dir,
}
/// Publishing is a durable one-way fence. Dropping it never detaches a mounted
/// workspace or releases its reservation while physical execution is unknown.
pub struct PublishedVolume {
    inner: MountedVolume,
}
#[derive(Debug, Serialize)]
pub struct PoolUsage {
    pub reserved_bytes: u64,
    pub reserved_volumes: u16,
    pub retained_records: u16,
}
#[derive(Debug, Serialize)]
pub struct VolumeUsage {
    pub capacity_bytes: u64,
    pub available_bytes: u64,
    pub inode_capacity: u64,
    pub available_inodes: u64,
}

impl VolumePool {
    pub fn initialize(
        directory: Dir,
        limits: PoolLimits,
        formatter: PinnedFormatter,
    ) -> Result<Self> {
        validate_pool(&limits)?;
        require_ext4_storage(&directory)?;
        let root = NativeId::directory(&directory)?;
        if directory.entries()?.next().is_some() {
            return Err(VolumeError::Identity);
        }
        let lock = create_file(&directory, "pool.lock")?;
        let volumes = private_dir(&directory, "volumes")?;
        let record = PoolRecord {
            format: 1,
            root,
            volumes: NativeId::directory(&volumes)?,
            lock: NativeId::file(&lock)?,
            limits,
        };
        new_json(&directory, "pool.json", &record)?;
        sync(&directory)?;
        Ok(Self(Arc::new(Inner {
            root: directory,
            volumes,
            record,
            formatter,
            transaction: Mutex::new(()),
        })))
    }
    pub fn open(directory: Dir, formatter: PinnedFormatter) -> Result<Self> {
        require_ext4_storage(&directory)?;
        let record: PoolRecord = read_json(&directory, "pool.json")?;
        validate_pool(&record.limits)?;
        let volumes = directory.open_dir_nofollow("volumes")?;
        let lock = open_file(&directory, "pool.lock", true)?;
        if record.format != 1
            || record.root != NativeId::directory(&directory)?
            || record.volumes != NativeId::directory(&volumes)?
            || record.lock != NativeId::file(&lock)?
        {
            return Err(VolumeError::Identity);
        }
        Ok(Self(Arc::new(Inner {
            root: directory,
            volumes,
            record,
            formatter,
            transaction: Mutex::new(()),
        })))
    }
    pub fn usage(&self) -> Result<PoolUsage> {
        let _guard = lock(&self.0)?;
        usage(&self.0)
    }
    #[cfg(feature = "containerd")]
    pub(crate) fn require_pool_limits(&self, expected: &PoolLimits) -> Result<()> {
        let _guard = lock(&self.0)?;
        if &self.0.record.limits != expected {
            return Err(VolumeError::Limits);
        }
        Ok(())
    }
    /// A new native service cannot reset compute accounting by reopening a
    /// pool with retained resources. Published/uncertain volumes first require
    /// explicit physical reconciliation. The lifetime lock excludes a second
    /// live receiver over the same original native pool.
    #[cfg(feature = "containerd")]
    pub(crate) fn claim_empty_receiver(&self) -> Result<ReceiverClaim> {
        let _guard = lock(&self.0)?;
        let file = match create_file(&self.0.root, "receiver.lock") {
            Ok(file) => {
                sync(&self.0.root)?;
                file
            }
            Err(VolumeError::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => {
                open_file(&self.0.root, "receiver.lock", true)?
            }
            Err(error) => return Err(error),
        };
        file.try_lock().map_err(|_| VolumeError::Busy)?;
        let current = usage(&self.0)?;
        if current.reserved_bytes != 0 || current.reserved_volumes != 0 {
            return Err(VolumeError::Uncertain);
        }
        let claim = ReceiverClaim {
            identity: NativeId::file(&file)?,
            file,
            pool: self.0.clone(),
        };
        claim.verify()?;
        Ok(claim)
    }
    pub fn reserve(&self, request: VolumeRequest) -> Result<MountedVolume> {
        request.validate()?;
        let _guard = lock(&self.0)?;
        for entry in self.0.volumes.entries()? {
            let name = entry?.file_name();
            let dir = self.0.volumes.open_dir_nofollow(&name)?;
            let record = read_record(&dir)?;
            if record.request.request_sha256 == request.request_sha256 {
                if record.request != request {
                    return Err(VolumeError::Identity);
                }
                // A replay observes the existing exact native workspace. It
                // never allocates, reformats or reconstructs missing data.
                return restore(self.0.clone(), dir, record);
            }
        }
        let used = usage(&self.0)?;
        if used.retained_records >= 4096
            || used.reserved_volumes >= self.0.record.limits.max_volumes
            || used
                .reserved_bytes
                .checked_add(request.bytes)
                .is_none_or(|v| v > self.0.record.limits.reserved_bytes)
        {
            return Err(VolumeError::Capacity);
        }
        let id = uuid::Uuid::new_v4().to_string();
        let dir = private_dir(&self.0.volumes, &id)?;
        let image = create_file(&dir, "image.ext4")?;
        let base = private_dir(&dir, "root")?;
        let a = journal_file(&dir, "volume.a")?;
        let b = journal_file(&dir, "volume.b")?;
        let mut record = Record {
            format: 1,
            sequence: 0,
            id,
            nonce: uuid::Uuid::new_v4().simple().to_string(),
            request,
            namespace: NativeId::directory(&dir)?,
            image: NativeId::file(&image)?,
            base: NativeId::directory(&base)?,
            journal: [NativeId::file(&a)?, NativeId::file(&b)?],
            phase: Phase::Reserved,
            device: None,
            mounted: None,
            mount_id: None,
            data_root: None,
        };
        replace_json(&dir, &mut record)?;
        sync(&dir)?;
        sync(&self.0.volumes)?;
        drop(base);
        let allocated = unsafe {
            libc::posix_fallocate(image.as_raw_fd(), 0, record.request.bytes as libc::off_t)
        };
        if allocated != 0 {
            return Err(io::Error::from_raw_os_error(allocated).into());
        }
        image.sync_all()?;
        check_allocated(&image, record.request.bytes)?;
        self.0.formatter.format(
            &image,
            &dir,
            &record.request,
            &uuid::Uuid::new_v4().to_string(),
        )?;
        record.phase = Phase::Formatted;
        replace_json(&dir, &mut record)?;
        let device = LoopDevice::attach(&image, &record.nonce, record.request.bytes)?;
        record.device = Some(device.identity.clone());
        record.phase = Phase::Attached;
        replace_json(&dir, &mut record)?;
        mount(&device, &dir)?;
        let mounted = dir.open_dir_nofollow("root")?;
        mounted.set_permissions(".", Permissions::from_mode(0o700))?;
        let work = private_dir(&mounted, "work")?;
        let work_handle = work.open(".")?.into_std();
        if unsafe { libc::fchown(work_handle.as_raw_fd(), 1000, 1000) } < 0 {
            return Err(io::Error::last_os_error().into());
        }
        record.data_root = Some(NativeId::data_directory(&work)?);
        drop(work_handle);
        drop(work);
        record.mounted = Some(NativeId::directory(&mounted)?);
        record.mount_id = Some(mount_id(&mounted)?);
        require_fs(&mounted, &record)?;
        record.phase = Phase::Mounted;
        replace_json(&dir, &mut record)?;
        Ok(MountedVolume {
            pool: self.0.clone(),
            namespace: dir,
            record,
            image,
            device,
            mounted,
        })
    }
    pub fn reopen_unlaunched(&self, id: &str) -> Result<MountedVolume> {
        if uuid::Uuid::parse_str(id).is_err() {
            return Err(VolumeError::Identity);
        }
        let _guard = lock(&self.0)?;
        let dir = self.0.volumes.open_dir_nofollow(id)?;
        let record = read_record(&dir)?;
        if record.id != id {
            return Err(VolumeError::Identity);
        }
        restore(self.0.clone(), dir, record)
    }
    /// Explicit reconciliation of a reservation which never crossed the
    /// durable publish fence. Incomplete formatting/attachment is cleanup-only;
    /// it can never be adopted as permission for a new process dispatch.
    pub fn abort_unlaunched(&self, id: &str) -> Result<()> {
        if uuid::Uuid::parse_str(id).is_err() {
            return Err(VolumeError::Identity);
        }
        let _guard = lock(&self.0)?;
        let dir = self.0.volumes.open_dir_nofollow(id)?;
        let record = read_record(&dir)?;
        if record.id != id || record.phase == Phase::Published {
            return Err(VolumeError::Uncertain);
        }
        abort(dir, record)
    }
}
impl MountedVolume {
    #[cfg(feature = "containerd")]
    pub(crate) fn require_request(&self, expected: &VolumeRequest) -> Result<()> {
        if &self.record.request != expected {
            return Err(VolumeError::Limits);
        }
        self.usage()?;
        Ok(())
    }
    pub fn id(&self) -> &str {
        &self.record.id
    }
    pub fn directory(&self) -> &Dir {
        &self.mounted
    }
    /// Only this subordinate directory is eligible for a workload mount. The
    /// filesystem root, transfer staging and the namespace journal stay private.
    pub fn data_directory(&self) -> Result<Dir> {
        let dir = self.mounted.open_dir_nofollow("work")?;
        if self.record.data_root.as_ref() != Some(&NativeId::data_directory(&dir)?) {
            return Err(VolumeError::Identity);
        }
        Ok(dir)
    }
    pub fn usage(&self) -> Result<VolumeUsage> {
        if NativeId::file(&self.image)? != self.record.image {
            return Err(VolumeError::Identity);
        }
        self.device.require(
            &self.record.image,
            &self.record.nonce,
            self.record.request.bytes,
        )?;
        require_fs(&self.mounted, &self.record)?;
        filesystem_usage(&self.mounted)
    }
    pub fn publish_for_dispatch(self) -> Result<PublishedVolume> {
        let pool = self.pool.clone();
        let _guard = lock(&pool)?;
        self.usage()?;
        self.data_directory()?;
        if NativeId::file(&open_file(&self.namespace, "image.ext4", true)?)? != self.record.image {
            return Err(VolumeError::Identity);
        }
        let mut record = read_record(&self.namespace)?;
        require_same(&self.record, &record)?;
        if record.phase != Phase::Mounted {
            return Err(VolumeError::Uncertain);
        }
        // Existence is an independent one-way fence. A torn/corrupt newest
        // journal slot must never downgrade a published volume to unlaunched.
        let mut fence = create_file(&self.namespace, "publish.fence")?;
        fence.write_all(fence_bytes(&record).as_bytes())?;
        fence.sync_all()?;
        sync(&self.namespace)?;
        record.phase = Phase::Published;
        replace_json(&self.namespace, &mut record)?;
        Ok(PublishedVolume {
            inner: Self { record, ..self },
        })
    }
    pub fn retire_unlaunched(self) -> Result<()> {
        let pool = self.pool.clone();
        let _guard = lock(&pool)?;
        let record = read_record(&self.namespace)?;
        require_same(&self.record, &record)?;
        if record.phase != Phase::Mounted {
            return Err(VolumeError::Uncertain);
        }
        let namespace = self.namespace.try_clone()?;
        drop(self);
        abort(namespace, record)
    }
}
impl PublishedVolume {
    #[cfg(feature = "containerd")]
    pub(crate) fn require_limits(&self, bytes: u64, inodes: u32) -> Result<()> {
        if self.inner.record.request.bytes != bytes || self.inner.record.request.inodes != inodes {
            return Err(VolumeError::Limits);
        }
        self.verify_published()
    }
    #[cfg(feature = "containerd")]
    pub(crate) fn request_sha256(&self) -> &str {
        &self.inner.record.request.request_sha256
    }
    #[cfg(feature = "containerd")]
    pub(crate) fn verify_published(&self) -> Result<()> {
        let _guard = lock(&self.inner.pool)?;
        self.inner.usage()?;
        let current = read_record(&self.inner.namespace)?;
        require_same(&self.inner.record, &current)?;
        if current.phase != Phase::Published {
            return Err(VolumeError::Uncertain);
        }
        Ok(())
    }
    pub fn id(&self) -> &str {
        &self.inner.record.id
    }
    pub fn directory(&self) -> &Dir {
        &self.inner.mounted
    }
    pub fn data_directory(&self) -> Result<Dir> {
        self.inner.data_directory()
    }
}

fn validate_pool(limits: &PoolLimits) -> Result<()> {
    if !(16 * 1024 * 1024..=64 * 1024 * 1024 * 1024).contains(&limits.reserved_bytes)
        || !(1..=128).contains(&limits.max_volumes)
    {
        return Err(VolumeError::Limits);
    }
    Ok(())
}
fn usage(pool: &Inner) -> Result<PoolUsage> {
    let mut result = PoolUsage {
        reserved_bytes: 0,
        reserved_volumes: 0,
        retained_records: 0,
    };
    let mut entries = 0;
    for entry in pool.volumes.entries()? {
        entries += 1;
        if entries > 4096 {
            return Err(VolumeError::Uncertain);
        }
        result.retained_records += 1;
        let name = entry?.file_name();
        let Some(id) = name.to_str() else {
            return Err(VolumeError::Identity);
        };
        if uuid::Uuid::parse_str(id).is_err() {
            return Err(VolumeError::Identity);
        }
        let dir = pool.volumes.open_dir_nofollow(id)?;
        let record = read_record(&dir)?;
        if record.id != id {
            return Err(VolumeError::Identity);
        }
        if matches!(record.phase, Phase::Retired | Phase::PublishedRetired) {
            match dir.symlink_metadata("image.ext4") {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                _ => return Err(VolumeError::Uncertain),
            }
            let base = dir.open_dir_nofollow("root")?;
            if NativeId::directory(&base)? != record.base || mount_id(&base)? != mount_id(&dir)? {
                return Err(VolumeError::Identity);
            }
            continue;
        }
        result.reserved_bytes = result
            .reserved_bytes
            .checked_add(record.request.bytes)
            .ok_or(VolumeError::Capacity)?;
        result.reserved_volumes = result
            .reserved_volumes
            .checked_add(1)
            .ok_or(VolumeError::Capacity)?;
    }
    Ok(result)
}
fn restore(pool: Arc<Inner>, namespace: Dir, record: Record) -> Result<MountedVolume> {
    if record.phase != Phase::Mounted {
        return Err(VolumeError::Uncertain);
    }
    let image = open_file(&namespace, "image.ext4", true)?;
    if NativeId::file(&image)? != record.image {
        return Err(VolumeError::Identity);
    }
    check_allocated(&image, record.request.bytes)?;
    let device = LoopDevice::reopen(
        record.device.as_ref().ok_or(VolumeError::Identity)?,
        &record.image,
        &record.nonce,
        record.request.bytes,
    )?;
    let mounted = namespace.open_dir_nofollow("root")?;
    if record.mounted.as_ref() != Some(&NativeId::directory(&mounted)?)
        || record.mount_id != Some(mount_id(&mounted)?)
    {
        return Err(VolumeError::Identity);
    }
    require_fs(&mounted, &record)?;
    let volume = MountedVolume {
        pool,
        namespace,
        record,
        image,
        device,
        mounted,
    };
    volume.data_directory()?;
    Ok(volume)
}
fn abort(namespace: Dir, record: Record) -> Result<()> {
    if matches!(
        record.phase,
        Phase::Published
            | Phase::PublishedRetiring
            | Phase::PublishedDrained
            | Phase::PublishedRetired
    ) {
        return Err(VolumeError::Uncertain);
    }
    drain(namespace, record, false)
}
fn drain(namespace: Dir, mut record: Record, published: bool) -> Result<()> {
    if published
        != matches!(
            record.phase,
            Phase::PublishedRetiring | Phase::PublishedDrained | Phase::PublishedRetired
        )
        || record.phase == Phase::Published
    {
        return Err(VolumeError::Uncertain);
    }
    let image = match open_file(&namespace, "image.ext4", true) {
        Ok(image) => Some(image),
        Err(VolumeError::Io(error))
            if error.kind() == io::ErrorKind::NotFound
                && matches!(
                    record.phase,
                    Phase::Drained
                        | Phase::Retired
                        | Phase::PublishedDrained
                        | Phase::PublishedRetired
                ) =>
        {
            None
        }
        Err(error) => return Err(error),
    };
    if let Some(image) = &image
        && NativeId::file(image)? != record.image
    {
        return Err(VolumeError::Identity);
    }
    let devices = if image.is_some() {
        loop_device::owned(&record.image, &record.nonce, record.request.bytes)?
    } else {
        Vec::new()
    };
    if devices.len() > 1
        || (devices.len() == 1
            && record
                .device
                .as_ref()
                .is_some_and(|expected| expected != &devices[0].identity))
    {
        return Err(VolumeError::Identity);
    }
    let mounted = namespace.open_dir_nofollow("root")?;
    let metadata = mounted.try_clone()?.into_std_file().metadata()?;
    let current = NativeId {
        dev: metadata.dev(),
        ino: metadata.ino(),
    };
    if current != record.base {
        if !metadata.is_dir()
            || metadata.uid() != 0
            || devices.len() != 1
            || current.dev != devices[0].identity.rdev
            || record
                .mounted
                .as_ref()
                .is_some_and(|expected| *expected != current)
            || record
                .mount_id
                .is_some_and(|id| mount_id(&mounted).ok() != Some(id))
        {
            return Err(VolumeError::Identity);
        }
    } else if mount_id(&mounted)? != mount_id(&namespace)? {
        return Err(VolumeError::Identity);
    }
    let target = fd_child_path(namespace.as_raw_fd(), "root");
    drop(mounted);
    let target = CString::new(target.as_os_str().as_bytes()).map_err(|_| VolumeError::Identity)?;
    if current != record.base && unsafe { libc::umount2(target.as_ptr(), 0) } < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let base = namespace.open_dir_nofollow("root")?;
    if NativeId::directory(&base)? != record.base {
        return Err(VolumeError::Identity);
    }
    for device in devices {
        device.detach(&record.image, &record.nonce, record.request.bytes)?;
    }
    if image.is_some() && loop_device::associated(&record.image)? {
        return Err(VolumeError::StillAttached);
    }
    if matches!(record.phase, Phase::Retired | Phase::PublishedRetired) {
        if published && image.is_some() {
            return Err(VolumeError::Uncertain);
        }
        return Ok(());
    }
    record.phase = if published {
        Phase::PublishedDrained
    } else {
        Phase::Drained
    };
    replace_json(&namespace, &mut record)?;
    if image.is_some() {
        drop(image);
        let current = open_file(&namespace, "image.ext4", true)?;
        if NativeId::file(&current)? != record.image {
            return Err(VolumeError::Identity);
        }
        drop(current);
        namespace.remove_file("image.ext4")?;
        sync(&namespace)?;
    }
    record.phase = if published {
        Phase::PublishedRetired
    } else {
        Phase::Retired
    };
    replace_json(&namespace, &mut record)?;
    Ok(())
}
fn require_same(a: &Record, b: &Record) -> Result<()> {
    if a.id != b.id
        || a.nonce != b.nonce
        || a.request != b.request
        || a.namespace != b.namespace
        || a.image != b.image
        || a.base != b.base
        || a.journal != b.journal
        || a.device != b.device
        || a.mounted != b.mounted
        || a.mount_id != b.mount_id
        || a.data_root != b.data_root
    {
        return Err(VolumeError::Identity);
    }
    Ok(())
}
fn read_record(dir: &Dir) -> Result<Record> {
    let a = open_file(dir, "volume.a", false)?;
    let b = open_file(dir, "volume.b", false)?;
    let identities = [NativeId::file(&a)?, NativeId::file(&b)?];
    let mut records = [read_slot(a)?, read_slot(b)?]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    records.sort_by_key(|r| r.sequence);
    let record = records.pop().ok_or(VolumeError::Uncertain)?;
    if records
        .last()
        .is_some_and(|r| r.sequence == record.sequence)
    {
        return Err(VolumeError::Uncertain);
    }
    record.request.validate()?;
    if record.format != 1
        || record.sequence == 0
        || record.journal != identities
        || uuid::Uuid::parse_str(&record.id).is_err()
        || record.nonce.len() != 32
        || !record.nonce.bytes().all(|b| b.is_ascii_hexdigit())
        || record.namespace != NativeId::directory(dir)?
    {
        return Err(VolumeError::Identity);
    }
    for entry in dir.entries()? {
        let name = entry?.file_name();
        if !matches!(
            name.to_str(),
            Some("volume.a" | "volume.b" | "publish.fence" | "image.ext4" | "root")
        ) {
            return Err(VolumeError::Uncertain);
        }
    }
    let fence = match open_file(dir, "publish.fence", false) {
        Ok(file) => Some(file),
        Err(VolumeError::Io(error)) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    match (fence, record.phase) {
        (
            Some(mut file),
            Phase::Published
            | Phase::PublishedRetiring
            | Phase::PublishedDrained
            | Phase::PublishedRetired,
        ) => {
            let mut bytes = Vec::new();
            Read::by_ref(&mut file).take(512).read_to_end(&mut bytes)?;
            if bytes != fence_bytes(&record).as_bytes() {
                return Err(VolumeError::Uncertain);
            }
        }
        (
            None,
            Phase::Published
            | Phase::PublishedRetiring
            | Phase::PublishedDrained
            | Phase::PublishedRetired,
        )
        | (Some(_), _) => return Err(VolumeError::Uncertain),
        _ => {}
    }
    Ok(record)
}
struct PoolLock<'a> {
    // Release the native lock before waking the next local transaction.
    _file: File,
    _local: MutexGuard<'a, ()>,
}
fn lock(pool: &Inner) -> Result<PoolLock<'_>> {
    // Clones of the original receiver share this mutex. A current observation
    // therefore sees a coherent transaction rather than treating its own
    // reserve/format/publish operation as a competing native owner. Independently
    // opened pools still use the unchanged nonblocking native identity lock.
    // Poisoning remains fail-closed; this never repairs or adopts retained state.
    let local = pool
        .transaction
        .lock()
        .map_err(|_| VolumeError::Uncertain)?;
    for entry in pool.root.entries()? {
        if !matches!(
            entry?.file_name().to_str(),
            Some("pool.json" | "pool.lock" | "receiver.lock" | "volumes")
        ) {
            return Err(VolumeError::Uncertain);
        }
    }
    if NativeId::directory(&pool.root)? != pool.record.root
        || NativeId::directory(&pool.volumes)? != pool.record.volumes
    {
        return Err(VolumeError::Identity);
    }
    let file = open_file(&pool.root, "pool.lock", true)?;
    if NativeId::file(&file)? != pool.record.lock {
        return Err(VolumeError::Identity);
    }
    file.try_lock().map_err(|_| VolumeError::Busy)?;
    Ok(PoolLock {
        _file: file,
        _local: local,
    })
}
fn create_file(dir: &Dir, name: &str) -> Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    {
        use cap_std::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let file = dir.open_with(name, &options)?.into_std();
    NativeId::file(&file)?;
    Ok(file)
}
fn open_file(dir: &Dir, name: &str, write: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(write).follow(FollowSymlinks::No);
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let file = dir.open_with(name, &options)?.into_std();
    NativeId::file(&file)?;
    Ok(file)
}
fn private_dir(parent: &Dir, name: &str) -> Result<Dir> {
    parent.create_dir(name)?;
    parent.set_permissions(name, Permissions::from_mode(0o700))?;
    let dir = parent.open_dir_nofollow(name)?;
    NativeId::directory(&dir)?;
    sync(parent)?;
    Ok(dir)
}
fn new_json<T: Serialize>(dir: &Dir, name: &str, value: &T) -> Result<()> {
    let mut file = create_file(dir, name)?;
    file.write_all(&serde_json::to_vec(value)?)?;
    file.sync_all()?;
    Ok(())
}
const SLOT_BYTES: usize = 8192;
fn journal_file(dir: &Dir, name: &str) -> Result<File> {
    let file = create_file(dir, name)?;
    let result = unsafe { libc::posix_fallocate(file.as_raw_fd(), 0, SLOT_BYTES as libc::off_t) };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result).into());
    }
    check_allocated(&file, SLOT_BYTES as u64)?;
    file.sync_all()?;
    Ok(file)
}
fn replace_json(dir: &Dir, record: &mut Record) -> Result<()> {
    let next = record
        .sequence
        .checked_add(1)
        .ok_or(VolumeError::Uncertain)?;
    let mut candidate = record.clone();
    candidate.sequence = next;
    let payload = serde_json::to_vec(&candidate)?;
    if payload.len() > SLOT_BYTES - 36 {
        return Err(VolumeError::Uncertain);
    }
    let index = next as usize % 2;
    let mut file = open_file(dir, if index == 0 { "volume.a" } else { "volume.b" }, true)?;
    if NativeId::file(&file)? != record.journal[index] {
        return Err(VolumeError::Identity);
    }
    check_allocated(&file, SLOT_BYTES as u64)?;
    let mut bytes = [0u8; SLOT_BYTES];
    bytes[..4].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes[4..36].copy_from_slice(&Sha256::digest(&payload));
    bytes[36..36 + payload.len()].copy_from_slice(&payload);
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    record.sequence = next;
    Ok(())
}
fn read_slot(mut file: File) -> Result<Option<Record>> {
    check_allocated(&file, SLOT_BYTES as u64)?;
    let mut bytes = [0u8; SLOT_BYTES];
    file.read_exact(&mut bytes)?;
    let len =
        u32::from_le_bytes(bytes[..4].try_into().map_err(|_| VolumeError::Identity)?) as usize;
    if len == 0 || len > SLOT_BYTES - 36 {
        return Ok(None);
    }
    let payload = &bytes[36..36 + len];
    if Sha256::digest(payload).as_slice() != &bytes[4..36]
        || bytes[36 + len..].iter().any(|b| *b != 0)
    {
        return Ok(None);
    }
    Ok(serde_json::from_slice(payload).ok())
}
fn fence_bytes(record: &Record) -> String {
    format!(
        "cowork-runtime-volume-published-v1\n{}\n{}\n{}\n",
        record.id, record.nonce, record.request.request_sha256
    )
}
fn read_json<T: serde::de::DeserializeOwned>(dir: &Dir, name: &str) -> Result<T> {
    let mut file = open_file(dir, name, false)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(65537)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err(VolumeError::Identity);
    }
    Ok(serde_json::from_slice(&bytes)?)
}
fn sync(dir: &Dir) -> Result<()> {
    // Capability directories may use O_PATH, which cannot fsync. Open the same
    // directory relative to that capability with a readable descriptor.
    dir.open(".")?.into_std().sync_all()?;
    Ok(())
}
fn check_allocated(file: &File, bytes: u64) -> Result<()> {
    let m = file.metadata()?;
    if m.len() != bytes || m.blocks().checked_mul(512).is_none_or(|v| v < bytes) {
        return Err(VolumeError::Capacity);
    }
    Ok(())
}

#[cfg(all(test, feature = "containerd"))]
mod receiver_claim_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    #[ignore = "Explicit bounded own ext4 fixture; no workload processes"]
    fn original_receiver_claim_excludes_duplicates_and_retained_resource_restart() {
        assert_eq!(unsafe { libc::geteuid() }, 0);
        let base = PathBuf::from(std::env::var("COWORK_RUNTIME_VOLUME_PROOF_DIR").unwrap());
        assert_eq!(base.file_name().unwrap(), "case-storage");
        let path = base.join(format!("receiver-claim-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let directory = Dir::open_ambient_dir(&path, cap_std::ambient_authority()).unwrap();
        let formatter = || {
            let path = Path::new("/usr/sbin/mke2fs").canonicalize().unwrap();
            let sha = format!("{:x}", Sha256::digest(std::fs::read(&path).unwrap()));
            PinnedFormatter::open(&path, &sha).unwrap()
        };
        let pool = VolumePool::initialize(
            directory.try_clone().unwrap(),
            PoolLimits {
                reserved_bytes: 64 * 1024 * 1024,
                max_volumes: 2,
            },
            formatter(),
        )
        .unwrap();
        let original = pool.claim_empty_receiver().unwrap();
        let other = VolumePool::open(directory.try_clone().unwrap(), formatter()).unwrap();
        // A status observation in this receiver must wait for its own pool
        // transaction. A separately opened pool must still fail the native
        // nonblocking owner lock, rather than wait or adopt this receiver.
        let held = lock(&pool.0).unwrap();
        let same_receiver = VolumePool(pool.0.clone());
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let observer = scope.spawn(move || {
                entered_tx.send(()).unwrap();
                result_tx.send(same_receiver.usage()).unwrap();
            });
            entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(matches!(
                result_rx.recv_timeout(Duration::from_millis(250)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ));
            assert!(matches!(other.usage(), Err(VolumeError::Busy)));
            drop(held);
            let observed = result_rx
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap();
            assert_eq!(observed.reserved_bytes, 0);
            assert_eq!(observed.reserved_volumes, 0);
            observer.join().unwrap();
        });
        assert!(matches!(
            other.claim_empty_receiver(),
            Err(VolumeError::Busy)
        ));
        original.verify().unwrap();
        // Equal names and bytes cannot substitute the retained native lock.
        directory
            .rename("receiver.lock", &directory, "original-receiver.lock")
            .unwrap();
        drop(create_file(&directory, "receiver.lock").unwrap());
        assert!(original.verify().is_err());
        directory.remove_file("receiver.lock").unwrap();
        directory
            .rename("original-receiver.lock", &directory, "receiver.lock")
            .unwrap();
        original.verify().unwrap();
        let request = VolumeRequest {
            request_sha256: "a".repeat(64),
            bytes: 32 * 1024 * 1024,
            inodes: 2048,
        };
        let volume = pool.reserve(request.clone()).unwrap();
        drop(original);
        drop(volume);
        assert!(matches!(
            other.claim_empty_receiver(),
            Err(VolumeError::Uncertain)
        ));
        // Only a positively never-published reservation can be retired here.
        pool.reserve(request).unwrap().retire_unlaunched().unwrap();
        let claim = other.claim_empty_receiver().unwrap();
        claim.verify().unwrap();
        let published = pool
            .reserve(VolumeRequest {
                request_sha256: "b".repeat(64),
                bytes: 32 * 1024 * 1024,
                inodes: 2048,
            })
            .unwrap()
            .publish_for_dispatch()
            .unwrap();
        drop(claim);
        drop(published);
        assert!(matches!(
            other.claim_empty_receiver(),
            Err(VolumeError::Uncertain)
        ));
        assert_eq!(pool.usage().unwrap().reserved_bytes, 32 * 1024 * 1024);
        println!(
            "receiver_claim=verified; same_receiver_observation=serialized; independent_pool_lock=rejected; duplicate_receiver=rejected; replaced_native_lock=rejected; retained_restart=rejected; proven_unpublished_retirement=verified; published_reservation_retained=true; workload_tasks_launched=false"
        );
    }
}
fn fd_path(fd: i32) -> PathBuf {
    PathBuf::from(format!("/proc/{}/fd/{fd}", std::process::id()))
}
fn fd_child_path(fd: i32, name: &str) -> PathBuf {
    fd_path(fd).join(name)
}
fn require_ext4_storage(dir: &Dir) -> Result<()> {
    let file = dir.open(".")?.into_std();
    let mut value = std::mem::MaybeUninit::<libc::statfs>::zeroed();
    if unsafe { libc::fstatfs(file.as_raw_fd(), value.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error().into());
    }
    if unsafe { value.assume_init() }.f_type != 0xef53 {
        return Err(VolumeError::Limits);
    }
    // ext2/ext3 use the same magic. Pin the actual kernel mount type as well;
    // an unsupported backing filesystem never becomes an implicit fallback.
    let id = mount_id(dir)?;
    let mut mounts = String::new();
    File::open("/proc/self/mountinfo")?
        .take(4 * 1024 * 1024 + 1)
        .read_to_string(&mut mounts)?;
    if mounts.len() > 4 * 1024 * 1024
        || !mounts.lines().any(|line| {
            line.split_ascii_whitespace()
                .next()
                .and_then(|v| v.parse::<u64>().ok())
                == Some(id)
                && line
                    .split_once(" - ")
                    .and_then(|(_, tail)| tail.split_ascii_whitespace().next())
                    == Some("ext4")
        })
    {
        return Err(VolumeError::Limits);
    }
    Ok(())
}
fn mount(device: &LoopDevice, namespace: &Dir) -> Result<()> {
    let source =
        CString::new(device.source().as_os_str().as_bytes()).map_err(|_| VolumeError::Identity)?;
    let target = CString::new(
        fd_child_path(namespace.as_raw_fd(), "root")
            .as_os_str()
            .as_bytes(),
    )
    .map_err(|_| VolumeError::Identity)?;
    let flags = libc::MS_NODEV | libc::MS_NOSUID | libc::MS_NOEXEC;
    if unsafe {
        libc::mount(
            source.as_ptr(),
            target.as_ptr(),
            c"ext4".as_ptr(),
            flags,
            std::ptr::null(),
        )
    } < 0
    {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}
fn mount_id(dir: &Dir) -> Result<u64> {
    let mut value = std::mem::MaybeUninit::<libc::statx>::zeroed();
    if unsafe {
        libc::statx(
            dir.as_raw_fd(),
            c"".as_ptr(),
            libc::AT_EMPTY_PATH,
            libc::STATX_MNT_ID,
            value.as_mut_ptr(),
        )
    } < 0
    {
        return Err(io::Error::last_os_error().into());
    }
    let value = unsafe { value.assume_init() };
    if value.stx_mask & libc::STATX_MNT_ID == 0 {
        return Err(VolumeError::Identity);
    }
    Ok(value.stx_mnt_id)
}
fn filesystem_usage(dir: &Dir) -> Result<VolumeUsage> {
    let mut value = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    if unsafe { libc::fstatvfs(dir.as_raw_fd(), value.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let value = unsafe { value.assume_init() };
    if value.f_flag & (libc::ST_NODEV | libc::ST_NOSUID | libc::ST_NOEXEC)
        != (libc::ST_NODEV | libc::ST_NOSUID | libc::ST_NOEXEC)
    {
        return Err(VolumeError::Identity);
    }
    Ok(VolumeUsage {
        capacity_bytes: value
            .f_blocks
            .checked_mul(value.f_frsize)
            .ok_or(VolumeError::Limits)?,
        available_bytes: value
            .f_bavail
            .checked_mul(value.f_frsize)
            .ok_or(VolumeError::Limits)?,
        inode_capacity: value.f_files,
        available_inodes: value.f_favail,
    })
}
fn require_fs(dir: &Dir, record: &Record) -> Result<()> {
    let mut value = std::mem::MaybeUninit::<libc::statfs>::zeroed();
    if unsafe { libc::fstatfs(dir.as_raw_fd(), value.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let value = unsafe { value.assume_init() };
    let actual = NativeId::directory(dir)?;
    if value.f_type != 0xef53 || record.device.as_ref().is_none_or(|d| d.rdev != actual.dev) {
        return Err(VolumeError::Identity);
    }
    let usage = filesystem_usage(dir)?;
    if usage.capacity_bytes > record.request.bytes
        || usage.inode_capacity > record.request.inodes as u64
    {
        return Err(VolumeError::Limits);
    }
    Ok(())
}
