//! Private result publication through the existing Store, never direct adoption.
use super::*;
use cowork_broker::ManagedResultStage;
use cowork_domain::environments::EnvironmentObservation;
use cowork_storage::{ManagedResultVersion, ManagedResultVersionStatus};
#[path = "delivery.rs"]
mod delivery;
#[path = "delivery_view.rs"]
mod delivery_view;
#[path = "owner.rs"]
mod owner;

struct LocalReadyAuthority {
    context: ManagedResultContext,
    guard: Arc<ManagedResultReadGuard>,
    version: ManagedResultVersion,
}

impl Runtime {
    async fn local_ready_result_authority(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
    ) -> Result<Option<LocalReadyAuthority>> {
        Box::pin(self.local_ready_result_authority_inner(order, run, attempt)).await
    }
    async fn local_ready_result_authority_inner(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
    ) -> Result<Option<LocalReadyAuthority>> {
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        let origin = tokio::task::spawn_blocking(move || files.local_controller_origin())
            .await
            .map_err(storage_error)??;
        let Some((context, version)) = self
            .store
            .local_ready_managed_result_version(order, run, attempt, &origin)
            .await?
        else {
            return Ok(None);
        };
        let guard = self.result_native_guard(&context, &origin).await?;
        let authority = LocalReadyAuthority {
            context,
            guard,
            version,
        };
        self.recheck_local_ready_result_authority(&authority)
            .await?;
        Ok(Some(authority))
    }
    async fn recheck_local_ready_result_authority(
        &self,
        authority: &LocalReadyAuthority,
    ) -> Result<()> {
        Box::pin(self.recheck_local_ready_result_authority_inner(authority)).await
    }
    async fn recheck_local_ready_result_authority_inner(
        &self,
        authority: &LocalReadyAuthority,
    ) -> Result<()> {
        let context = &authority.context;
        self.recheck_result_native_guard(context, &authority.guard)
            .await?;
        let Some((fresh, version)) = self
            .store
            .local_ready_managed_result_version(
                &context.run.spec.order_id,
                &context.run.id,
                &context.input.request.payload.process_attempt_id,
                authority.guard.origin(),
            )
            .await?
        else {
            return Err(Error::NotFound);
        };
        if fresh.authorization_sha256 != context.authorization_sha256
            || version != authority.version
        {
            return Err(Error::ExecutionPlanChanged);
        }
        self.recheck_result_native_guard(context, &authority.guard)
            .await
    }
    async fn result_version_observation(
        &self,
        authority: &Authority,
    ) -> Result<EnvironmentObservation> {
        self.recheck_result_native(authority).await?;
        let run = &authority.context.run;
        let reference = run.spec.environment.as_ref().ok_or(Error::Forbidden)?;
        let (_, observation) = self
            .process_environment(&run.spec.project_id, reference)
            .await?;
        Ok(observation)
    }
    async fn retained_managed_version(
        &self,
        authority: &Authority,
    ) -> Result<Option<ManagedResultVersion>> {
        let context = &authority.context;
        let observation = self.result_version_observation(authority).await?;
        let version = self
            .store
            .managed_result_version(
                &context.run.spec.order_id,
                &context.run.id,
                &context.input.request.payload.process_attempt_id,
                &observation,
                authority.guard.origin(),
                &context.authorization_sha256,
            )
            .await?;
        self.recheck_result_authority(authority).await?;
        Ok(version)
    }
    async fn open_result_version_stage(
        &self,
        version: &ManagedResultVersion,
    ) -> Result<Arc<ManagedResultStage>> {
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        let version = version.clone();
        tokio::task::spawn_blocking(move || {
            // Once attached, the original independent native record always
            // takes precedence. No failed reopen falls back to CreateReplay.
            let stage = match &version.custody {
                Some(record) => files.reopen_managed_result_stage(record)?,
                None if version.state == ManagedResultVersionStatus::Receiving => files
                    .create_managed_result_stage(
                        version.stage_binding(),
                        version.result.manifest,
                    )?,
                None => return Err(Error::Conflict),
            };
            Ok(Arc::new(stage))
        })
        .await
        .map_err(storage_error)?
    }
    /// Trusted Owner entry. Returns a local version ID only after complete
    /// verification and SQL publication. It neither edits originals nor adopts.
    pub async fn retain_managed_result_version(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
    ) -> Result<String> {
        // Keep the composed Store/authority/transfer state off the caller's
        // normal Windows request/test stack. No larger thread stack required.
        Box::pin(self.retain_managed_result_version_inner(order, run, attempt)).await
    }
    async fn retain_managed_result_version_inner(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
    ) -> Result<String> {
        let _permit = self.begin_upload_transfer()?;
        if let Some(authority) = self
            .local_ready_result_authority(order, run, attempt)
            .await?
        {
            let stage = self.open_result_version_stage(&authority.version).await?;
            let verified = tokio::task::spawn_blocking(move || stage.verify_owned())
                .await
                .map_err(storage_error)??;
            self.recheck_local_ready_result_authority(&authority)
                .await?;
            drop(verified);
            return Ok(authority.version.id);
        }
        let authority = self.result_authority(order, run, attempt).await?;
        let mut version = match self.retained_managed_version(&authority).await? {
            Some(original) if original.state != ManagedResultVersionStatus::Retired => original,
            Some(_) => return Err(Error::NotFound),
            None => {
                // Read uses its own short-lived native/source guards. The fresh
                // Store transaction below resolves authorization again.
                let read_authority = self.result_authority(order, run, attempt).await?;
                let result = self.read_result_manifest_authorized(read_authority).await?;
                let observation = self.result_version_observation(&authority).await?;
                self.store
                    .prepare_managed_result_version(
                        order,
                        run,
                        attempt,
                        &result,
                        &result.attestation.payload.runtime_instance_sha256,
                        &observation,
                        authority.guard.origin(),
                        &authority.context.authorization_sha256,
                    )
                    .await?
            }
        };
        let stage = self.open_result_version_stage(&version).await?;
        self.recheck_result_authority(&authority).await?;
        if version.custody.is_none() {
            let observation = self.result_version_observation(&authority).await?;
            version = self
                .store
                .attach_managed_result_custody(
                    &version.id,
                    stage.record(),
                    order,
                    run,
                    attempt,
                    &observation,
                    authority.guard.origin(),
                    &authority.context.authorization_sha256,
                )
                .await?;
        }
        // Release source revocation/copy locks between remote chunk reads. The
        // native receiver lock remains held, but it never blocks source revoke.
        drop(authority);
        if version.state == ManagedResultVersionStatus::Receiving {
            let receiver = stage.clone();
            let missing = tokio::task::spawn_blocking(move || receiver.missing_chunks())
                .await
                .map_err(storage_error)??;
            for chunk in missing {
                let authority = self.result_authority(order, run, attempt).await?;
                let bytes = self
                    .read_result_chunk_authorized(authority, &version.result, chunk.clone())
                    .await?;
                let receiver = stage.clone();
                tokio::task::spawn_blocking(move || receiver.accept_chunk(&chunk, &bytes))
                    .await
                    .map_err(storage_error)??;
                let current = self.result_authority(order, run, attempt).await?;
                self.recheck_result_authority(&current).await?;
            }
        }
        let receiver = stage.clone();
        let verified = tokio::task::spawn_blocking(move || receiver.verify_owned())
            .await
            .map_err(storage_error)??;
        let authority = self.result_authority(order, run, attempt).await?;
        let observation = self.result_version_observation(&authority).await?;
        let ready = self
            .store
            .publish_managed_result_version(
                &version.id,
                &verified,
                order,
                run,
                attempt,
                &observation,
                authority.guard.origin(),
                &authority.context.authorization_sha256,
            )
            .await?;
        self.recheck_result_authority(&authority).await?;
        Ok(ready.id)
    }
    /// Read one declared block of a completely verified local result version.
    /// Native service retention is historical acquisition evidence here; it
    /// does not grant a remote read, require a live runner or revive a lease.
    pub async fn managed_result_version_chunk(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
        version_id: &str,
        chunk: ReadWorkspaceTransferChunk,
    ) -> Result<Vec<u8>> {
        Box::pin(self.managed_result_version_chunk_inner(order, run, attempt, version_id, chunk))
            .await
    }
    async fn managed_result_version_chunk_inner(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
        version_id: &str,
        chunk: ReadWorkspaceTransferChunk,
    ) -> Result<Vec<u8>> {
        let _permit = self.begin_upload_transfer()?;
        let authority = self
            .local_ready_result_authority(order, run, attempt)
            .await?
            .ok_or(Error::NotFound)?;
        if authority.version.id != version_id {
            return Err(Error::NotFound);
        }
        let stage = self.open_result_version_stage(&authority.version).await?;
        let bytes = tokio::task::spawn_blocking(move || {
            let mut verified = stage.verify_owned()?;
            verified.read_chunk(&chunk)
        })
        .await
        .map_err(storage_error)??;
        self.recheck_local_ready_result_authority(&authority)
            .await?;
        Ok(bytes)
    }
}
