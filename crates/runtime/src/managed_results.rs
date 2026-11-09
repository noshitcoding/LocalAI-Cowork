//! Separate short-lived result content reads. Neither a completed receipt nor
//! an expired execution/input lease becomes a file grant through this module.
use super::*;
use cowork_broker::{ManagedResultReadGuard, RunWorkspaceRecord};
use cowork_runner_protocol::{
    MANAGED_RESULT_READ_MILLIS, ManagedReadiness, ManagedReadyRequest, ManagedResultContextRequest,
    ManagedResultManifest, ManagedResultReadContext, ManagedResultReadOperation,
    ManagedResultReadRequest, Signed,
};
use cowork_storage::ManagedResultContext;
use std::time::Instant;
mod versions;
mod window;

struct Authority {
    context: ManagedResultContext,
    guard: Arc<ManagedResultReadGuard>,
    runner: Arc<ProcessRunnerConfig>,
}

/// The deadline starts before the observation request; response latency and
/// subsequent local authorization checks never create a new observation window.
struct ResultObservation {
    context: Signed<ManagedResultReadContext>,
    deadline: Instant,
    window: window::Window,
}

impl Runtime {
    async fn result_context(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
        origin: &str,
    ) -> Result<ManagedResultContext> {
        let record = self.store.stored_run(run).await?.record;
        if record.spec.order_id != order {
            return Err(Error::Forbidden);
        }
        let environment = record.spec.environment.as_ref().ok_or(Error::Forbidden)?;
        let (_, observation) = self
            .process_environment(&record.spec.project_id, environment)
            .await?;
        self.store
            .managed_result_context(order, run, attempt, &observation, origin)
            .await
    }
    async fn result_authority(&self, order: &str, run: &str, attempt: &str) -> Result<Authority> {
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        let origin = {
            let files = files.clone();
            tokio::task::spawn_blocking(move || files.local_controller_origin())
                .await
                .map_err(storage_error)??
        };
        let context = self.result_context(order, run, attempt, &origin).await?;
        let guard = self.result_native_guard(&context, &origin).await?;
        let runner = self.process_runner.clone().ok_or(Error::Forbidden)?;
        if runner.execution_identity() != context.input.grant.payload.runner
            || runner.registered_target() != Some(&context.input.grant.payload.target)
        {
            return Err(Error::ExecutionPlanChanged);
        }
        let authority = Authority {
            context,
            guard,
            runner,
        };
        self.recheck_result_authority(&authority).await?;
        Ok(authority)
    }
    async fn result_native_guard(
        &self,
        context: &ManagedResultContext,
        origin: &str,
    ) -> Result<Arc<ManagedResultReadGuard>> {
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        let order = &context.run.spec.order_id;
        // A restarted broker has no in-memory order-workspace registration.
        // Resolve only the independently stored original source workspace;
        // expected=Some forbids creation/replacement if its directory is gone.
        if context
            .plan
            .source_spec
            .folders
            .iter()
            .any(|grant| grant.order_id.is_some())
        {
            let expected = self.store.workspace(order).await?.ok_or(Error::Forbidden)?;
            let owner = self.store.order(order).await?;
            let base = self.store.workspace_base(order).await?;
            let source_files = files.clone();
            tokio::task::spawn_blocking(move || {
                source_files.ensure_workspace(&owner, base.as_ref(), Some(&expected))
            })
            .await
            .map_err(storage_error)??;
        }
        let custody: RunWorkspaceRecord =
            serde_json::from_value(context.workspace.clone()).map_err(storage_error)?;
        let source = context.plan.source_spec.clone();
        let record = context.run.clone();
        let guard = tokio::task::spawn_blocking(move || {
            files.prepare_managed_result_read(&custody, &record, &source)
        })
        .await
        .map_err(storage_error)??;
        if guard.origin() != origin {
            return Err(Error::ExecutionPlanChanged);
        }
        Ok(Arc::new(guard))
    }
    async fn recheck_result_authority(&self, authority: &Authority) -> Result<()> {
        let context = &authority.context;
        let record = context.run.clone();
        // This read performs no copy writes or fallback to an original folder.
        self.recheck_result_native(authority).await?;
        let fresh = self
            .result_context(
                &record.spec.order_id,
                &record.id,
                &context.input.request.payload.process_attempt_id,
                authority.guard.origin(),
            )
            .await?;
        if fresh.authorization_sha256 != context.authorization_sha256 {
            return Err(Error::ExecutionPlanChanged);
        }
        self.recheck_result_native(authority).await
    }
    async fn recheck_result_native(&self, authority: &Authority) -> Result<()> {
        self.recheck_result_native_guard(&authority.context, &authority.guard)
            .await
    }
    async fn recheck_result_native_guard(
        &self,
        context: &ManagedResultContext,
        guard: &Arc<ManagedResultReadGuard>,
    ) -> Result<()> {
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        let custody: RunWorkspaceRecord =
            serde_json::from_value(context.workspace.clone()).map_err(storage_error)?;
        let source = context.plan.source_spec.clone();
        let record = context.run.clone();
        let guard = guard.clone();
        tokio::task::spawn_blocking(move || {
            files.revalidate_managed_result_read(&guard, &custody, &record, &source)
        })
        .await
        .map_err(storage_error)?
    }
    async fn result_ready(&self, authority: &Authority) -> Result<Signed<ManagedReadiness>> {
        let request = ManagedReadyRequest {
            format_version: 1,
            request_id: uuid::Uuid::new_v4().to_string(),
            environment: authority.context.input.request.payload.environment.clone(),
            target: authority.context.input.grant.payload.target.clone(),
        };
        let started = Instant::now();
        let signed = authority.runner.result_readiness(request.clone()).await?;
        signed
            .verify(
                "managed_readiness",
                &authority.runner.result_binding().runner_public_key,
            )
            .map_err(|_| Error::Forbidden)?;
        signed
            .payload
            .validate(&request, &authority.runner.result_binding(), now())
            .map_err(|_| Error::Forbidden)?;
        if started.elapsed()
            >= Duration::from_millis(cowork_runner_protocol::MANAGED_READINESS_MILLIS as u64)
        {
            return Err(Error::Forbidden);
        }
        self.recheck_result_authority(authority).await?;
        Ok(signed)
    }
    async fn observe_result_context(
        &self,
        authority: &Authority,
        readiness: &Signed<ManagedReadiness>,
        retained_until: Option<i64>,
    ) -> Result<ResultObservation> {
        let clock = window::Clock::sample()?;
        let started = Instant::now();
        let issued = now();
        if issued < readiness.payload.observed_at || issued >= readiness.payload.expires_at {
            return Err(Error::Forbidden);
        }
        let grant = &authority.context.input.grant.payload;
        let expires = issued
            .checked_add(MANAGED_RESULT_READ_MILLIS)
            .ok_or(Error::Forbidden)?
            .min(retained_until.unwrap_or(i64::MAX));
        let request = ManagedResultContextRequest {
            format_version: 1,
            request_id: uuid::Uuid::new_v4().to_string(),
            runtime_instance_sha256: readiness.payload.runtime_instance_sha256.clone(),
            process_attempt_id: grant.process_attempt_id.clone(),
            run_id: grant.run_id.clone(),
            launch_envelope_sha256: cowork_runner_protocol::envelope_hash(
                &authority.context.input.request,
            ),
            execution_receipt_sha256: canonical_hash(&authority.context.receipt),
            runner: grant.runner.clone(),
            target: grant.target.clone(),
            issued_at: issued,
            expires_at: expires,
        };
        request
            .validate(
                &authority.runner.result_binding(),
                &grant.target,
                &readiness.payload.runtime_instance_sha256,
                issued,
            )
            .map_err(|_| Error::Forbidden)?;
        let signed = authority
            .runner
            .result_read_context(request.clone())
            .await?;
        signed
            .verify(
                "managed_result_context",
                &authority.runner.result_binding().runner_public_key,
            )
            .map_err(|_| Error::Forbidden)?;
        signed
            .payload
            .validate(&request, &authority.runner.result_binding(), now())
            .map_err(|_| Error::Forbidden)?;
        // Use the original pre-request anchor even if native retention clipped
        // the returned expiry. No second HTTP call or legacy read is attempted.
        let remaining = signed
            .payload
            .expires_at
            .checked_sub(issued)
            .filter(|n| *n > 0)
            .ok_or(Error::Forbidden)?;
        let observation = ResultObservation {
            context: signed,
            deadline: started
                .checked_add(Duration::from_millis(remaining as u64))
                .ok_or(Error::Forbidden)?,
            window: clock.window(remaining)?,
        };
        self.recheck_result_authority(authority).await?;
        Self::require_context_window(authority, &observation)?;
        Ok(observation)
    }
    fn require_context_window(
        authority: &Authority,
        observation: &ResultObservation,
    ) -> Result<()> {
        observation.window.check()?;
        if Instant::now() >= observation.deadline {
            return Err(Error::Forbidden);
        }
        observation
            .context
            .payload
            .validate(
                &observation.context.payload.request,
                &authority.runner.result_binding(),
                now(),
            )
            .map_err(|_| Error::Forbidden)
    }
    fn result_read_request(
        authority: &Authority,
        readiness: &Signed<ManagedReadiness>,
        observation: &ResultObservation,
        operation: ManagedResultReadOperation,
        retained_until: Option<i64>,
    ) -> Result<(Signed<ManagedResultReadRequest>, Instant)> {
        let context = &authority.context;
        let grant = &context.input.grant.payload;
        let issued = now();
        Self::require_context_window(authority, observation)?;
        if issued < observation.context.payload.observed_at {
            return Err(Error::Forbidden);
        }
        let expires = issued
            .checked_add(MANAGED_RESULT_READ_MILLIS)
            .ok_or(Error::Forbidden)?
            .min(retained_until.unwrap_or(i64::MAX))
            .min(observation.context.payload.expires_at);
        if expires <= issued {
            return Err(Error::Forbidden);
        }
        let request = ManagedResultReadRequest {
            format_version: 2,
            runtime_instance_sha256: readiness.payload.runtime_instance_sha256.clone(),
            request_id: uuid::Uuid::new_v4().to_string(),
            actor_id: context.actor_id.clone(),
            authorization_sha256: context.authorization_sha256.clone(),
            execution_plan_id: grant.execution_plan_id.clone(),
            execution_plan_sha256: grant.execution_plan_sha256.clone(),
            process_attempt_id: grant.process_attempt_id.clone(),
            run_id: grant.run_id.clone(),
            launch_envelope_sha256: cowork_runner_protocol::envelope_hash(&context.input.request),
            execution_receipt_sha256: canonical_hash(&context.receipt),
            runner: grant.runner.clone(),
            target: grant.target.clone(),
            operation,
            issued_at: issued,
            expires_at: expires,
            context: Some(observation.context.clone()),
        };
        request
            .validate(
                &authority.runner.result_binding(),
                &grant.target,
                &readiness.payload.runtime_instance_sha256,
                issued,
            )
            .map_err(|_| Error::Forbidden)?;
        let deadline = observation
            .deadline
            .min(Instant::now() + Duration::from_millis((expires - issued) as u64));
        Ok((authority.runner.sign_result_read(request)?, deadline))
    }
    fn validate_result_manifest(
        authority: &Authority,
        ready: &Signed<ManagedReadiness>,
        result: &ManagedResultManifest,
    ) -> Result<()> {
        let context = &authority.context;
        result
            .attestation
            .verify(
                "managed_output",
                &context.input.grant.payload.runner.runner_public_key,
            )
            .map_err(|_| Error::Forbidden)?;
        result
            .attestation
            .payload
            .validate(
                &context.input.grant.payload,
                &context.input.request,
                &context.receipt,
                &result.manifest,
                &ready.payload.runtime_instance_sha256,
                now(),
            )
            .map_err(|_| Error::Forbidden)?;
        // A portable result never gains another folder/root merely by naming it.
        for path in result
            .manifest
            .directories
            .iter()
            .chain(result.manifest.files.iter().map(|f| &f.path))
        {
            if !context
                .plan
                .workspace
                .copies
                .iter()
                .any(|copy| path.split('/').next() == Some(copy.alias.as_str()))
            {
                return Err(Error::Forbidden);
            }
        }
        Ok(())
    }
    fn require_read_window(
        authority: &Authority,
        ready: &Signed<ManagedReadiness>,
        observation: &ResultObservation,
        request: &Signed<ManagedResultReadRequest>,
        deadline: Instant,
    ) -> Result<()> {
        Self::require_context_window(authority, observation)?;
        if Instant::now() >= deadline {
            return Err(Error::Forbidden);
        }
        request
            .payload
            .validate(
                &authority.runner.result_binding(),
                &authority.context.input.grant.payload.target,
                &ready.payload.runtime_instance_sha256,
                now(),
            )
            .map_err(|_| Error::Forbidden)
    }
    fn result_retention_deadline(result: &ManagedResultManifest) -> Result<Instant> {
        let remaining = result
            .attestation
            .payload
            .retained_until
            .checked_sub(now())
            .filter(|n| *n > 0)
            .ok_or(Error::Forbidden)?;
        Instant::now()
            .checked_add(Duration::from_millis(remaining as u64))
            .ok_or(Error::Forbidden)
    }
    /// Trusted owner read of one original result. No renderer-provided actor,
    /// receipt, worker token, native path or input lease becomes read authority.
    pub async fn managed_result_manifest(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
    ) -> Result<ManagedResultManifest> {
        let _permit = self.begin_upload_transfer()?;
        let authority = self.result_authority(order, run, attempt).await?;
        self.read_result_manifest_authorized(authority).await
    }
    async fn read_result_manifest_authorized(
        &self,
        authority: Authority,
    ) -> Result<ManagedResultManifest> {
        let ready = self.result_ready(&authority).await?;
        let observation = self
            .observe_result_context(&authority, &ready, None)
            .await?;
        let (request, deadline) = Self::result_read_request(
            &authority,
            &ready,
            &observation,
            ManagedResultReadOperation::Manifest,
            None,
        )?;
        let result = authority.runner.result_manifest(request.clone()).await?;
        Self::validate_result_manifest(&authority, &ready, &result)?;
        let retention_deadline = Self::result_retention_deadline(&result)?;
        self.recheck_result_authority(&authority).await?;
        Self::validate_result_manifest(&authority, &ready, &result)?;
        Self::require_read_window(
            &authority,
            &ready,
            &observation,
            &request,
            deadline.min(retention_deadline),
        )?;
        Ok(result)
    }
    pub async fn managed_result_chunk(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
        result: &ManagedResultManifest,
        chunk: ReadWorkspaceTransferChunk,
    ) -> Result<Vec<u8>> {
        let _permit = self.begin_upload_transfer()?;
        let authority = self.result_authority(order, run, attempt).await?;
        self.read_result_chunk_authorized(authority, result, chunk)
            .await
    }
    async fn read_result_chunk_authorized(
        &self,
        authority: Authority,
        result: &ManagedResultManifest,
        chunk: ReadWorkspaceTransferChunk,
    ) -> Result<Vec<u8>> {
        let ready = self.result_ready(&authority).await?;
        Self::validate_result_manifest(&authority, &ready, result)?;
        let retention_deadline = Self::result_retention_deadline(result)?;
        let expected = result
            .manifest
            .files
            .iter()
            .find(|f| f.path == chunk.path)
            .and_then(|f| {
                f.chunks
                    .iter()
                    .find(|c| c.offset == chunk.offset && c.sha256 == chunk.expected_sha256)
            })
            .ok_or(Error::Forbidden)?;
        let operation = ManagedResultReadOperation::Chunk {
            attestation_sha256: canonical_hash(&result.attestation),
            manifest_sha256: result.manifest.sha256.clone(),
            chunk,
        };
        let observation = self
            .observe_result_context(
                &authority,
                &ready,
                Some(result.attestation.payload.retained_until),
            )
            .await?;
        let (request, deadline) = Self::result_read_request(
            &authority,
            &ready,
            &observation,
            operation,
            Some(result.attestation.payload.retained_until),
        )?;
        let bytes = authority
            .runner
            .result_chunk(request.clone(), expected.size)
            .await?;
        if bytes.len() as u64 != expected.size || hash(&bytes) != expected.sha256 {
            return Err(Error::FileConflict);
        }
        Self::validate_result_manifest(&authority, &ready, result)?;
        self.recheck_result_authority(&authority).await?;
        Self::validate_result_manifest(&authority, &ready, result)?;
        Self::require_read_window(
            &authority,
            &ready,
            &observation,
            &request,
            deadline.min(retention_deadline),
        )?;
        Ok(bytes)
    }
}
