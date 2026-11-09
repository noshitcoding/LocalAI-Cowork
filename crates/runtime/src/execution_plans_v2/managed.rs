//! Controller-owned resolution. A renderer selects a destination; it cannot
//! supply a qualification, active registry revision, private key or host path.
use super::*;
use cowork_domain::environments::{EnvironmentArtifact, EnvironmentNetwork};
use cowork_runner_protocol::{LaunchRequest, ManagedInputGrant, Signed};

/// Portable response for the installed transport. No local custody records,
/// original folder grants, access tokens or host paths cross this boundary.
#[derive(Clone)]
pub struct ManagedInputManifest {
    pub grant: Signed<ManagedInputGrant>,
    pub manifest: WorkspaceTransferManifest,
}

#[async_trait::async_trait]
pub trait ManagedInputAccess: Send + Sync {
    async fn manifest(&self) -> Result<ManagedInputManifest>;
    async fn chunk(&self, input: ReadWorkspaceTransferChunk) -> Result<cowork_broker::BinaryFile>;
}

struct InputSource {
    runtime: Runtime,
    project: String,
    run: String,
    attempt: String,
    token: String,
    environment: ExecutionEnvironmentRef,
}

impl InputSource {
    async fn current(
        &self,
    ) -> Result<(
        cowork_storage::ManagedInputTransfer,
        cowork_storage::ManagedInputContext,
    )> {
        let (_, observation) = self
            .runtime
            .process_environment(&self.project, &self.environment)
            .await?;
        self.runtime
            .store
            .managed_input_transfer(&self.run, &self.token, &self.attempt, &observation)
            .await
    }
}

#[async_trait::async_trait]
impl ManagedInputAccess for InputSource {
    async fn manifest(&self) -> Result<ManagedInputManifest> {
        let (transfer, context) = self.current().await?;
        let custody: cowork_broker::RunWorkspaceRecord =
            serde_json::from_value(context.workspace).map_err(storage_error)?;
        let files = self.runtime.files.clone().ok_or(Error::Forbidden)?;
        tokio::task::spawn_blocking(move || {
            files.validate_managed_input_sources(&custody, &context.run, &context.plan.source_spec)
        })
        .await
        .map_err(storage_error)??;
        let (fresh, _) = self.current().await?;
        if canonical_hash(&fresh) != canonical_hash(&transfer) {
            return Err(Error::ExecutionPlanChanged);
        }
        Ok(ManagedInputManifest {
            grant: transfer.grant,
            manifest: transfer.manifest,
        })
    }

    async fn chunk(&self, input: ReadWorkspaceTransferChunk) -> Result<cowork_broker::BinaryFile> {
        let (transfer, context) = self.current().await?;
        let file = transfer
            .manifest
            .files
            .iter()
            .find(|file| file.path == input.path)
            .ok_or(Error::Forbidden)?;
        if !file
            .chunks
            .iter()
            .any(|chunk| chunk.offset == input.offset && chunk.sha256 == input.expected_sha256)
        {
            return Err(Error::Forbidden);
        }
        let custody: cowork_broker::RunWorkspaceRecord =
            serde_json::from_value(context.workspace).map_err(storage_error)?;
        let files = self.runtime.files.clone().ok_or(Error::Forbidden)?;
        let snapshot = transfer.snapshot.clone();
        let permit = self.runtime.begin_upload_transfer()?;
        let bytes = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            files.managed_input_chunk(
                &custody,
                &context.run,
                &context.plan.source_spec,
                &snapshot,
                &input,
            )
        })
        .await
        .map_err(storage_error)??;
        let (fresh, _) = self.current().await?;
        if canonical_hash(&fresh) != canonical_hash(&transfer) {
            return Err(Error::ExecutionPlanChanged);
        }
        Ok(bytes)
    }
}

