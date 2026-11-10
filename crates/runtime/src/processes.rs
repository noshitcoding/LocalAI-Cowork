use super::*;
use cowork_domain::environments::{
    EnvironmentChange, EnvironmentObservation, EnvironmentPin, EnvironmentQualification,
    EnvironmentRecord, EnvironmentVersionState,
};
use cowork_runner_protocol::{
    Binding, EnvironmentInstall, ExecutionLeaseRenewal, ExecutionState, FenceRequest,
    LaunchRequest, Receipt, SandboxLifecycleReceipt, SandboxLifecycleRequest, SandboxPreviewGrant,
    SandboxPreviewRequest, Signed, SigningKey,
};
mod owner_reconciliation;
#[cfg(test)]
mod tests;

/// Transport-level decision for one exact persisted delivery request. Only the
/// authenticated explicit expiry response permits preparing a replacement.
#[derive(Clone, Debug)]
pub enum ManagedResultDeliveryResponse {
    Accepted(Box<Signed<cowork_runner_protocol::ManagedResultDeliveryReceipt>>),
    ConfirmationExpired,
}

/// Trusted installed runner transport; implementations verify TLS/endpoint identity and
/// perform actual adapter qualification. Renderer payloads cannot create this authority.
#[async_trait::async_trait]
pub trait ProcessRunnerAccess: Send + Sync {
    async fn managed_result_delivery_history(
        &self,
        _request: Signed<cowork_runner_protocol::ManagedResultDeliveryRequest>,
    ) -> Result<Signed<cowork_runner_protocol::ManagedResultDeliveryReceipt>> {
        Err(Error::Forbidden)
    }
    async fn managed_result_delivery(
        &self,
        _request: Signed<cowork_runner_protocol::ManagedResultDeliveryRequest>,
    ) -> Result<ManagedResultDeliveryResponse> {
        Err(Error::Forbidden)
    }
    async fn managed_result_retirement(
        &self,
        _request: Signed<cowork_runner_protocol::ManagedResultRetirementQuery>,
    ) -> Result<Signed<cowork_runner_protocol::ManagedResultRetirementReceipt>> {
        Err(Error::Forbidden)
    }
    /// Observe retained result custody and the current fence without granting
    /// execution authority. Unsupported transports must fail without fallback.
    async fn managed_result_context(
        &self,
        _request: Signed<cowork_runner_protocol::ManagedResultContextRequest>,
    ) -> Result<Signed<cowork_runner_protocol::ManagedResultReadContext>> {
        Err(Error::Forbidden)
    }
    async fn managed_result_readiness(
        &self,
        _request: cowork_runner_protocol::ManagedReadyRequest,
    ) -> Result<Signed<cowork_runner_protocol::ManagedReadiness>> {
        Err(Error::Forbidden)
    }
    async fn managed_result_manifest(
        &self,
        _request: Signed<cowork_runner_protocol::ManagedResultReadRequest>,
    ) -> Result<cowork_runner_protocol::ManagedResultManifest> {
        Err(Error::Forbidden)
    }
    async fn managed_result_chunk(
        &self,
        _request: Signed<cowork_runner_protocol::ManagedResultReadRequest>,
        _expected_size: u64,
    ) -> Result<Vec<u8>> {
        Err(Error::Forbidden)
    }
    /// Readiness for this exact installed profile and recipient. No reservation
    /// or launch occurs here. An older transport cannot advertise this adapter.
    async fn managed_input_ready(
        &self,
        _environment: &ExecutionEnvironmentRef,
        _target: &ExecutionTargetRef,
    ) -> Result<()> {
        Err(Error::Forbidden)
    }
    /// Separate verified-input path. Older adapters cannot silently turn a
    /// confirmed snapshot into their legacy workspace-id launch mechanism.
    async fn managed_launch(
        &self,
        _request: Signed<LaunchRequest>,
        _input: Arc<dyn crate::ManagedInputAccess>,
        _stop: CancellationToken,
    ) -> Result<Signed<Receipt>> {
        Err(Error::Forbidden)
    }
    async fn sandbox_preview(
        &self,
        _request: Signed<SandboxPreviewRequest>,
    ) -> Result<Signed<SandboxPreviewGrant>> {
        Err(Error::Forbidden)
    }
    async fn sandbox_lifecycle_receipt(
        &self,
        _request_id: &str,
    ) -> Result<Signed<SandboxLifecycleReceipt>> {
        Err(Error::Forbidden)
    }
    async fn sandbox_lifecycle(
        &self,
        _request: Signed<SandboxLifecycleRequest>,
    ) -> Result<Signed<SandboxLifecycleReceipt>> {
        Err(Error::Forbidden)
    }
    /// Read only the existing exact execution receipt. This never posts launch.
    async fn receipt(&self, _attempt: &str) -> Result<Signed<Receipt>> {
        Err(Error::Forbidden)
    }
    /// Install only the operator-provided pinned artifact and return fresh physical
    /// qualification evidence. Payloads cannot assert capabilities or host paths.
    async fn install_environment(
        &self,
        _request: Signed<EnvironmentInstall>,
    ) -> Result<Signed<EnvironmentQualification>> {
        Err(Error::Forbidden)
    }
    async fn observe(
        &self,
        reference: &ExecutionEnvironmentRef,
    ) -> Result<Signed<EnvironmentObservation>>;
    async fn fence(&self, request: Signed<FenceRequest>) -> Result<()>;
    /// Extend only the current, exact dispatched execution before its durable lease expires.
    /// The adapter verifies controller purpose/signature and retains immutable renewals.
    async fn renew_execution_lease(&self, request: Signed<ExecutionLeaseRenewal>) -> Result<()>;
    /// Returned completion must follow physical termination of the exact owned process tree.
    /// A lost reply stays uncertain; implementations never resend an OS launch on retry.
    /// Dropping this request future must not drop process ownership: an independent
    /// supervisor observes `stop`, durable fences, the expiring authenticated execution
    /// lease and the original execution deadline,
    /// terminates/reaps its owned tree, then durably records the physical result.
    async fn launch(
        &self,
        request: Signed<LaunchRequest>,
        stop: CancellationToken,
    ) -> Result<Signed<Receipt>>;
}

// Cancellation also reaches the independent runner supervisor if the caller is
// aborted while awaiting a response. This is not a physical-termination receipt.
struct ProcessCallStop(CancellationToken, CancellationToken);
impl Drop for ProcessCallStop {
    fn drop(&mut self) {
        self.0.cancel();
        self.1.cancel();
    }
}

