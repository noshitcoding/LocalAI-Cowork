//! Bounded private owner result reception. These records prove local custody,
//! not current actor rights, result approval or authority to launch a process.
use super::*;
use cowork_domain::workspace_transfer as transfer;
use workspace_receive::{
    NativeObject, OwnedVerifiedTransfer, TransferBinding, VerifiedTransfer, WorkspaceReceiver,
    directory_identity,
};

const PARENT: &str = "managed-results";
// Each stage is bounded to a 512 MiB preallocated pack, plus bounded metadata.
// Partial and finished stages stay charged across restart; drop frees no slot.
const MAX_STAGES: usize = 8;
const MAX_RECORD_BYTES: u64 = transfer::MAX_MANIFEST_BYTES as u64 + 16384;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManagedResultStageBinding {
    pub plan_id: String,
    pub plan_sha256: String,
    pub run_id: String,
    pub attempt_id: String,
    pub input_manifest_sha256: String,
    pub receipt_sha256: String,
    pub attestation_sha256: String,
    pub native_origin_sha256: String,
}
impl ManagedResultStageBinding {
    fn validate(&self) -> Result<()> {
        if [&self.plan_id, &self.run_id, &self.attempt_id]
            .iter()
            .any(|value| {
                value.is_empty()
                    || value.len() > 128
                    || !value
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
            })
            || [
                &self.plan_sha256,
                &self.input_manifest_sha256,
                &self.receipt_sha256,
                &self.attestation_sha256,
                &self.native_origin_sha256,
            ]
            .iter()
            .any(|value| !transfer::valid_sha256(value))
        {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
    fn transfer(&self) -> TransferBinding {
        TransferBinding {
            plan_id: self.plan_id.clone(),
            plan_sha256: self.plan_sha256.clone(),
            recipient_id: format!("owner:{}", self.native_origin_sha256),
            recipient_generation: 1,
        }
    }
}

/// Private native journal expected independently from the controller Store.
/// Never include these identities in a renderer or managed-service response.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManagedResultStageRecord {
    format_version: u32,
    id: String,
    binding: ManagedResultStageBinding,
    manifest: WorkspaceTransferManifest,
    container: NativeObject,
    transfer: NativeObject,
}
impl ManagedResultStageRecord {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn manifest(&self) -> &WorkspaceTransferManifest {
        &self.manifest
    }
    pub fn binding(&self) -> &ManagedResultStageBinding {
        &self.binding
    }
    fn validate(&self) -> Result<()> {
        self.binding.validate()?;
        transfer::validate(&self.manifest)?;
        if self.format_version != 1 || self.id != stage_id(&self.binding, &self.manifest) {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
}

pub struct ManagedResultStage {
    record: ManagedResultStageRecord,
    receiver: Arc<WorkspaceReceiver>,
    // One receiver owner at a time, including other FileBroker instances.
    _lock: ResourceLock,
}
impl ManagedResultStage {
    pub fn record(&self) -> &ManagedResultStageRecord {
        &self.record
    }
    pub fn missing_chunks(&self) -> Result<Vec<ReadWorkspaceTransferChunk>> {
        self.receiver.missing_chunks()
    }
    pub fn accept_chunk(
        &self,
        chunk: &ReadWorkspaceTransferChunk,
        bytes: &[u8],
    ) -> Result<workspace_receive::ChunkReceipt> {
        self.receiver.accept_chunk(chunk, bytes)
    }
    pub fn verify(&self) -> Result<VerifiedManagedResultStage<'_>> {
        Ok(VerifiedManagedResultStage {
            stage: self,
            transfer: self.receiver.verify()?,
        })
    }
    pub fn verify_owned(self: Arc<Self>) -> Result<OwnedVerifiedManagedResultStage> {
        let transfer = self.receiver.clone().verify_owned()?;
        Ok(OwnedVerifiedManagedResultStage {
            stage: self,
            transfer,
        })
    }
}
pub struct OwnedVerifiedManagedResultStage {
    stage: Arc<ManagedResultStage>,
    transfer: OwnedVerifiedTransfer,
}
impl OwnedVerifiedManagedResultStage {
    pub fn record(&self) -> &ManagedResultStageRecord {
        &self.stage.record
    }
    pub fn read_chunk(&mut self, chunk: &ReadWorkspaceTransferChunk) -> Result<Vec<u8>> {
        self.transfer.read_chunk(chunk)
    }
}
pub struct VerifiedManagedResultStage<'a> {
    stage: &'a ManagedResultStage,
    transfer: VerifiedTransfer<'a>,
}
impl VerifiedManagedResultStage<'_> {
    pub fn record(&self) -> &ManagedResultStageRecord {
        &self.stage.record
    }
    pub fn read_chunk(&mut self, chunk: &ReadWorkspaceTransferChunk) -> Result<Vec<u8>> {
        self.transfer.read_chunk(chunk)
    }
}

