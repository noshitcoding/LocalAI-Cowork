//! Native incoming snapshot boundary. Only fresh common-Broker input permits
//! and the exact controller-signed plan grant may write/materialize a snapshot.
//! No API input can submit a host path, directory identity or dispatch permit.
use super::*;
use super::{
    input::{VerifiedInputGrant, VerifiedTaskInput},
    profile::ManagedProfile,
};
use crate::volumes::{MountedVolume, VolumeRequest};
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use cowork_broker::workspace_receive::{ChunkReceipt, WorkspaceReceiver};
use cowork_contracts::{ReadWorkspaceTransferChunk, WorkspaceTransferManifest};
use cowork_runner_protocol::{
    Broker, InputTransferPermit, LaunchRequest, ManagedInputGrant, Signed,
};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::time::Instant;

pub struct IncomingManagedInput {
    origin: Broker,
    deadline: Instant,
    profile: Arc<ManagedProfile>,
    request: LaunchRequest,
    grant: Signed<ManagedInputGrant>,
    manifest: WorkspaceTransferManifest,
    receiver: WorkspaceReceiver,
    workspace: Dir,
    volume: MountedVolume,
}
impl IncomingManagedInput {
    pub async fn create(
        profile: Arc<ManagedProfile>,
        broker: &Broker,
        permit: InputTransferPermit,
        grant: Signed<ManagedInputGrant>,
        manifest: WorkspaceTransferManifest,
        volume: MountedVolume,
    ) -> Result<Self> {
        let request = authorize(&profile, broker, permit, &grant, &manifest).await?;
        let deadline = initial_deadline(broker, &request, &grant)?;
        storage(&profile, &request, &volume)?;
        let entries = manifest
            .files
            .len()
            .checked_add(manifest.directories.len())
            .ok_or(Error::Input)?;
        let needed = manifest
            .total_bytes
            .checked_mul(2)
            .and_then(|v| v.checked_add(1024 * 1024))
            .ok_or(Error::Input)?;
        let usage = volume.usage().map_err(|_| Error::Input)?;
        if entries
            .checked_add(32)
            .is_none_or(|n| n > profile.maximum_files as usize)
            || needed > usage.available_bytes
        {
            return Err(Error::Input);
        }
        custody(&volume, broker, &request, &grant, &manifest).create(volume.directory())?;
        let stage = private_directory(volume.directory(), "transfer")?;
        let work = volume.data_directory().map_err(|_| Error::Input)?;
        let workspace = private_directory(&work, "workspace")?;
        private_directory(&work, "tmp")?;
        let verified =
            VerifiedInputGrant::verify(grant.clone(), &profile.binding, &profile.target, wall()?)?;
        let receiver =
            WorkspaceReceiver::create(stage, verified.transfer_binding(), manifest.clone())
                .map_err(|_| Error::Input)?;
        Ok(Self {
            origin: broker.clone(),
            deadline,
            profile,
            request,
            grant,
            manifest,
            receiver,
            workspace,
            volume,
        })
    }
    /// Reopen only an unpublished, exactly retained transfer. A partially
    /// materialized workspace or published/uncertain reservation is not adopted.
    pub async fn reopen(
        profile: Arc<ManagedProfile>,
        broker: &Broker,
        permit: InputTransferPermit,
        grant: Signed<ManagedInputGrant>,
        manifest: WorkspaceTransferManifest,
        volume: MountedVolume,
    ) -> Result<Self> {
        let request = authorize(&profile, broker, permit, &grant, &manifest).await?;
        let deadline = initial_deadline(broker, &request, &grant)?;
        storage(&profile, &request, &volume)?;
        custody(&volume, broker, &request, &grant, &manifest).verify(volume.directory())?;
        let work = volume.data_directory().map_err(|_| Error::Input)?;
        let workspace = work.open_dir_nofollow("workspace")?;
        if workspace.entries()?.next().is_some()
            || work.open_dir_nofollow("tmp")?.entries()?.next().is_some()
        {
            return Err(Error::Input);
        }
        let verified =
            VerifiedInputGrant::verify(grant.clone(), &profile.binding, &profile.target, wall()?)?;
        let receiver = WorkspaceReceiver::reopen(
            volume.directory().open_dir_nofollow("transfer")?,
            &verified.transfer_binding(),
            &manifest,
        )
        .map_err(|_| Error::Input)?;
        Ok(Self {
            origin: broker.clone(),
            deadline,
            profile,
            request,
            grant,
            manifest,
            receiver,
            workspace,
            volume,
        })
    }
    pub async fn missing_chunks(
        &self,
        broker: &Broker,
        permit: InputTransferPermit,
    ) -> Result<Vec<ReadWorkspaceTransferChunk>> {
        self.current(broker, permit)
            .await
            .map_err(|error| input_stage("missing_current", error))?;
        self.receiver
            .missing_chunks()
            .map_err(|error| input_stage("missing_snapshot", error))
    }
    pub async fn accept_chunk(
        &self,
        broker: &Broker,
        permit: InputTransferPermit,
        input: &ReadWorkspaceTransferChunk,
        bytes: &[u8],
    ) -> Result<ChunkReceipt> {
        self.current(broker, permit).await?;
        self.receiver
            .accept_chunk(input, bytes)
            .map_err(|_| Error::Input)
    }
    /// The caller may only now admit/dispatch through the existing common
    /// Broker. This capability is neither serializable nor cloneable.
    pub async fn seal(
        self,
        broker: &Broker,
        permit: InputTransferPermit,
    ) -> Result<VerifiedTaskInput> {
        self.current(broker, permit)
            .await
            .map_err(|error| input_stage("seal_current_authority", error))?;
        let verified = VerifiedInputGrant::verify(
            self.grant,
            &self.profile.binding,
            &self.profile.target,
            wall()?,
        )
        .map_err(|error| input_stage("seal_grant_verify", error))?;
        let received = self
            .receiver
            .verify()
            .map_err(|error| input_stage("seal_snapshot_verify", error))?
            .materialize(self.workspace)
            .map_err(|error| input_stage("seal_snapshot_materialize", error))?;
        verified
            .bind_received(
                received,
                self.volume
                    .publish_for_dispatch()
                    .map_err(|error| input_stage("seal_volume_publish", error))?,
                self.origin,
                self.deadline,
                wall()?,
            )
            .map_err(|error| input_stage("seal_native_bind", error))
    }
    pub fn reserved_volume_id(&self) -> &str {
        self.volume.id()
    }
    async fn current(&self, broker: &Broker, permit: InputTransferPermit) -> Result<()> {
        if !self.origin.is_same_instance(broker) || Instant::now() >= self.deadline {
            return Err(Error::Input);
        }
        let request = authorize(&self.profile, broker, permit, &self.grant, &self.manifest).await?;
        if request != self.request || Instant::now() >= self.deadline {
            return Err(Error::Input);
        }
        custody(&self.volume, broker, &request, &self.grant, &self.manifest)
            .verify(self.volume.directory())
            .map_err(|error| input_stage("current_custody", error))?;
        storage(&self.profile, &request, &self.volume)
    }
}
fn input_stage(stage: &'static str, error: impl std::fmt::Debug) -> Error {
    Error::InputStage {
        stage,
        detail: format!("{error:?}").chars().take(1024).collect(),
    }
}
// Root-private local custody only. A reopened Broker (even with identical
// keys, binding and portable bytes) cannot adopt an earlier transfer. The
// monotonic epoch itself stays in the original process and is never serialized.
#[derive(Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct InputCustody {
    format_version: u32,
    broker_instance_sha256: String,
    launch_sha256: String,
    grant_sha256: String,
    manifest_sha256: String,
    volume_id: String,
}
impl InputCustody {
    fn create(&self, root: &Dir) -> Result<()> {
        use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
        use cap_std::fs::OpenOptionsExt;
        let mut options = cap_std::fs::OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .mode(0o600)
            .follow(FollowSymlinks::No);
        let mut file = root.open_with("input-custody.json", &options)?.into_std();
        let bytes = serde_json::to_vec(self).map_err(|_| Error::Input)?;
        if bytes.len() > 8192 {
            return Err(Error::Input);
        }
        file.write_all(&bytes)?;
        file.sync_all()?;
        root.open(".")?.into_std().sync_all()?;
        self.verify(root)
    }
    fn verify(&self, root: &Dir) -> Result<()> {
        use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
        let mut options = cap_std::fs::OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let file = root.open_with("input-custody.json", &options)?.into_std();
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != 0
            || metadata.nlink() != 1
            || metadata.mode() & 0o777 != 0o600
            || metadata.len() > 8192
        {
            return Err(Error::Input);
        }
        let mut bytes = Vec::new();
        file.take(8193).read_to_end(&mut bytes)?;
        if bytes.len() > 8192
            || serde_json::from_slice::<Self>(&bytes).map_err(|_| Error::Input)? != *self
        {
            return Err(Error::Input);
        }
        Ok(())
    }
}
fn custody(
    volume: &MountedVolume,
    broker: &Broker,
    request: &LaunchRequest,
    grant: &Signed<ManagedInputGrant>,
    manifest: &WorkspaceTransferManifest,
) -> InputCustody {
    InputCustody {
        format_version: 1,
        broker_instance_sha256: broker.instance_fingerprint(),
        launch_sha256: cowork_domain::canonical_hash(request),
        grant_sha256: cowork_domain::canonical_hash(grant),
        manifest_sha256: manifest.sha256.clone(),
        volume_id: volume.id().to_owned(),
    }
}
fn initial_deadline(
    broker: &Broker,
    request: &LaunchRequest,
    grant: &Signed<ManagedInputGrant>,
) -> Result<Instant> {
    let deadline = broker
        .initial_input_deadline(request, grant.payload.expires_at)
        .map_err(|_| Error::Input)?;
    if Instant::now() >= deadline {
        return Err(Error::Input);
    }
    Ok(deadline)
}
fn storage(
    profile: &ManagedProfile,
    request: &LaunchRequest,
    volume: &MountedVolume,
) -> Result<()> {
    volume
        .require_request(&VolumeRequest {
            request_sha256: request.request_sha256.clone(),
            bytes: profile.definition.limits.scratch_bytes,
            inodes: profile.maximum_files,
        })
        .map_err(|error| input_stage("storage_request", error))
}
async fn authorize(
    profile: &ManagedProfile,
    broker: &Broker,
    permit: InputTransferPermit,
    grant: &Signed<ManagedInputGrant>,
    manifest: &WorkspaceTransferManifest,
) -> Result<LaunchRequest> {
    if !broker.is_bound_to(&profile.binding) {
        return Err(Error::Input);
    }
    let request = broker
        .consume_input_transfer(permit)
        .await
        .map_err(|error| input_stage("authorize_permit", error))?;
    profile.verify()?;
    grant
        .verify("managed_input_grant", &profile.binding.control_public_key)
        .map_err(|error| input_stage("authorize_grant_signature", error))?;
    grant
        .payload
        .validate_for_launch(&profile.binding, &profile.target, &request, wall()?)
        .map_err(|error| input_stage("authorize_grant_binding", error))?;
    cowork_domain::workspace_transfer::validate(manifest).map_err(|_| Error::Input)?;
    let program = profile
        .definition
        .programs
        .get(&request.invocation.program)
        .ok_or(Error::Input)?;
    profile.rootfs.executable(program)?;
    if grant.payload.input_manifest_sha256 != manifest.sha256
        || request.environment != profile.reference
    {
        return Err(Error::Input);
    }
    Ok(request)
}
fn private_directory(parent: &Dir, name: &str) -> Result<Dir> {
    parent.create_dir(name)?;
    parent.set_permissions(
        name,
        cap_std::fs::Permissions::from_std(std::fs::Permissions::from_mode(0o700)),
    )?;
    Ok(parent.open_dir_nofollow(name)?)
}
pub(super) fn wall() -> Result<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| Error::Input)?
        .as_millis()
        .try_into()
        .map_err(|_| Error::Input)
}
