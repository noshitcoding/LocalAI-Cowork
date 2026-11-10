//! Snapshot only a previously confirmed native working copy. Source locks keep
//! current resource rights live while its immutable transfer bytes are retained.
use super::*;

pub struct PreparedManagedInput {
    pub snapshot: WorkspaceSnapshot,
    pub manifest: WorkspaceTransferManifest,
    _copy: PreparedSnapshot,
    _sources: Vec<ResourceLock>,
}

/// Holds actual source revocation and copy-namespace locks while a remote
/// result read is authorized, received and rechecked. Never portable authority.
pub struct ManagedResultReadGuard {
    origin: String,
    context_sha256: String,
    _sources: Vec<ResourceLock>,
    _copy: ResourceLock,
}
impl ManagedResultReadGuard {
    pub fn origin(&self) -> &str {
        &self.origin
    }
}

fn result_context(custody: &RunWorkspaceRecord, run: &RunRecord, source: &RunSpec) -> String {
    canonical_hash(&json!({"custody":custody,"spec":run.spec,"run_id":run.id,"source":source}))
}

impl FileBroker {
    pub fn prepare_managed_result_read(
        &self,
        custody: &RunWorkspaceRecord,
        run: &RunRecord,
        source: &RunSpec,
    ) -> Result<ManagedResultReadGuard> {
        if source.project_id != run.spec.project_id || source.order_id != run.spec.order_id {
            return Err(Error::Forbidden);
        }
        let sources = self.managed_source_locks(source)?;
        self.restore_run_workspace_record(custody, &run.spec)?;
        let copy = self.lock_named(
            &format!("run-workspace-data-{}", custody.intent.binding.id),
            false,
        )?;
        self.restore_run_workspace_record(custody, &run.spec)?;
        self.validate_spec_grants(&run.spec)?;
        Ok(ManagedResultReadGuard {
            origin: self.local_controller_origin()?,
            context_sha256: result_context(custody, run, source),
            _sources: sources,
            _copy: copy,
        })
    }

    pub fn revalidate_managed_result_read(
        &self,
        guard: &ManagedResultReadGuard,
        custody: &RunWorkspaceRecord,
        run: &RunRecord,
        source: &RunSpec,
    ) -> Result<()> {
        if guard.origin != self.local_controller_origin()?
            || guard.context_sha256 != result_context(custody, run, source)
        {
            return Err(Error::Forbidden);
        }
        self.validate_spec_grants(source)?;
        self.restore_run_workspace_record(custody, &run.spec)?;
        self.validate_spec_grants(&run.spec)
    }
    pub fn validate_managed_input_sources(
        &self,
        custody: &RunWorkspaceRecord,
        run: &RunRecord,
        source: &RunSpec,
    ) -> Result<()> {
        if source.project_id != run.spec.project_id || source.order_id != run.spec.order_id {
            return Err(Error::Forbidden);
        }
        let _sources = self.managed_source_locks(source)?;
        self.restore_run_workspace_record(custody, &run.spec)?;
        self.validate_spec_grants(&run.spec)
    }

    fn managed_source_locks(&self, source: &RunSpec) -> Result<Vec<ResourceLock>> {
        if source.input_workspace.is_some() {
            return Err(Error::Forbidden);
        }
        self.validate_spec_grants(source)?;
        let mut roots = source
            .folders
            .iter()
            .map(|grant| self.root_for_spec(source, &grant.id, false))
            .collect::<Result<Vec<_>>>()?;
        roots.sort_by(|a, b| a.lock_name.cmp(&b.lock_name));
        roots.dedup_by(|a, b| a.lock_name == b.lock_name);
        roots.iter().map(|root| self.lock(root, false)).collect()
    }

    pub fn prepare_managed_input(
        &self,
        custody: &RunWorkspaceRecord,
        run: &RunRecord,
        source: &RunSpec,
    ) -> Result<PreparedManagedInput> {
        if source.project_id != run.spec.project_id || source.order_id != run.spec.order_id {
            return Err(Error::Forbidden);
        }
        let sources = self.managed_source_locks(source)?;
        self.restore_run_workspace_record(custody, &run.spec)?;
        self.validate_spec_grants(&run.spec)?;
        let copy = self.snapshot_workspace(run)?;
        let manifest = self.workspace_transfer_manifest(&copy.snapshot)?;
        Ok(PreparedManagedInput {
            snapshot: copy.snapshot.clone(),
            manifest,
            _copy: copy,
            _sources: sources,
        })
    }

    pub fn managed_input_chunk(
        &self,
        custody: &RunWorkspaceRecord,
        run: &RunRecord,
        source: &RunSpec,
        snapshot: &WorkspaceSnapshot,
        input: &ReadWorkspaceTransferChunk,
    ) -> Result<BinaryFile> {
        if snapshot.run_id != run.id
            || snapshot.order_id != run.spec.order_id
            || source.project_id != run.spec.project_id
            || source.order_id != run.spec.order_id
        {
            return Err(Error::Forbidden);
        }
        let _sources = self.managed_source_locks(source)?;
        self.restore_run_workspace_record(custody, &run.spec)?;
        self.validate_spec_grants(&run.spec)?;
        // Read retained snapshot bytes. Neither mutable copy bytes nor originals
        // are a fallback if the retained block is missing or changed.
        self.workspace_transfer_chunk(snapshot, input)
    }
}