fn stage_id(binding: &ManagedResultStageBinding, manifest: &WorkspaceTransferManifest) -> String {
    canonical_hash(
        &json!({"format":"owner-managed-result-stage/1","binding":binding,"manifest_sha256":manifest.sha256}),
    )
}
fn private_directory(parent: &Dir, name: &str) -> Result<()> {
    let builder = cap_std::fs::DirBuilder::new();
    #[cfg(unix)]
    let builder = {
        use cap_std::fs::DirBuilderExt;
        let mut builder = builder;
        builder.mode(0o700);
        builder
    };
    parent
        .create_dir_with(name, &builder)
        .map_err(storage_error)
}
fn sync(parent: &Dir) -> Result<()> {
    #[cfg(unix)]
    {
        sync_directory(parent)?;
    }
    let _ = parent;
    Ok(())
}
fn read_record(container: &Dir) -> Result<ManagedResultStageRecord> {
    let bytes = read_regular_limit(container, "record.json", MAX_RECORD_BYTES)?
        .ok_or(Error::UncertainEffect)?;
    let record: ManagedResultStageRecord = serde_json::from_slice(&bytes).map_err(storage_error)?;
    record.validate()?;
    Ok(record)
}
fn write_record(container: &Dir, record: &ManagedResultStageRecord) -> Result<()> {
    let bytes = serde_json::to_vec(record).map_err(storage_error)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(Error::Forbidden);
    }
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = container
        .open_with("record.json", &options)
        .map_err(storage_error)?
        .into_std();
    workspace_receive::file_identity(&file)?;
    file.write_all(&bytes).map_err(storage_error)?;
    file.sync_all().map_err(storage_error)?;
    sync(container)
}
fn require_container(container: &Dir, record: &ManagedResultStageRecord) -> Result<Dir> {
    if directory_identity(container)? != record.container || read_record(container)? != *record {
        return Err(Error::FileConflict);
    }
    let mut names = Vec::new();
    for entry in container.entries().map_err(storage_error)? {
        names.push(entry.map_err(storage_error)?.file_name());
    }
    if names.len() != 2
        || !names.iter().any(|n| n == "record.json")
        || !names.iter().any(|n| n == "transfer")
    {
        return Err(Error::FileConflict);
    }
    let transfer = open_directory(container, Path::new("transfer"))?;
    if directory_identity(&transfer)? != record.transfer {
        return Err(Error::FileConflict);
    }
    Ok(transfer)
}