impl Runtime {
    /// Trusted controller entry. A valid signature alone is insufficient: the
    /// store must prove the original confirmed plan, one admitted attempt and
    /// current worker/target/environment before any source bytes are captured.
    pub async fn prepare_managed_process_input(
        &self,
        token: &str,
        request: Signed<LaunchRequest>,
    ) -> Result<Arc<dyn ManagedInputAccess>> {
        let runner = self.process_runner.as_ref().ok_or(Error::Forbidden)?;
        let (pin, observation) = self
            .process_environment_for_request(&request.payload)
            .await?;
        let context = self
            .store
            .managed_input_context(token, &request, &observation)
            .await?;
        let ExecutionDestinationV2::ManagedRunner {
            target,
            runner: identity,
            ..
        } = &context.plan.plan.target
        else {
            return Err(Error::Forbidden);
        };
        if identity != &runner.execution_identity()
            || pin.reference() != request.payload.environment
        {
            return Err(Error::Forbidden);
        }
        let custody: cowork_broker::RunWorkspaceRecord =
            serde_json::from_value(context.workspace).map_err(storage_error)?;
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        let source = context.plan.source_spec.clone();
        let record = context.run.clone();
        let permit = self.begin_upload_transfer()?;
        let prepared = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            files.prepare_managed_input(&custody, &record, &source)
        })
        .await
        .map_err(storage_error)??;
        let r = &request.payload;
        let grant = ManagedInputGrant {
            format_version: 1,
            execution_plan_id: context.plan.plan.id.clone(),
            execution_plan_sha256: context.plan.sha256.clone(),
            target: target.clone(),
            runner: identity.clone(),
            input_manifest_sha256: prepared.manifest.sha256.clone(),
            process_attempt_id: r.process_attempt_id.clone(),
            run_id: r.run_id.clone(),
            request_sha256: r.request_sha256.clone(),
            environment: r.environment.clone(),
            invocation_sha256: canonical_hash(&r.invocation),
            worker_fence_sha256: r.worker_fence_sha256.clone(),
            lease_epoch: r.lease_epoch,
            issued_at: r.issued_at,
            expires_at: r.expires_at,
        };
        let signed = runner.sign_managed_input(grant)?;
        let transfer = cowork_storage::ManagedInputTransfer {
            request: request.clone(),
            grant: signed,
            snapshot: prepared.snapshot.clone(),
            manifest: prepared.manifest.clone(),
        };
        // Keep the source/revocation and native-copy locks through SQL custody.
        let (_, observation) = self.process_environment_for_request(r).await?;
        self.store
            .retain_managed_input_transfer(token, &transfer, &observation)
            .await?;
        drop(prepared);
        Ok(Arc::new(InputSource {
            runtime: self.clone(),
            project: context.run.spec.project_id,
            run: r.run_id.clone(),
            attempt: r.process_attempt_id.clone(),
            token: token.into(),
            environment: r.environment.clone(),
        }))
    }

    async fn process_environment_for_request(
        &self,
        request: &LaunchRequest,
    ) -> Result<(
        cowork_domain::environments::EnvironmentPin,
        cowork_domain::environments::EnvironmentObservation,
    )> {
        let run = self.store.stored_run(&request.run_id).await?;
        if run.record.spec.environment.as_ref() != Some(&request.environment) {
            return Err(Error::Forbidden);
        }
        self.process_environment(&run.record.spec.project_id, &request.environment)
            .await
    }

    pub(super) async fn managed_execution_destination(
        &self,
        spec: &RunSpec,
        controller: BrokerControllerRef,
    ) -> Result<ExecutionDestinationV2> {
        let reference = spec.environment.as_ref().ok_or(Error::Forbidden)?;
        let (pin, _) = self
            .process_environment(&spec.project_id, reference)
            .await?;
        if !matches!(
            pin.definition.artifact,
            EnvironmentArtifact::ManagedLinuxPackage { .. }
        ) || pin.definition.network != Some(EnvironmentNetwork::Denied)
            || spec.folders.iter().any(|folder| !folder.writable)
        {
            // The current recipient exposes one writable workspace. Read-only
            // alias enforcement must be bound by its own future grant contract.
            return Err(Error::Validation(
                "managed_snapshot_scope_not_supported".into(),
            ));
        }
        let (target, runner) = self
            .execution_target_binding()
            .await?
            .ok_or(Error::Forbidden)?;
        if runner.runner_id != spec.executor {
            return Err(Error::ExecutionPlanChanged);
        }
        self.process_runner
            .as_ref()
            .ok_or(Error::Forbidden)?
            .require_managed_input_ready(reference, &target)
            .await?;
        if self.execution_target_binding().await?.as_ref()
            != Some(&(target.clone(), runner.clone()))
        {
            return Err(Error::ExecutionPlanChanged);
        }
        Ok(ExecutionDestinationV2::ManagedRunner {
            controller,
            target,
            runner,
            programs: pin.definition.programs,
        })
    }
}
