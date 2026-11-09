//! Operator/package bootstrap only. There is deliberately no network install,
//! discovery, qualification minting, global controller change or host-command
//! input. Successful activation is distinct from installation/qualification.
use super::{
    backend::ManagedReceiver,
    cgroups::CgroupParent,
    deadline_process::FenceExecutable,
    gvisor::{GvisorTools, Stamp, require_owned_ancestors},
    profile::{ManagedProfile, RuntimeArtifacts},
    rootfs::SealedRootfs,
    runsc_driver::RunscDriverExecutable,
    *,
};
use crate::volumes::{PinnedFormatter, PoolLimits, VolumePool};
use cap_std::fs::Dir;
use cowork_contracts::{ExecutionEnvironmentRef, ExecutionTargetRef};
use cowork_domain::environments::{EnvironmentDefinition, EnvironmentQualification};
use cowork_runner_protocol::{Binding, Broker, Signed};
use serde::{Deserialize, Serialize};
use std::os::fd::AsRawFd;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinnedRuntimeFile {
    pub path: PathBuf,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedBootstrap {
    pub format_version: u32,
    pub installed_profile: PinnedRuntimeFile,
    pub gvisor_directory: PathBuf,
    pub gvisor_manifest: PinnedRuntimeFile,
    pub rootfs_directory: PathBuf,
    pub rootfs_manifest: PinnedRuntimeFile,
    pub containerd: PinnedRuntimeFile,
    pub fence: PinnedRuntimeFile,
    pub runsc_driver: PinnedRuntimeFile,
    pub formatter: PinnedRuntimeFile,
    pub volume_pool_directory: PathBuf,
    pub volume_pool_limits: PoolLimits,
    /// The private data filesystem itself must be bounded, not just its guests.
    /// Bootstrap refuses a shared/unbounded host filesystem instead of changing it.
    pub data_filesystem_max_bytes: u64,
    pub delegated_cgroup_directory: PathBuf,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledManagedProfile {
    pub format_version: u32,
    pub binding: Binding,
    pub target: ExecutionTargetRef,
    pub reference: ExecutionEnvironmentRef,
    pub definition: EnvironmentDefinition,
    pub qualification: Signed<EnvironmentQualification>,
}
impl ManagedBootstrap {
    pub fn installed(&self) -> Result<InstalledManagedProfile> {
        if self.format_version != 1 {
            return Err(Error::TaskAdmission);
        }
        let profile: InstalledManagedProfile =
            serde_json::from_slice(&self.installed_profile.read(2 * 1024 * 1024)?)
                .map_err(|_| Error::TaskAdmission)?;
        if profile.format_version != 1 {
            return Err(Error::TaskAdmission);
        }
        // Independently trusted config pins this file; it does not become valid
        // merely because an archive claims its own hash or qualification.
        profile
            .qualification
            .verify(
                "environment_qualification",
                &profile.binding.runner_public_key,
            )
            .map_err(|_| Error::TaskAdmission)?;
        Ok(profile)
    }
    pub fn state_directory(&self, path: &Path) -> Result<Dir> {
        require_owned_ancestors(path)?;
        let directory = Dir::open_ambient_dir(path, cap_std::ambient_authority())?;
        bounded_data_filesystem(&directory, self.data_filesystem_max_bytes)?;
        if directory.open(".")?.into_std().metadata()?.dev()
            != std::fs::metadata(&self.volume_pool_directory)?.dev()
        {
            return Err(Error::TaskAdmission);
        }
        Ok(directory)
    }
    pub fn activate(&self, broker: Broker) -> Result<ManagedReceiver> {
        if unsafe { libc::geteuid() } != 0 {
            return Err(Error::TaskAdmission);
        }
        let installed = self.installed()?;
        if !broker.is_bound_to(&installed.binding) {
            return Err(Error::TaskAdmission);
        }
        let gvisor_manifest = self.gvisor_manifest.read(65536)?;
        let rootfs_manifest = self.rootfs_manifest.read(16 * 1024 * 1024)?;
        let profile = Arc::new(ManagedProfile::from_qualification(
            installed.binding,
            installed.target,
            installed.reference,
            installed.definition,
            installed.qualification,
            RuntimeArtifacts {
                tools: GvisorTools::open(
                    &self.gvisor_directory,
                    &gvisor_manifest,
                    &self.gvisor_manifest.sha256,
                )?,
                rootfs: SealedRootfs::open(
                    &self.rootfs_directory,
                    &rootfs_manifest,
                    &self.rootfs_manifest.sha256,
                )?,
                fence: FenceExecutable::open(&self.fence.path, &self.fence.sha256)?,
                driver: RunscDriverExecutable::open(
                    &self.runsc_driver.path,
                    &self.runsc_driver.sha256,
                )?,
            },
        )?);
        let daemon = DaemonExecutable::open(&self.containerd.path, &self.containerd.sha256)?;
        require_owned_ancestors(&self.volume_pool_directory)?;
        let directory =
            Dir::open_ambient_dir(&self.volume_pool_directory, cap_std::ambient_authority())?;
        bounded_data_filesystem(&directory, self.data_filesystem_max_bytes)?;
        let formatter = PinnedFormatter::open(&self.formatter.path, &self.formatter.sha256)
            .map_err(|_| Error::TaskAdmission)?;
        let pool = match directory.symlink_metadata("pool.json") {
            Ok(_) => VolumePool::open(directory, formatter),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                VolumePool::initialize(directory, self.volume_pool_limits.clone(), formatter)
            }
            Err(error) => return Err(error.into()),
        }
        .map_err(|_| Error::TaskAdmission)?;
        pool.require_pool_limits(&self.volume_pool_limits)
            .map_err(|_| Error::TaskAdmission)?;
        // Refuse before creating a new task generation when explicit physical
        // reconciliation is needed. Never adopt/reclassify any old mount/task.
        if pool
            .usage()
            .map_err(|_| Error::ManagedUncertain)?
            .reserved_volumes
            != 0
        {
            return Err(Error::ManagedUncertain);
        }
        let group = CgroupParent::create_in(&self.delegated_cgroup_directory)?;
        ManagedReceiver::new(profile, broker, pool, group, daemon)
    }
}
impl PinnedRuntimeFile {
    pub fn read(&self, maximum: u64) -> Result<Vec<u8>> {
        if !crate::valid_hash(&self.sha256) || maximum == 0 || maximum > 16 * 1024 * 1024 {
            return Err(Error::TaskAdmission);
        }
        let bytes = read_operator_file(&self.path, maximum, false)?;
        if cowork_domain::hash(&bytes) != self.sha256 {
            return Err(Error::TaskAdmission);
        }
        Ok(bytes)
    }
}
/// Private service/installer configuration only. No network route exposes this
/// reader. Secret files require root-only permissions and all reads are bounded.
pub fn read_operator_file(path: &Path, maximum: u64, secret: bool) -> Result<Vec<u8>> {
    if maximum == 0 || maximum > 16 * 1024 * 1024 {
        return Err(Error::TaskAdmission);
    }
    require_owned_ancestors(path.parent().ok_or(Error::TaskAdmission)?)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != 0
        || metadata.nlink() != 1
        || metadata.mode() & 0o022 != 0
        || (secret && metadata.mode() & 0o077 != 0)
        || metadata.len() == 0
        || metadata.len() > maximum
    {
        return Err(Error::TaskAdmission);
    }
    let stamp = Stamp::of(&metadata);
    let mut bytes = Vec::new();
    Read::by_ref(&mut &file)
        .take(maximum + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum || Stamp::of(&file.metadata()?) != stamp {
        return Err(Error::TaskAdmission);
    }
    Ok(bytes)
}

fn bounded_data_filesystem(directory: &Dir, maximum: u64) -> Result<()> {
    if !(64 * 1024 * 1024..=64 * 1024 * 1024 * 1024).contains(&maximum) {
        return Err(Error::TaskAdmission);
    }
    let file = directory.open(".")?.into_std();
    let metadata = file.metadata()?;
    if !metadata.is_dir() || metadata.uid() != 0 || metadata.mode() & 0o077 != 0 {
        return Err(Error::TaskAdmission);
    }
    let mut information = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    if unsafe { libc::fstatvfs(file.as_raw_fd(), information.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let information = unsafe { information.assume_init() };
    if information.f_frsize == 0
        || information
            .f_blocks
            .checked_mul(information.f_frsize)
            .is_none_or(|total| total == 0 || total > maximum)
    {
        return Err(Error::TaskAdmission);
    }
    Ok(())
}