impl FileBroker {
    fn result_stage_parent(&self) -> Result<Dir> {
        match self.registry_state.symlink_metadata(PARENT) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                private_directory(&self.registry_state, PARENT)?;
                sync(&self.registry_state)?;
            }
            Err(e) => return Err(storage_error(e)),
        }
        open_directory(&self.registry_state, Path::new(PARENT))
    }
    /// Caller resolves fresh Owner/content authority before and after receiving.
    /// Capacity is reserved by the durable directory before allocating its pack.
    /// A partial stage never gets adopted as a new complete receiver.
    pub fn create_managed_result_stage(
        &self,
        binding: ManagedResultStageBinding,
        manifest: WorkspaceTransferManifest,
    ) -> Result<ManagedResultStage> {
        binding.validate()?;
        transfer::validate(&manifest)?;
        let _catalog = self.lock_named("managed-result-stage-catalog", true)?;
        if self.local_controller_origin()? != binding.native_origin_sha256 {
            return Err(Error::Forbidden);
        }
        let parent = self.result_stage_parent()?;
        let id = stage_id(&binding, &manifest);
        // Enumerate every entry, including failed creates/orphans. Unknown or
        // replaced entries block new allocations instead of freeing capacity.
        let mut count = 0;
        let mut exists = false;
        for entry in parent.entries().map_err(storage_error)? {
            let name = entry
                .map_err(storage_error)?
                .file_name()
                .into_string()
                .map_err(|_| Error::Forbidden)?;
            if !transfer::valid_sha256(&name) {
                return Err(Error::FileConflict);
            }
            open_directory(&parent, Path::new(&name))?;
            count += 1;
            if name == id {
                exists = true;
            }
        }
        if exists {
            // Replay only a completed original preparation receipt at the exact
            // independently journaled CreateIntent path. Never repair metadata
            // or replace native identities. Once SQL has a native record, its
            // stricter reopen(expected) path must be used instead.
            let container = open_directory(&parent, Path::new(&id))?;
            let record = read_record(&container)?;
            if record.binding != binding || record.manifest != manifest || record.id != id {
                return Err(Error::Conflict);
            }
            require_container(&container, &record)?;
            let lock = self.lock_named(&format!("managed-result-stage-{id}"), true)?;
            let directory = require_container(&container, &record)?;
            let receiver = WorkspaceReceiver::reopen(directory, &binding.transfer(), &manifest)?;
            return Ok(ManagedResultStage {
                record,
                receiver: Arc::new(receiver),
                _lock: lock,
            });
        }
        if count >= MAX_STAGES {
            return Err(Error::Validation(
                "managed_result_stage_capacity_exceeded".into(),
            ));
        }
        private_directory(&parent, &id)?;
        sync(&parent)?;
        // The persistent stage slot is already charged. Rejected requests must
        // not create an unbounded number of otherwise unaccounted lock files.
        let lock = self.lock_named(&format!("managed-result-stage-{id}"), true)?;
        let container = open_directory(&parent, Path::new(&id))?;
        private_directory(&container, "transfer")?;
        sync(&container)?;
        let directory = open_directory(&container, Path::new("transfer"))?;
        let record = ManagedResultStageRecord {
            format_version: 1,
            id,
            binding: binding.clone(),
            manifest: manifest.clone(),
            container: directory_identity(&container)?,
            transfer: directory_identity(&directory)?,
        };
        let receiver = WorkspaceReceiver::create(directory, binding.transfer(), manifest)?;
        write_record(&container, &record)?;
        Ok(ManagedResultStage {
            record,
            receiver: Arc::new(receiver),
            _lock: lock,
        })
    }
    /// The expected native record comes from the existing controller journal,
    /// never from a client or by discovering this directory's own metadata.
    pub fn reopen_managed_result_stage(
        &self,
        expected: &ManagedResultStageRecord,
    ) -> Result<ManagedResultStage> {
        expected.validate()?;
        let _catalog = self.lock_named("managed-result-stage-catalog", true)?;
        if self.local_controller_origin()? != expected.binding.native_origin_sha256 {
            return Err(Error::Forbidden);
        }
        let parent = self.result_stage_parent()?;
        let container = open_directory(&parent, Path::new(&expected.id))?;
        require_container(&container, expected)?;
        let lock = self.lock_named(&format!("managed-result-stage-{}", expected.id), true)?;
        let directory = require_container(&container, expected)?;
        let receiver =
            WorkspaceReceiver::reopen(directory, &expected.binding.transfer(), &expected.manifest)?;
        Ok(ManagedResultStage {
            record: expected.clone(),
            receiver: Arc::new(receiver),
            _lock: lock,
        })
    }
}
