//! Public Owner projections over the existing local reception journal.
use super::*;
use cowork_storage::{ManagedResultVersion, ManagedResultVersionStatus};

fn entry(
    context: &ManagedResultContext,
    version: Option<&ManagedResultVersion>,
) -> Result<OwnerManagedResultEntry> {
    Ok(OwnerManagedResultEntry {
        order_id: context.run.spec.order_id.clone(),
        run_id: context.run.id.clone(),
        process_attempt_id: context.input.request.payload.process_attempt_id.clone(),
        version: version
            .map(|v| {
                Ok(OwnerManagedResultVersion {
                    id: v.id.clone(),
                    revision: v.revision,
                    state: match v.state {
                        ManagedResultVersionStatus::Receiving => {
                            OwnerManagedResultVersionState::Receiving
                        }
                        ManagedResultVersionStatus::Ready => OwnerManagedResultVersionState::Ready,
                        ManagedResultVersionStatus::Retired => return Err(Error::NotFound),
                    },
                    created_at: v.created_at,
                    manifest_sha256: v.result.manifest.sha256.clone(),
                    file_count: v.result.manifest.files.len() as u64,
                    total_bytes: v.result.manifest.total_bytes,
                })
            })
            .transpose()?,
    })
}

impl Runtime {
    async fn owner_result_origin(&self) -> Result<String> {
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        tokio::task::spawn_blocking(move || files.local_controller_origin())
            .await
            .map_err(storage_error)?
    }

    /// Current local Owner/content rights, with no runner call or allocation.
    pub async fn owner_managed_result_entry(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
    ) -> Result<OwnerManagedResultEntry> {
        Box::pin(async move {
            let _permit = self.begin_upload_transfer()?;
            let origin = self.owner_result_origin().await?;
            let (context, version) = self
                .store
                .local_managed_result_metadata(order, run, attempt, &origin)
                .await?;
            let guard = self.result_native_guard(&context, &origin).await?;
            self.recheck_result_native_guard(&context, &guard).await?;
            let (fresh, current) = self
                .store
                .local_managed_result_metadata(order, run, attempt, &origin)
                .await?;
            if fresh.authorization_sha256 != context.authorization_sha256 || current != version {
                return Err(Error::ExecutionPlanChanged);
            }
            self.recheck_result_native_guard(&context, &guard).await?;
            entry(&context, version.as_ref())
        })
        .await
    }

    pub async fn owner_managed_results(
        &self,
        order: &str,
        run: &str,
    ) -> Result<OwnerManagedResultList> {
        Box::pin(async move {
            let _permit = self.begin_upload_transfer()?;
            let origin = self.owner_result_origin().await?;
            let candidates = self
                .store
                .local_managed_result_candidates(order, run, &origin)
                .await?;
            // Keep the actual source/copy guards alive across the final SQL
            // snapshot. A cancellation or changed journal fails the whole list.
            let mut guards = Vec::with_capacity(candidates.len());
            for (context, _) in &candidates {
                guards.push(self.result_native_guard(context, &origin).await?);
            }
            let fresh = self
                .store
                .local_managed_result_candidates(order, run, &origin)
                .await?;
            if fresh.len() != candidates.len() {
                return Err(Error::ExecutionPlanChanged);
            }
            let mut entries = Vec::with_capacity(candidates.len());
            for (((context, version), (current, current_version)), guard) in
                candidates.iter().zip(&fresh).zip(&guards)
            {
                if context.authorization_sha256 != current.authorization_sha256
                    || version != current_version
                {
                    return Err(Error::ExecutionPlanChanged);
                }
                self.recheck_result_native_guard(context, guard).await?;
                entries.push(entry(context, version.as_ref())?);
            }
            Ok(OwnerManagedResultList { entries })
        })
        .await
    }

    /// Only this explicit action can acquire/resume remote output. Ready replay
    /// still uses the original local version and never repairs via remote GET.
    pub async fn receive_owner_managed_result(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
    ) -> Result<OwnerManagedResultEntry> {
        Box::pin(async move {
            let id = self
                .retain_managed_result_version(order, run, attempt)
                .await?;
            let value = self.owner_managed_result_entry(order, run, attempt).await?;
            if !value
                .version
                .as_ref()
                .is_some_and(|v| v.id == id && v.state == OwnerManagedResultVersionState::Ready)
            {
                return Err(Error::ExecutionPlanChanged);
            }
            Ok(value)
        })
        .await
    }

    async fn owner_result_attempt(&self, order: &str, run: &str, id: &str) -> Result<String> {
        let origin = self.owner_result_origin().await?;
        let (context, _) = self
            .store
            .local_ready_managed_result_version_by_id(order, run, id, &origin)
            .await?;
        Ok(context.input.request.payload.process_attempt_id)
    }

    pub async fn owner_managed_result_manifest(
        &self,
        order: &str,
        run: &str,
        id: &str,
    ) -> Result<OwnerManagedResultManifest> {
        Box::pin(async move {
            let _permit = self.begin_upload_transfer()?;
            let attempt = self.owner_result_attempt(order, run, id).await?;
            let authority = self
                .local_ready_result_authority(order, run, &attempt)
                .await?
                .ok_or(Error::NotFound)?;
            if authority.version.id != id {
                return Err(Error::NotFound);
            }
            let stage = self.open_result_version_stage(&authority.version).await?;
            let verified = tokio::task::spawn_blocking(move || stage.verify_owned())
                .await
                .map_err(storage_error)??;
            self.recheck_local_ready_result_authority(&authority)
                .await?;
            let result = OwnerManagedResultManifest {
                entry: entry(&authority.context, Some(&authority.version))?,
                manifest: authority.version.result.manifest.clone(),
            };
            drop(verified);
            Ok(result)
        })
        .await
    }

    pub async fn owner_managed_result_chunk(
        &self,
        order: &str,
        run: &str,
        id: &str,
        chunk: ReadWorkspaceTransferChunk,
    ) -> Result<cowork_broker::BinaryFile> {
        Box::pin(async move {
            let attempt = self.owner_result_attempt(order, run, id).await?;
            let file_name = chunk
                .path
                .rsplit('/')
                .next()
                .ok_or(Error::Forbidden)?
                .to_owned();
            let bytes = self
                .managed_result_version_chunk(order, run, &attempt, id, chunk)
                .await?;
            Ok(cowork_broker::BinaryFile {
                metadata: DownloadMetadata {
                    file_name,
                    size: bytes.len() as u64,
                    sha256: hash(&bytes),
                },
                bytes,
            })
        })
        .await
    }
}