async fn revoke_process_authority(
    runner: &ProcessRunnerConfig,
    fence: &FenceRequest,
    stop: &CancellationToken,
) {
    // Local cancellation must not wait for a broken transport or a revocation ACK.
    stop.cancel();
    let mut revoke = fence.clone();
    revoke.revoked = true;
    if let Ok(signed) = runner.control.sign("fence", revoke) {
        let _ = tokio::time::timeout(Duration::from_secs(2), runner.access.fence(signed)).await;
    }
}

pub struct ProcessRunnerConfig {
    binding: Binding,
    control: SigningKey,
    access: Arc<dyn ProcessRunnerAccess>,
    target: Option<ExecutionTargetRef>,
}
impl ProcessRunnerConfig {
    pub(crate) async fn result_delivery_history(
        &self,
        request: cowork_runner_protocol::ManagedResultDeliveryRequest,
    ) -> Result<Signed<cowork_runner_protocol::ManagedResultDeliveryReceipt>> {
        request
            .validate(
                &self.binding,
                &request.target,
                &request.runtime_instance_sha256,
                request.issued_at,
            )
            .map_err(|_| Error::Forbidden)?;
        let signed = self
            .control
            .sign("managed_result_delivery_request", request)
            .map_err(|_| Error::Forbidden)?;
        let receipt = self
            .access
            .managed_result_delivery_history(signed.clone())
            .await?;
        receipt
            .verify(
                "managed_result_delivery_receipt",
                &self.binding.runner_public_key,
            )
            .map_err(|_| Error::UncertainEffect)?;
        receipt
            .payload
            .validate(&signed, &self.binding)
            .map_err(|_| Error::UncertainEffect)?;
        Ok(receipt)
    }
    pub(crate) async fn deliver_result(
        &self,
        request: cowork_runner_protocol::ManagedResultDeliveryRequest,
    ) -> Result<ManagedResultDeliveryResponse> {
        // Exact committed requests must remain replayable after accept_before;
        // first acceptance is checked by the native service, never renewed here.
        request
            .validate(
                &self.binding,
                &request.target,
                &request.runtime_instance_sha256,
                request.issued_at,
            )
            .map_err(|_| Error::Forbidden)?;
        let signed = self
            .control
            .sign("managed_result_delivery_request", request)
            .map_err(|_| Error::Forbidden)?;
        let response = self.access.managed_result_delivery(signed.clone()).await?;
        if let ManagedResultDeliveryResponse::Accepted(receipt) = &response {
            receipt
                .verify(
                    "managed_result_delivery_receipt",
                    &self.binding.runner_public_key,
                )
                .map_err(|_| Error::UncertainEffect)?;
            receipt
                .payload
                .validate(&signed, &self.binding)
                .map_err(|_| Error::UncertainEffect)?;
        }
        Ok(response)
    }
    pub(crate) async fn result_retirement(
        &self,
        request: cowork_runner_protocol::ManagedResultRetirementQuery,
        delivery: &Signed<cowork_runner_protocol::ManagedResultDeliveryReceipt>,
        physical_sha256: &str,
    ) -> Result<Signed<cowork_runner_protocol::ManagedResultRetirementReceipt>> {
        request
            .validate(delivery, &self.binding, now())
            .map_err(|_| Error::Forbidden)?;
        let signed = self
            .control
            .sign("managed_result_retirement_query", request)
            .map_err(|_| Error::Forbidden)?;
        let receipt = self.access.managed_result_retirement(signed).await?;
        receipt
            .verify(
                "managed_result_retirement_receipt",
                &self.binding.runner_public_key,
            )
            .map_err(|_| Error::Forbidden)?;
        receipt
            .payload
            .validate(delivery, &self.binding, physical_sha256)
            .map_err(|_| Error::Forbidden)?;
        Ok(receipt)
    }
    pub(crate) async fn result_read_context(
        &self,
        request: cowork_runner_protocol::ManagedResultContextRequest,
    ) -> Result<Signed<cowork_runner_protocol::ManagedResultReadContext>> {
        let signed = self
            .control
            .sign("managed_result_context_request", request)
            .map_err(|_| Error::Forbidden)?;
        self.access.managed_result_context(signed).await
    }
    pub(crate) fn result_binding(&self) -> Binding {
        self.binding.clone()
    }
    pub(crate) fn sign_result_read(
        &self,
        request: cowork_runner_protocol::ManagedResultReadRequest,
    ) -> Result<Signed<cowork_runner_protocol::ManagedResultReadRequest>> {
        self.control
            .sign("managed_result_read", request)
            .map_err(|_| Error::Forbidden)
    }
    pub(crate) async fn result_readiness(
        &self,
        request: cowork_runner_protocol::ManagedReadyRequest,
    ) -> Result<Signed<cowork_runner_protocol::ManagedReadiness>> {
        self.access.managed_result_readiness(request).await
    }
    pub(crate) async fn result_manifest(
        &self,
        request: Signed<cowork_runner_protocol::ManagedResultReadRequest>,
    ) -> Result<cowork_runner_protocol::ManagedResultManifest> {
        self.access.managed_result_manifest(request).await
    }
    pub(crate) async fn result_chunk(
        &self,
        request: Signed<cowork_runner_protocol::ManagedResultReadRequest>,
        size: u64,
    ) -> Result<Vec<u8>> {
        self.access.managed_result_chunk(request, size).await
    }
    pub(crate) async fn require_managed_input_ready(
        &self,
        environment: &ExecutionEnvironmentRef,
        target: &ExecutionTargetRef,
    ) -> Result<()> {
        self.access.managed_input_ready(environment, target).await
    }
    pub(crate) fn sign_managed_input(
        &self,
        grant: cowork_runner_protocol::ManagedInputGrant,
    ) -> Result<Signed<cowork_runner_protocol::ManagedInputGrant>> {
        self.control
            .sign("managed_input_grant", grant)
            .map_err(|_| Error::Forbidden)
    }
    pub fn new(
        binding: Binding,
        control: SigningKey,
        access: Arc<dyn ProcessRunnerAccess>,
    ) -> Result<Self> {
        if control.public_key() != binding.control_public_key
            || binding.runner_public_key.len() != 32
            || binding.generation == 0
            || !cowork_domain::processes::valid_sha256(&binding.adapter_build_sha256)
        {
            return Err(Error::Forbidden);
        }
        let value = Self {
            binding,
            control,
            access,
            target: None,
        };
        cowork_domain::execution_targets::validate_runner_identity(&value.execution_identity())?;
        Ok(value)
    }
    pub fn execution_identity(&self) -> ExecutionRunnerIdentity {
        ExecutionRunnerIdentity {
            runner_id: self.binding.runner_id.clone(),
            generation: self.binding.generation,
            adapter_build_sha256: self.binding.adapter_build_sha256.clone(),
            control_public_key: self.binding.control_public_key.clone(),
            runner_public_key: self.binding.runner_public_key.clone(),
        }
    }
    pub fn registered_target(&self) -> Option<&ExecutionTargetRef> {
        self.target.as_ref()
    }
    async fn observation(
        &self,
        reference: &ExecutionEnvironmentRef,
    ) -> Result<EnvironmentObservation> {
        let signed = self.access.observe(reference).await?;
        signed
            .verify("environment_observation", &self.binding.runner_public_key)
            .map_err(|_| Error::Forbidden)?;
        let observation = signed.payload;
        if observation.runner_id != self.binding.runner_id
            || observation.runner_generation != self.binding.generation
            || observation.adapter_build_sha256 != self.binding.adapter_build_sha256
            || observation.definition_sha256 != reference.definition_sha256
        {
            return Err(Error::Forbidden);
        }
        Ok(observation)
    }
}

