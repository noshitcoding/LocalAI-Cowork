//! Explicit confirmed-plan/snapshot grant for the actual recipient. A launch-V1
//! workspace identifier, path, digest or synchronized content grants no input
//! authority. The app/controller must issue this only after plan confirmation.
use super::*;
use crate::volumes::PublishedVolume;
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use cowork_broker::workspace_receive::{ReceivedWorkspace, TransferBinding};
use cowork_contracts::ExecutionTargetRef;
use cowork_runner_protocol::{Binding, Broker, DispatchedLaunch, Signed};
use std::os::fd::AsRawFd;
use std::time::Instant;

pub use cowork_runner_protocol::ManagedInputGrant;
/// Private capability, not serializable or cloneable. Its signature and actual
/// local expected target are checked independently of the received journal.
pub struct VerifiedInputGrant {
    grant: ManagedInputGrant,
}
pub struct VerifiedTaskInput {
    origin: Broker,
    deadline: Instant,
    grant: ManagedInputGrant,
    volume: PublishedVolume,
    workspace: Dir,
    temporary: Dir,
    workspace_id: (u64, u64),
    temporary_id: (u64, u64),
}
impl VerifiedInputGrant {
    pub fn verify(
        signed: Signed<ManagedInputGrant>,
        binding: &Binding,
        current_target: &ExecutionTargetRef,
        now: i64,
    ) -> Result<Self> {
        signed
            .verify("managed_input_grant", &binding.control_public_key)
            .map_err(|_| Error::Input)?;
        let grant = signed.payload;
        grant
            .validate_for_runner(binding, current_target, now)
            .map_err(|_| Error::Input)?;
        Ok(Self { grant })
    }
    pub fn transfer_binding(&self) -> TransferBinding {
        TransferBinding {
            plan_id: self.grant.execution_plan_id.clone(),
            plan_sha256: self.grant.execution_plan_sha256.clone(),
            recipient_id: self.grant.runner.runner_id.clone(),
            recipient_generation: self.grant.runner.generation,
        }
    }
    pub fn manifest_sha256(&self) -> &str {
        &self.grant.input_manifest_sha256
    }
    /// Materialization must already have verified the full manifest. The native
    /// completed tree must be precisely the subordinate workspace in this exact
    /// published reserved volume. No original or foreign tree can be attached.
    pub(super) fn bind_received(
        self,
        received: ReceivedWorkspace,
        volume: PublishedVolume,
        origin: Broker,
        deadline: Instant,
        now: i64,
    ) -> Result<VerifiedTaskInput> {
        if Instant::now() >= deadline
            || now < self.grant.issued_at
            || now >= self.grant.expires_at
            || received.manifest_sha256() != self.grant.input_manifest_sha256
            || volume.request_sha256() != self.grant.request_sha256
        {
            return Err(Error::Input);
        }
        volume.verify_published().map_err(|_| Error::Input)?;
        let data = volume.data_directory().map_err(|_| Error::Input)?;
        let expected = data.open_dir_nofollow("workspace")?;
        let workspace = received.into_directory();
        let workspace_id = identity(&workspace)?;
        if identity(&expected)? != workspace_id {
            return Err(Error::Input);
        }
        let temporary = data.open_dir_nofollow("tmp")?;
        if temporary.entries()?.next().is_some() {
            return Err(Error::Input);
        }
        let temporary_id = identity(&temporary)?;
        // Receiver-created files are root-owned private bytes; convert only the
        // verified workspace, not transfer staging, control records or journals.
        make_workload_owned(&workspace)?;
        make_workload_owned(&temporary)?;
        Ok(VerifiedTaskInput {
            origin,
            deadline,
            grant: self.grant,
            volume,
            workspace,
            temporary,
            workspace_id,
            temporary_id,
        })
    }
}
impl VerifiedTaskInput {
    /// Called only after the task's actual completed outcome was rechecked.
    /// Closing both subordinate directory handles precedes any volume unmount.
    pub(super) fn into_retirement_volume(self) -> PublishedVolume {
        let Self {
            volume,
            workspace,
            temporary,
            ..
        } = self;
        drop(workspace);
        drop(temporary);
        volume
    }
    pub(super) fn producer_instance_sha256(&self) -> String {
        self.origin.instance_fingerprint()
    }
    pub(super) fn initial_deadline(&self) -> Instant {
        self.deadline
    }
    pub(super) fn grant(&self) -> &ManagedInputGrant {
        &self.grant
    }
    pub(super) fn workspace(&self) -> &Dir {
        &self.workspace
    }
    pub(super) fn temporary(&self) -> &Dir {
        &self.temporary
    }
    pub(super) fn volume(&self) -> &PublishedVolume {
        &self.volume
    }
    pub fn validate_dispatch(
        &self,
        launch: &DispatchedLaunch,
        broker: &Broker,
        binding: &Binding,
        current_target: &ExecutionTargetRef,
        now: i64,
    ) -> Result<()> {
        if !self.origin.is_same_instance(broker)
            || !broker.is_bound_to(binding)
            || Instant::now() >= self.deadline
        {
            return Err(Error::Input);
        }
        broker
            .validate_owned_dispatch(launch)
            .map_err(|_| Error::Input)?;
        self.grant
            .validate_for_launch(binding, current_target, launch.request(), now)
            .map_err(|_| Error::Input)?;
        self.verify()
    }
    pub(super) fn verify(&self) -> Result<()> {
        self.volume.verify_published().map_err(|_| Error::Input)?;
        let data = self.volume.data_directory().map_err(|_| Error::Input)?;
        if identity(&self.workspace)? != self.workspace_id
            || identity(&data.open_dir_nofollow("workspace")?)? != self.workspace_id
            || identity(&self.temporary)? != self.temporary_id
            || identity(&data.open_dir_nofollow("tmp")?)? != self.temporary_id
        {
            return Err(Error::Input);
        }
        Ok(())
    }
}
fn identity(dir: &Dir) -> Result<(u64, u64)> {
    let m = dir.open(".")?.into_std().metadata()?;
    if !m.is_dir() {
        return Err(Error::Input);
    }
    Ok((m.dev(), m.ino()))
}
fn make_workload_owned(root: &Dir) -> Result<()> {
    let mut pending = vec![String::new()];
    let mut entries = 0;
    while let Some(relative) = pending.pop() {
        let mut dir = root.try_clone()?;
        if !relative.is_empty() {
            for part in relative.split('/') {
                dir = dir.open_dir_nofollow(part)?;
            }
        }
        for entry in dir.entries()? {
            entries += 1;
            if entries > 32768 {
                return Err(Error::Input);
            }
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_str().ok_or(Error::Input)?;
            let path = if relative.is_empty() {
                name.into()
            } else {
                format!("{relative}/{name}")
            };
            if entry.file_type()?.is_dir() {
                pending.push(path);
            } else if entry.file_type()?.is_file() {
                let mut options = cap_std::fs::OpenOptions::new();
                options.read(true);
                use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
                options.follow(FollowSymlinks::No);
                let file = dir.open_with(name, &options)?.into_std();
                let m = file.metadata()?;
                if !m.is_file() || m.nlink() != 1 || m.uid() != 0 {
                    return Err(Error::Input);
                }
                if unsafe { libc::fchown(file.as_raw_fd(), 1000, 1000) } < 0
                    || unsafe { libc::fchmod(file.as_raw_fd(), 0o600) } < 0
                {
                    return Err(io::Error::last_os_error().into());
                }
                file.sync_all()?;
            } else {
                return Err(Error::Input);
            }
        }
        let file = dir.open(".")?.into_std();
        if file.metadata()?.uid() != 0 {
            return Err(Error::Input);
        }
        if unsafe { libc::fchown(file.as_raw_fd(), 1000, 1000) } < 0
            || unsafe { libc::fchmod(file.as_raw_fd(), 0o700) } < 0
        {
            return Err(io::Error::last_os_error().into());
        }
        file.sync_all()?;
    }
    Ok(())
}