impl Runtime {
    pub async fn sandbox_operation_history(
        &self,
        project: &str,
        order_id: &str,
    ) -> Result<Vec<SandboxSessionOperation>> {
        let order = self.store.order(order_id).await?;
        if order.project_id != project {
            return Err(Error::NotFound);
        }
        let runner = self.process_runner.as_ref().ok_or(Error::Forbidden)?;
        let mut output = Vec::new();
        for (id, key, state, value) in self
            .store
            .sandbox_operation_history(project, order_id)
            .await?
        {
            let signed: Signed<SandboxLifecycleRequest> =
                serde_json::from_value(value).map_err(storage_error)?;
            signed
                .verify("sandbox_lifecycle", &runner.binding.control_public_key)
                .map_err(|_| Error::Forbidden)?;
            let p = signed.payload;
            if p.project_id != project {
                return Err(Error::Forbidden);
            }
            output.push(SandboxSessionOperation {
                id,
                state,
                request: ManageSandboxSession {
                    environment: p.environment,
                    idempotency_key: key,
                    operation: p.operation,
                    expected_sandbox_uid: p.expected_sandbox_uid,
                },
            });
        }
        Ok(output)
    }
    pub async fn sandbox_preview(
        &self,
        project: &str,
        order_id: &str,
        input: &CreateSandboxPreview,
        preview_base_domain: &str,
    ) -> Result<SandboxPreview> {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let base = preview_base_domain;
        if base.len() > 200
            || !base.contains('.')
            || !base.split('.').all(|s| {
                !s.is_empty()
                    && s.len() <= 63
                    && s.as_bytes()[0].is_ascii_alphanumeric()
                    && s.as_bytes()[s.len() - 1].is_ascii_alphanumeric()
                    && s.bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            })
        {
            return Err(Error::Forbidden);
        }
        let order = self.store.order(order_id).await?;
        if order.project_id != project {
            return Err(Error::NotFound);
        }
        self.require_sandbox_session_for_order(project, order_id, &input.environment)
            .await?;
        let workspace = self
            .store
            .workspace(order_id)
            .await?
            .ok_or(Error::NotFound)?;
        let runner = self.process_runner.as_ref().ok_or(Error::Forbidden)?;
        let at = now();
        self.assert_process_target(runner).await?;
        let request = SandboxPreviewRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            runner_id: runner.binding.runner_id.clone(),
            runner_generation: runner.binding.generation,
            adapter_build_sha256: runner.binding.adapter_build_sha256.clone(),
            project_id: project.into(),
            workspace_resource_id: workspace.folder.resource_id.clone(),
            environment: input.environment.clone(),
            expected_session_uid: input.expected_sandbox_uid.clone(),
            port: input.port,
            issued_at: at,
            expires_at: at + 30_000,
        };
        request
            .validate(&runner.binding, at)
            .map_err(|_| Error::Forbidden)?;
        let signed = runner
            .access
            .sandbox_preview(
                runner
                    .control
                    .sign("sandbox_preview_request", request)
                    .map_err(|_| Error::Forbidden)?,
            )
            .await?;
        signed
            .verify("sandbox_preview", &runner.binding.runner_public_key)
            .map_err(|_| Error::Forbidden)?;
        self.assert_process_target(runner).await?;
        let grant = &signed.payload;
        let suffix = format!(".{base}");
        let label = grant
            .preview_host
            .strip_suffix(&suffix)
            .ok_or(Error::Forbidden)?;
        if label.is_empty()
            || label.len() > 63
            || !label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || grant.runner_id != runner.binding.runner_id
            || grant.runner_generation != runner.binding.generation
            || grant.project_id != project
            || grant.workspace_resource_id != workspace.folder.resource_id
            || grant.environment != input.environment
            || grant.port != input.port
            || grant.issued_at < at
            || grant.issued_at > now() + 5000
            || grant.expires_at <= now()
            || grant.expires_at.saturating_sub(grant.issued_at) > 60_000
        {
            return Err(Error::Forbidden);
        }
        let ticket = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&signed).map_err(storage_error)?);
        Ok(SandboxPreview {
            url: format!("https://{}/_cowork/bootstrap#{ticket}", grant.preview_host),
            expires_at: grant.expires_at,
        })
    }
    pub(crate) async fn require_sandbox_session_for_order(
        &self,
        project: &str,
        order: &str,
        reference: &ExecutionEnvironmentRef,
    ) -> Result<()> {
        let record = self
            .store
            .environment(project, &reference.environment_id)
            .await?;
        let version = record
            .versions
            .iter()
            .find(|v| v.id == reference.version_id)
            .ok_or(Error::NotFound)?;
        if !matches!(
            version.definition.artifact,
            cowork_domain::environments::EnvironmentArtifact::LinuxKataImage { .. }
        ) {
            return Ok(());
        }
        let value = self
            .store
            .sandbox_session_result(project, order)
            .await?
            .ok_or_else(|| {
                Error::Validation(
                    "Prepare the Kubernetes workspace session before starting this run".into(),
                )
            })?;
        let receipt: Signed<SandboxLifecycleReceipt> =
            serde_json::from_value(value).map_err(storage_error)?;
        let runner = self.process_runner.as_ref().ok_or(Error::Forbidden)?;
        receipt
            .verify(
                "sandbox_lifecycle_receipt",
                &runner.binding.runner_public_key,
            )
            .map_err(|_| Error::Forbidden)?;
        if receipt.payload.request.project_id != project
            || receipt.payload.request.environment != *reference
            || !matches!(
                receipt.payload.state,
                SandboxLifecycleState::Ready | SandboxLifecycleState::Suspended
            )
        {
            return Err(Error::Validation(
                "Prepare or resume the selected Kubernetes workspace session".into(),
            ));
        }
        Ok(())
    }
    /// Owner API control path. A model tool cannot choose a namespace, Pod or
    /// host path; identities come from the registered project/workspace.
    pub async fn manage_sandbox_session(
        &self,
        project: &str,
        order_id: &str,
        input: &ManageSandboxSession,
    ) -> Result<SandboxSession> {
        valid_text(&input.idempotency_key, 128)?;
        let order = self.store.order(order_id).await?;
        if order.project_id != project {
            return Err(Error::NotFound);
        }
        let runner = self.process_runner.as_ref().ok_or(Error::Forbidden)?;
        let hash = canonical_hash(&(project, order_id, input));
        let prior = self
            .store
            .sandbox_operation_by_key(project, order_id, &input.idempotency_key, &hash)
            .await?;
        if let Some(prior) = prior.as_ref().filter(|p| p.state == "pending") {
            let original: Signed<SandboxLifecycleRequest> =
                serde_json::from_value(prior.request.clone()).map_err(storage_error)?;
            original
                .verify("sandbox_lifecycle", &runner.binding.control_public_key)
                .map_err(|_| Error::Forbidden)?;
            if original.payload.expires_at <= now() {
                // Expire only a never-dispatched intent, including after revoke.
                self.store
                    .expire_undispatched_sandbox_operation(&prior.id)
                    .await?;
                return Err(Error::Conflict);
            }
        }
        // Reconciliation is read-only and uses the frozen signed intent. Fresh
        // environment observation or filesystem access must not strand an
        // already dispatched operation after revocation or a broker outage.
        let operation = if let Some(prior) = prior.filter(|p| p.state != "pending") {
            prior
        } else {
            let record = self
                .store
                .environment(project, &input.environment.environment_id)
                .await?;
            let version = record
                .versions
                .iter()
                .find(|v| v.id == input.environment.version_id)
                .ok_or(Error::NotFound)?;
            if !matches!(
                version.definition.artifact,
                cowork_domain::environments::EnvironmentArtifact::LinuxKataImage { .. }
            ) {
                return Err(Error::Validation(
                    "Session lifecycle requires a Kubernetes Kata environment".into(),
                ));
            }
            let starts_guest = matches!(
                input.operation,
                SandboxLifecycleOperation::Ensure | SandboxLifecycleOperation::Resume
            );
            if starts_guest {
                self.process_environment(project, &input.environment)
                    .await?;
            }
            // Revocation must block new execution while preserving owner access
            // to inspect or stop the existing exact session. Cleanup never
            // creates a broker workspace and still uses the runner's UID checks.
            let workspace = if !starts_guest {
                self.store.workspace(order_id).await?.ok_or(
                    if input.operation == SandboxLifecycleOperation::Inspect {
                        Error::SandboxSessionNotFound
                    } else {
                        Error::NotFound
                    },
                )?
            } else {
                self.workspace(order_id).await?
            };
            if workspace.folder.project_id != project
                || workspace.folder.order_id.as_deref() != Some(order_id)
            {
                return Err(Error::Forbidden);
            }
            let at = now();
            let request = runner
                .control
                .sign(
                    "sandbox_lifecycle",
                    SandboxLifecycleRequest {
                        request_id: uuid::Uuid::new_v4().to_string(),
                        runner_id: runner.binding.runner_id.clone(),
                        runner_generation: runner.binding.generation,
                        adapter_build_sha256: runner.binding.adapter_build_sha256.clone(),
                        project_id: project.into(),
                        workspace_resource_id: workspace.folder.resource_id.clone(),
                        environment: input.environment.clone(),
                        operation: input.operation,
                        expected_sandbox_uid: input.expected_sandbox_uid.clone(),
                        issued_at: at,
                        expires_at: at + 30_000,
                    },
                )
                .map_err(|_| Error::Forbidden)?;
            self.store
                .prepare_sandbox_operation(
                    project,
                    order_id,
                    &workspace.folder.resource_id,
                    &input.idempotency_key,
                    &hash,
                    input.operation,
                    &serde_json::to_value(&request).map_err(storage_error)?,
                )
                .await?
        };
        let frozen: Signed<SandboxLifecycleRequest> =
            serde_json::from_value(operation.request).map_err(storage_error)?;
        frozen
            .verify("sandbox_lifecycle", &runner.binding.control_public_key)
            .map_err(|_| Error::Forbidden)?;
        if frozen.payload.project_id != project
            || frozen.payload.workspace_resource_id != operation.resource_id
            || frozen.payload.environment != input.environment
            || frozen.payload.operation != input.operation
            || frozen.payload.expected_sandbox_uid != input.expected_sandbox_uid
        {
            return Err(Error::Forbidden);
        }
        let signed: Signed<SandboxLifecycleReceipt> = if let Some(result) = operation.result {
            serde_json::from_value(result).map_err(storage_error)?
        } else if operation.state == "dispatched" {
            let response = runner
                .access
                .sandbox_lifecycle_receipt(&frozen.payload.request_id)
                .await?;
            response
                .verify(
                    "sandbox_lifecycle_receipt",
                    &runner.binding.runner_public_key,
                )
                .map_err(|_| Error::Forbidden)?;
            if response.payload.request != frozen.payload {
                return Err(Error::UncertainEffect);
            }
            self.store
                .complete_sandbox_operation(
                    &operation.id,
                    &serde_json::to_value(&response).map_err(storage_error)?,
                )
                .await?;
            response
        } else {
            if operation.state != "pending" {
                return Err(Error::UncertainEffect);
            }
            if frozen.payload.validate(&runner.binding, now()).is_err() {
                self.store
                    .expire_undispatched_sandbox_operation(&operation.id)
                    .await?;
                return Err(Error::Conflict);
            }
            if matches!(
                frozen.payload.operation,
                SandboxLifecycleOperation::Ensure | SandboxLifecycleOperation::Resume
            ) {
                self.assert_process_target(runner).await?;
            }
            if !self.store.dispatch_sandbox_operation(&operation.id).await? {
                return Err(Error::UncertainEffect);
            }
            // Exactly one transport mutation. Read-only reconciliation is a separate operation.
            let response = match runner.access.sandbox_lifecycle(frozen.clone()).await {
                Ok(response) => response,
                Err(error @ (Error::WorkspaceBusy | Error::SandboxSessionNotFound)) => {
                    self.store
                        .reject_unadmitted_sandbox_operation(&operation.id)
                        .await?;
                    return Err(error);
                }
                Err(error) => return Err(error),
            };
            response
                .verify(
                    "sandbox_lifecycle_receipt",
                    &runner.binding.runner_public_key,
                )
                .map_err(|_| Error::Forbidden)?;
            if response.payload.request != frozen.payload {
                return Err(Error::UncertainEffect);
            }
            self.store
                .complete_sandbox_operation(
                    &operation.id,
                    &serde_json::to_value(&response).map_err(storage_error)?,
                )
                .await?;
            response
        };
        signed
            .verify(
                "sandbox_lifecycle_receipt",
                &runner.binding.runner_public_key,
            )
            .map_err(|_| Error::Forbidden)?;
        let receipt = signed.payload;
        if receipt.request != frozen.payload
            || receipt.observed_at < receipt.request.issued_at
            || receipt.workspace_pvc.is_empty()
            || receipt.sandbox_name.is_empty()
        {
            return Err(Error::UncertainEffect);
        }
        Ok(SandboxSession {
            project_id: project.into(),
            order_id: order_id.into(),
            environment: input.environment.clone(),
            operation_id: operation.id,
            sandbox_name: receipt.sandbox_name,
            sandbox_uid: receipt.sandbox_uid,
            pod_uid: receipt.pod_uid,
            state: receipt.state,
            workspace_preserved: true,
            observed_at: receipt.observed_at,
        })
    }
    pub(super) async fn retained_workflow_runner_outcome(
        &self,
        evidence: &cowork_storage::WorkflowExternalEvidence,
    ) -> Result<Value> {
        let runner = self.process_runner.as_ref().ok_or(Error::Forbidden)?;
        let request: Signed<LaunchRequest> =
            serde_json::from_value(evidence.preparation["launch"].clone())
                .map_err(storage_error)?;
        request
            .verify("launch", &runner.binding.control_public_key)
            .map_err(|_| Error::Forbidden)?;
        if request.payload.run_id != evidence.run_id
            || request.payload.process_attempt_id != evidence.tool.id
            || request.payload.tool_call_id != evidence.tool.id
            || request.payload.invocation
                != cowork_domain::processes::process_invocation(&evidence.tool)?
        {
            return Err(Error::Forbidden);
        }
        let receipt = runner
            .access
            .receipt(&request.payload.process_attempt_id)
            .await?;
        receipt
            .verify("receipt", &runner.binding.runner_public_key)
            .map_err(|_| Error::Forbidden)?;
        if receipt.payload.request != request.payload
            || receipt.payload.request_envelope_sha256
                != cowork_runner_protocol::envelope_hash(&request)
            || receipt.payload.revision != 3
        {
            return Err(Error::UncertainEffect);
        }
        let ExecutionState::Completed { result } = &receipt.payload.state else {
            return Err(Error::UncertainEffect);
        };
        let success = result.termination == ProcessTermination::Exited
            && result.exit_code == Some(0)
            && result.output_complete;
        Ok(
            json!({"result":result,"runner_receipt_sha256":canonical_hash(&receipt.payload),"runner_signed_receipt":receipt,"_workflow_success":success,"reconciled":true}),
        )
    }
    pub async fn qualify_process_environment(
        &self,
        project: &str,
        environment: &str,
        version: &str,
        expected_revision: i64,
        key: &str,
    ) -> Result<EnvironmentRecord> {
        let record = self.store.environment(project, environment).await?;
        if record.revision != expected_revision || record.revoked {
            return Err(Error::Conflict);
        }
        let staged = record
            .versions
            .iter()
            .find(|v| v.id == version && !v.revoked)
            .ok_or(Error::NotFound)?;
        if !matches!(staged.state, EnvironmentVersionState::Staged) {
            return Err(Error::Conflict);
        }
        let runner = self.process_runner.as_ref().ok_or(Error::Forbidden)?;
        self.assert_process_target(runner).await?;
        let request = runner
            .control
            .sign(
                "environment_install",
                EnvironmentInstall {
                    environment_id: environment.into(),
                    version_id: version.into(),
                    definition: staged.definition.clone(),
                },
            )
            .map_err(|_| Error::Forbidden)?;
        let signed = runner.access.install_environment(request).await?;
        signed
            .verify(
                "environment_qualification",
                &runner.binding.runner_public_key,
            )
            .map_err(|_| Error::Forbidden)?;
        self.assert_process_target(runner).await?;
        let q = &signed.payload;
        if q.runner_id != runner.binding.runner_id
            || q.runner_generation != runner.binding.generation
            || q.adapter_build_sha256 != runner.binding.adapter_build_sha256
            || q.definition_sha256 != staged.sha256
            || q.artifact_sha256 != staged.definition.artifact.sha256()
        {
            return Err(Error::Forbidden);
        }
        self.store
            .change_environment(
                project,
                environment,
                expected_revision,
                key,
                &EnvironmentChange::Qualify {
                    version_id: version.into(),
                    qualification: signed.payload,
                },
            )
            .await?;
        self.store.environment(project, environment).await
    }
    pub async fn activate_process_environment(
        &self,
        project: &str,
        environment: &str,
        version: &str,
        expected_revision: i64,
        key: &str,
    ) -> Result<EnvironmentRecord> {
        let record = self.store.environment(project, environment).await?;
        if record.revision != expected_revision || record.revoked {
            return Err(Error::Conflict);
        }
        let staged = record
            .versions
            .iter()
            .find(|v| v.id == version && !v.revoked)
            .ok_or(Error::NotFound)?;
        let q = match &staged.state {
            EnvironmentVersionState::Qualified { qualification } => qualification,
            _ => return Err(Error::Forbidden),
        };
        let reference = ExecutionEnvironmentRef {
            environment_id: environment.into(),
            version_id: version.into(),
            definition_sha256: staged.sha256.clone(),
            qualification_sha256: canonical_hash(q),
        };
        let runner = self.process_runner.as_ref().ok_or(Error::Forbidden)?;
        self.assert_process_target(runner).await?;
        let observation = runner.observation(&reference).await?;
        self.assert_process_target(runner).await?;
        self.store
            .change_environment(
                project,
                environment,
                expected_revision,
                key,
                &EnvironmentChange::Activate {
                    version_id: version.into(),
                    observation,
                },
            )
            .await?;
        self.store.environment(project, environment).await
    }
    /// The workflow broker has already committed the exact scoped tool attempt.
    /// This path uses only a qualified installed runner and its signed physical receipt.
    pub(crate) async fn invoke_workflow_program(
        &self,
        record: &RunRecord,
        tool: &ToolCall,
        expected_lease: &str,
        turn: &str,
        raw: &str,
        stop: &CancellationToken,
    ) -> Result<Value> {
        let runner = self
            .process_runner
            .as_ref()
            .ok_or(Error::Forbidden)?
            .clone();
        let reference = record.spec.environment.as_ref().ok_or(Error::Forbidden)?;
        let (pin, _) = self
            .process_environment(&record.spec.project_id, reference)
            .await?;
        let initial = self.store.stored_run(&record.id).await?;
        let lease = expected_lease.to_owned();
        initial.assert_lease(&lease, now())?;
        let workspace = record
            .spec
            .folders
            .iter()
            .find(|f| f.writable && f.order_id.as_deref() == Some(record.spec.order_id.as_str()))
            .ok_or(Error::Forbidden)?;
        let issued = now();
        let execution_deadline = issued
            .checked_add(remaining_run_millis(&initial.record, issued) as i64)
            .ok_or(Error::BudgetExceeded)?;
        let request = LaunchRequest {
            format_version: 1,
            runner_id: runner.binding.runner_id.clone(),
            runner_generation: runner.binding.generation,
            adapter_build_sha256: runner.binding.adapter_build_sha256.clone(),
            process_attempt_id: tool.id.clone(),
            run_id: record.id.clone(),
            tool_call_id: tool.id.clone(),
            request_sha256: canonical_hash(&json!({"spec":record.spec,"tool":tool})),
            worker_fence_sha256: hash(lease.as_bytes()),
            lease_epoch: initial.record.revision as u64,
            environment: reference.clone(),
            workspace_resource_id: workspace.resource_id.clone(),
            invocation: cowork_domain::processes::process_invocation(tool)?,
            issued_at: issued,
            expires_at: initial
                .lease
                .as_ref()
                .ok_or(Error::StaleLease)?
                .until
                .min(issued + 30_000)
                .min(execution_deadline),
            execution_deadline,
        };
        let fence = FenceRequest {
            runner_id: request.runner_id.clone(),
            runner_generation: request.runner_generation,
            run_id: request.run_id.clone(),
            lease_epoch: request.lease_epoch,
            revoked: false,
        };
        runner
            .access
            .fence(
                runner
                    .control
                    .sign("fence", fence.clone())
                    .map_err(|_| Error::Forbidden)?,
            )
            .await?;
        self.store
            .check_environment_pin(
                &record.spec.project_id,
                &pin,
                &runner.observation(reference).await?,
            )
            .await?;
        let signed = runner
            .control
            .sign("launch", request.clone())
            .map_err(|_| Error::Forbidden)?;
        let envelope = cowork_runner_protocol::envelope_hash(&signed);
        self.store
            .retain_workflow_external_preparation(
                &record.spec.project_id,
                &record.id,
                turn,
                raw,
                "process_runner",
                json!({"launch":signed}),
            )
            .await?;
        let job = stop.child_token();
        let watcher = CancellationToken::new();
        let _guard = ProcessCallStop(job.clone(), watcher);
        self.assert_process_target(&runner).await?;
        let launch = runner.access.launch(signed, job.clone());
        tokio::pin!(launch);
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        let mut heartbeat = tokio::time::Instant::now();
        let mut remote_until = request.expires_at;
        let response = loop {
            tokio::select! {
                biased;
                _=stop.cancelled()=>{revoke_process_authority(&runner,&fence,&job).await;return Err(Error::UncertainEffect);},
                result=&mut launch=>break result?,
                _=tick.tick()=>{
                    let mut current=self.store.stored_run(&record.id).await?;current.assert_lease(&lease,now())?;
                    if current.record.state!=RunState::Running||now()>=execution_deadline {revoke_process_authority(&runner,&fence,&job).await;return Err(Error::UncertainEffect);}
                    self.process_environment(&record.spec.project_id,reference).await?;
                    if heartbeat.elapsed()>=Duration::from_secs(3){
                        current=self.store.heartbeat(&record.id,&lease).await?;
                        let issued=now();let expires=current.lease.as_ref().ok_or(Error::StaleLease)?.until.min(issued+30_000).min(execution_deadline);
                        if expires>remote_until{
                            let renewal=ExecutionLeaseRenewal{runner_id:request.runner_id.clone(),runner_generation:request.runner_generation,adapter_build_sha256:request.adapter_build_sha256.clone(),process_attempt_id:request.process_attempt_id.clone(),run_id:request.run_id.clone(),request_envelope_sha256:envelope.clone(),worker_fence_sha256:request.worker_fence_sha256.clone(),lease_epoch:request.lease_epoch,revision:current.record.revision as u64,issued_at:issued,expires_at:expires};
                            runner.access.renew_execution_lease(runner.control.sign("execution_lease",renewal).map_err(|_|Error::Forbidden)?).await?;remote_until=expires;
                        }
                        heartbeat=tokio::time::Instant::now();
                    }
                }
            }
        };
        response
            .verify("receipt", &runner.binding.runner_public_key)
            .map_err(|_| Error::UncertainEffect)?;
        if response.payload.request != request
            || response.payload.revision != 3
            || response.payload.request_envelope_sha256 != envelope
            || response.payload.updated_at < request.issued_at
        {
            return Err(Error::UncertainEffect);
        }
        let receipt_sha256 = canonical_hash(&response.payload);
        let ExecutionState::Completed { result } = &response.payload.state else {
            return Err(Error::UncertainEffect);
        };
        let success = result.termination == ProcessTermination::Exited
            && result.exit_code == Some(0)
            && result.output_complete;
        let value = json!({"result":result,"runner_receipt_sha256":receipt_sha256,"runner_signed_receipt":response,"_workflow_success":success});
        self.store
            .retain_workflow_external_outcome(
                &record.spec.project_id,
                &record.id,
                &tool.id,
                value.clone(),
            )
            .await?;
        Ok(value)
    }
    pub fn with_process_runner(mut self, config: ProcessRunnerConfig) -> Self {
        self.process_runner = Some(Arc::new(config));
        self
    }
    /// Freeze the explicitly activated operator registration. Revocation and
    /// reactivation cannot silently upgrade this runtime to a new revision.
    pub async fn with_registered_process_runner(
        mut self,
        mut config: ProcessRunnerConfig,
    ) -> Result<Self> {
        config.target = Some(
            self.store
                .execution_target_reference(&config.execution_identity())
                .await?,
        );
        self.process_runner = Some(Arc::new(config));
        Ok(self)
    }
    /// The control plane remains available for original receipts and cleanup
    /// after restart. A missing/inactive/mismatched registration supplies no
    /// execution reference, so every new runner effect still fails its gate.
    pub async fn with_configured_process_runner(
        mut self,
        mut config: ProcessRunnerConfig,
    ) -> Result<Self> {
        let identity = config.execution_identity();
        config.target = match self.store.execution_target(&identity).await? {
            Some(value)
                if value.identity == identity && value.state == ExecutionTargetState::Active =>
            {
                Some(cowork_domain::execution_targets::target_reference(&value)?)
            }
            _ => None,
        };
        self.process_runner = Some(Arc::new(config));
        Ok(self)
    }
    async fn assert_process_target(&self, runner: &ProcessRunnerConfig) -> Result<()> {
        let reference = runner.target.as_ref().ok_or(Error::Forbidden)?;
        self.store
            .validate_execution_target(reference, &runner.execution_identity())
            .await
    }
    /// Fresh configured receiver metadata for the common plan/input adapters.
    /// This reserves no capacity and grants no snapshot or execution authority.
    pub async fn execution_target_binding(
        &self,
    ) -> Result<Option<(ExecutionTargetRef, ExecutionRunnerIdentity)>> {
        let Some(runner) = self.process_runner.as_ref() else {
            return Ok(None);
        };
        let Some(reference) = runner.registered_target() else {
            return Ok(None);
        };
        self.assert_process_target(runner).await?;
        Ok(Some((reference.clone(), runner.execution_identity())))
    }
    pub async fn process_environment(
        &self,
        project: &str,
        reference: &ExecutionEnvironmentRef,
    ) -> Result<(EnvironmentPin, EnvironmentObservation)> {
        let runner = self.process_runner.as_ref().ok_or(Error::Forbidden)?;
        self.assert_process_target(runner).await?;
        let record = self
            .store
            .environment(project, &reference.environment_id)
            .await?;
        let version = record
            .versions
            .iter()
            .find(|v| v.id == reference.version_id)
            .ok_or(Error::Forbidden)?;
        let EnvironmentVersionState::Qualified { qualification } = &version.state else {
            return Err(Error::Forbidden);
        };
        let pin = EnvironmentPin {
            environment_id: record.id.clone(),
            project_id: project.into(),
            version_id: version.id.clone(),
            definition_sha256: version.sha256.clone(),
            qualification_sha256: canonical_hash(qualification),
            definition: version.definition.clone(),
        };
        if pin.reference() != *reference {
            return Err(Error::Forbidden);
        }
        let observation = runner.observation(reference).await?;
        self.assert_process_target(runner).await?;
        record.resolve_pin(&pin, &observation, now())?;
        Ok((pin, observation))
    }
    pub(crate) async fn invoke_process_tool(
        &self,
        run_id: &str,
        token: &str,
        tool: &ToolCall,
        stop: &CancellationToken,
    ) -> Result<()> {
        let runner = self
            .process_runner
            .as_ref()
            .ok_or(Error::Forbidden)?
            .clone();
        let initial = self.store.stored_run(run_id).await?;
        initial.assert_lease(token, now())?;
        let reference = initial
            .record
            .spec
            .environment
            .as_ref()
            .ok_or(Error::Forbidden)?;
        let (pin, observation) = self
            .process_environment(&initial.record.spec.project_id, reference)
            .await?;
        let admitted = self
            .store
            .admit_process(run_id, token, tool, &observation)
            .await?;
        let permit = match admitted {
            cowork_storage::ProcessAdmission::Launch(p) => p,
            cowork_storage::ProcessAdmission::Existing(_) => return Err(Error::UncertainEffect),
        };
        // Keep the original permit until the trusted launch call. A preparation
        // error can then be settled without inventing physical termination proof.
        let prepared = async {
            // Recheck after the actual admission transaction; this never reconstructs a permit.
            let stored = self.store.stored_run(run_id).await?;
            stored.assert_lease(token, now())?;
            let timestamp = now();
            let execution_deadline = timestamp
                .checked_add(remaining_run_millis(&stored.record, timestamp) as i64)
                .ok_or(Error::BudgetExceeded)?;
            let expires_at = stored
                .lease
                .as_ref()
                .ok_or(Error::StaleLease)?
                .until
                .min(timestamp + 30_000)
                .min(execution_deadline);
            if expires_at <= timestamp {
                return Err(Error::StaleLease);
            }
            let attempt = permit.attempt();
            let request = LaunchRequest {
                format_version: 1,
                runner_id: runner.binding.runner_id.clone(),
                runner_generation: runner.binding.generation,
                adapter_build_sha256: runner.binding.adapter_build_sha256.clone(),
                process_attempt_id: attempt.id.clone(),
                run_id: run_id.into(),
                tool_call_id: tool.id.clone(),
                request_sha256: attempt.request_sha256.clone(),
                worker_fence_sha256: hash(token.as_bytes()),
                lease_epoch: stored.record.revision as u64,
                environment: reference.clone(),
                workspace_resource_id: attempt.workspace_resource_id.clone(),
                invocation: cowork_domain::processes::process_invocation(tool)?,
                issued_at: timestamp,
                expires_at,
                execution_deadline,
            };
            let fence = FenceRequest {
                runner_id: request.runner_id.clone(),
                runner_generation: request.runner_generation,
                run_id: request.run_id.clone(),
                lease_epoch: request.lease_epoch,
                revoked: false,
            };
            runner
                .access
                .fence(
                    runner
                        .control
                        .sign("fence", fence.clone())
                        .map_err(|_| Error::Forbidden)?,
                )
                .await?;
            // Final current policy/environment check immediately before the trusted transport.
            self.store
                .check_environment_pin(
                    &initial.record.spec.project_id,
                    &pin,
                    &runner.observation(reference).await?,
                )
                .await?;
            let current = self.store.stored_run(run_id).await?;
            current.assert_lease(token, now())?;
            if current.record.state != RunState::Running || stop.is_cancelled() {
                return Err(Error::UncertainEffect);
            }
            self.assert_process_target(&runner).await?;
            let signed_request = runner
                .control
                .sign("launch", request.clone())
                .map_err(|_| Error::Forbidden)?;
            let input_source = if stored.record.spec.input_workspace.is_some() {
                Some(
                    self.prepare_managed_process_input(token, signed_request.clone())
                        .await?,
                )
            } else {
                None
            };
            Ok::<_, Error>((
                request,
                fence,
                signed_request,
                input_source,
                execution_deadline,
            ))
        }
        .await;
        let (request, fence, signed_request, input_source, execution_deadline) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                permit
                    .reject_before_transport(token, error.code())
                    .await
                    .map_err(|_| Error::UncertainEffect)?;
                return Err(error);
            }
        };
        // Consuming this one capability commits to the only transport handoff.
        // Errors or lost replies from that point retain the existing uncertainty.
        let _attempt = permit.into_attempt();
        let request_envelope_sha256 = cowork_runner_protocol::envelope_hash(&signed_request);
        let managed = input_source.is_some();
        let job_stop = stop.child_token();
        let watcher_stop = CancellationToken::new();
        let _call_stop = ProcessCallStop(job_stop.clone(), watcher_stop.clone());
        let watcher = {
            let runtime = self.clone();
            let runner = runner.clone();
            let run = run_id.to_owned();
            let token = token.to_owned();
            let job = job_stop.clone();
            let end = watcher_stop.clone();
            let fence = fence.clone();
            let authority = request.clone();
            let envelope = request_envelope_sha256.clone();
            tokio::spawn(async move {
                let mut last_heartbeat = tokio::time::Instant::now();
                let mut remote_expires_at = authority.expires_at;
                loop {
                    tokio::select! {
                        _ = end.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_millis(100)) => {},
                    }
                    if job.is_cancelled() {
                        revoke_process_authority(&runner, &fence, &job).await;
                        break;
                    }
                    let validation = async {
                        let mut current = runtime.store.stored_run(&run).await?;
                        current.assert_lease(&token, now())?;
                        if current.record.state != RunState::Running
                            || time_budget_exhausted(&current.record, now())
                            || (managed && now() >= authority.expires_at)
                        {
                            return Err(Error::Forbidden);
                        }
                        let reference = current
                            .record
                            .spec
                            .environment
                            .as_ref()
                            .ok_or(Error::Forbidden)?;
                        runtime
                            .process_environment(&current.record.spec.project_id, reference)
                            .await
                            .inspect_err(|error| {
                                tracing::warn!(
                                    phase = "process_environment",
                                    code = error.code(),
                                    managed,
                                    "Process authority monitor check failed"
                                );
                            })?;
                        if last_heartbeat.elapsed() >= Duration::from_secs(3) {
                            current = runtime.store.heartbeat(&run, &token).await?;
                            let timestamp = now();
                            current.assert_lease(&token, timestamp)?;
                            if current.record.state != RunState::Running
                                || time_budget_exhausted(&current.record, timestamp)
                            {
                                return Err(Error::Forbidden);
                            }
                            let expires_at = current
                                .lease
                                .as_ref()
                                .ok_or(Error::StaleLease)?
                                .until
                                .min(timestamp.checked_add(30_000).ok_or(Error::Forbidden)?)
                                .min(authority.execution_deadline);
                            // A run already covered through its original deadline needs no
                            // new authority. Never send a shortened or equivalent extension.
                            if !managed && expires_at > remote_expires_at {
                                let renewal = ExecutionLeaseRenewal {
                                    runner_id: authority.runner_id.clone(),
                                    runner_generation: authority.runner_generation,
                                    adapter_build_sha256: authority.adapter_build_sha256.clone(),
                                    process_attempt_id: authority.process_attempt_id.clone(),
                                    run_id: authority.run_id.clone(),
                                    request_envelope_sha256: envelope.clone(),
                                    worker_fence_sha256: authority.worker_fence_sha256.clone(),
                                    lease_epoch: authority.lease_epoch,
                                    revision: current
                                        .record
                                        .revision
                                        .try_into()
                                        .map_err(|_| Error::Forbidden)?,
                                    issued_at: timestamp,
                                    expires_at,
                                };
                                runner
                                    .access
                                    .renew_execution_lease(
                                        runner
                                            .control
                                            .sign("execution_lease", renewal)
                                            .map_err(|_| Error::Forbidden)?,
                                    )
                                    .await?;
                                remote_expires_at = expires_at;
                            }
                            last_heartbeat = tokio::time::Instant::now();
                        }
                        Ok::<(), Error>(())
                    };
                    let valid = tokio::select! {
                        _ = end.cancelled() => break,
                        result = tokio::time::timeout(Duration::from_secs(3), validation) =>
                            result.unwrap_or(Err(Error::UncertainEffect)),
                    };
                    if let Err(error) = &valid {
                        tracing::warn!(
                            phase = "authority_validation",
                            code = error.code(),
                            managed,
                            "Process authority monitor rejected execution"
                        );
                    }
                    if valid.is_err() || job.is_cancelled() {
                        revoke_process_authority(&runner, &fence, &job).await;
                        break;
                    }
                }
            })
        };
        let remaining = (if managed {
            request.expires_at
        } else {
            execution_deadline
        })
        .saturating_sub(now())
        .max(0) as u64;
        let response = tokio::time::timeout(
            Duration::from_millis(remaining.saturating_add(5_000)),
            async {
                if let Some(input) = input_source {
                    runner
                        .access
                        .managed_launch(signed_request, input, job_stop.clone())
                        .await
                } else {
                    runner.access.launch(signed_request, job_stop.clone()).await
                }
            },
        )
        .await;
        let observed = (|| {
            let response = response
                .map_err(|_| Error::UncertainEffect)?
                .map_err(|_| Error::UncertainEffect)?;
            response
                .verify("receipt", &runner.binding.runner_public_key)
                .map_err(|_| Error::UncertainEffect)?;
            if response.payload.request != request
                || response.payload.revision != 3
                || response.payload.request_envelope_sha256 != request_envelope_sha256
                || response.payload.updated_at < request.issued_at
            {
                return Err(Error::UncertainEffect);
            }
            let ExecutionState::Completed { result } = &response.payload.state else {
                return Err(Error::UncertainEffect);
            };
            cowork_domain::processes::validate_process_result(result)
                .map_err(|_| Error::UncertainEffect)?;
            Ok((result.clone(), response))
        })();
        if observed.is_err() {
            revoke_process_authority(&runner, &fence, &job_stop).await;
        }
        // Join our monitor on every normal return. Unknown transport outcomes retain
        // the launch journal; only the adapter's physical receipt can settle it.
        watcher_stop.cancel();
        if watcher.await.is_err() {
            revoke_process_authority(&runner, &fence, &job_stop).await;
            return Err(Error::UncertainEffect);
        }
        let (result, receipt) = observed?;
        if managed {
            // Preserve the original signed history before the process journal
            // is settled. A future result attestation must bind this exact
            // receipt; the ProcessResult alone is not that authority.
            if self
                .store
                .retain_managed_completion_receipt(&receipt)
                .await
                .is_err()
            {
                revoke_process_authority(&runner, &fence, &job_stop).await;
                return Err(Error::UncertainEffect);
            }
        }
        if let Err(error) = self
            .store
            .finish_process(run_id, token, &tool.id, result)
            .await
        {
            revoke_process_authority(&runner, &fence, &job_stop).await;
            return Err(error);
        }
        Ok(())
    }
}
