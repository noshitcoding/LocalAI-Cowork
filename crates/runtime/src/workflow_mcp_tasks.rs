//! Native graph tasks retain their original tool turn and role model authority.
use super::native_secrets::VerifiedNativeKeys;
use super::*;
use cowork_mcp::{CallResult, Session, SessionCipher, TaskOperation, TaskOperationResult};
use cowork_secrets::{KeyId, KeyStore};
use cowork_storage::mcp_interactions::{
    McpInteractionExecution, McpInteractionRecord, ScopedTaskInteractions, sampling_messages,
};
use cowork_storage::mcp_tasks::McpTaskOperationPermit;
use cowork_storage::{WorkflowModelContext, WorkflowModelResponse};
use futures_util::future::BoxFuture;

struct Execution<'a> {
    runtime: &'a Runtime,
    project: &'a str,
    run: &'a str,
    lease: &'a str,
    turn: &'a str,
    raw: &'a str,
    permit: &'a McpTaskOperationPermit,
    keys: &'a VerifiedNativeKeys,
    key: KeyId,
    stop: &'a CancellationToken,
}
impl Execution<'_> {
    async fn current(&self) -> Result<()> {
        if self.stop.is_cancelled() {
            return Err(Error::UncertainEffect);
        }
        self.runtime
            .store
            .validate_workflow_mcp_task_operation(
                self.project,
                self.run,
                self.lease,
                self.turn,
                self.raw,
                self.permit,
                self.key,
            )
            .await
    }
    async fn sampling_current(&self, original: &McpInteractionRecord) -> Result<()> {
        self.current().await?;
        let fresh = self
            .runtime
            .store
            .mcp_task_interaction(self.permit, &original.view.id, self.keys)
            .await?;
        if fresh.view.state != "sampling_admitted"
            || fresh.view.request_sha256 != original.view.request_sha256
            || canonical_hash(&fresh.decision) != canonical_hash(&original.decision)
            || canonical_hash(&fresh.decision_actor) != canonical_hash(&original.decision_actor)
        {
            return Err(Error::UncertainEffect);
        }
        Ok(())
    }
    async fn sample_original(&self, interaction: &McpInteractionRecord) -> Result<()> {
        self.current().await?;
        let original = self
            .runtime
            .store
            .workflow_mcp_task_context(
                self.project,
                self.run,
                self.lease,
                self.turn,
                self.raw,
                self.key,
            )
            .await?;
        let mut spec = original.record.spec.clone();
        let frozen = spec.provider.clone().ok_or(Error::Forbidden)?;
        let credential = self.runtime.workflow_credential(&spec).await?;
        let edited =
            interaction.decision.as_ref().ok_or(Error::Forbidden)?["edited_messages"].as_array();
        let mut messages = original.turn.context.messages.clone();
        messages.extend(sampling_messages(
            &interaction.view.original_request["params"],
            edited.map(Vec::as_slice),
        )?);
        spec.allowed_tools.clear();
        spec.mcp_tools.clear();
        spec.output = None;
        spec.file_action = None;
        let allowance = interaction.view.original_request["params"]["maxTokens"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n > 0)
            .ok_or(Error::Forbidden)?;
        spec.budgets.max_output_tokens = spec.budgets.max_output_tokens.min(allowance);
        let mut prepared = self.runtime.prepare_model(&spec, &messages).await?;
        let id = canonical_hash(&json!([
            "workflow-mcp-sampling/1",
            self.run,
            original.tool.id,
            interaction.view.id
        ]));
        let logical = prepared.logical_fingerprint().to_owned();
        let wire = prepared.wire_sha256().to_owned();
        let context = WorkflowModelContext {
            spec: spec.clone(),
            messages: messages.clone(),
            tools: json!([]),
            child_results: Value::Null,
            request_sha256: logical.clone(),
        };
        let intent = json!({"format":1,"kind":"workflow_sampling","interaction_id":interaction.view.id,
            "task_record_id":interaction.task.view.id,"operation_id":self.permit.id,"attempt_id":id,
            "parent_spec_sha256":interaction.task.spec_sha256,"role_spec_sha256":canonical_hash(&original.record.spec),
            "claim":original.turn.claim,"spec":spec,"messages":messages,"logical_sha256":logical,
            "wire_sha256":wire,"context_measurement":prepared.context_measurement()});
        self.runtime
            .store
            .admit_mcp_interaction_sampling(
                self.permit,
                &interaction.view.id,
                &intent,
                self.keys,
                self.key,
            )
            .await?;
        self.runtime
            .store
            .admit_workflow_mcp_sampling(
                self.project,
                self.run,
                self.lease,
                self.turn,
                self.raw,
                &id,
                interaction,
                context,
                &wire,
            )
            .await?;
        self.sampling_current(interaction).await?;
        self.runtime.store.registered_model(&spec).await?;
        prepared.bind_dispatch_id(self.run, &id)?;
        let money = self
            .runtime
            .store
            .monetary_dispatch_scope(self.run, self.lease, &id)
            .await?;
        let observer = provider::ProviderReceiveObserver::new(money);
        let (tx, mut rx) = mpsc::channel(16);
        let request =
            provider::complete_prepared_observed(prepared, credential, tx, observer.clone());
        tokio::pin!(request);
        let deadline = tokio::time::Instant::now()
            + Duration::from_millis(original.deadline.saturating_sub(now()).max(0) as u64);
        let mut pulse = tokio::time::interval(Duration::from_millis(250));
        let outcome: Result<provider::Reply> = async {
            loop {
                tokio::select! { biased;
                    _ = self.stop.cancelled() => return Err(Error::UncertainEffect),
                    _ = tokio::time::sleep_until(deadline) => return Err(Error::BudgetExceeded),
                    _ = pulse.tick() => self.sampling_current(interaction).await?,
                    Some(_) = rx.recv() => {},
                    result = &mut request => return result,
                }
            }
        }
        .await;
        let reply = match outcome {
            Ok(reply) => reply,
            Err(error) => {
                self.runtime
                    .note_provider_uncertainty(self.run, &id, &observer)
                    .await?;
                return Err(error);
            }
        };
        let workflow_response = WorkflowModelResponse {
            provider_continuation: reply.provider_continuation.clone(),
            content: reply.content.clone(),
            tools: reply.tools.clone(),
            usage: reply.usage.clone(),
            received_cost_metadata: reply.received_cost_metadata.clone(),
            rejection_code: None,
        };
        let protected = json!({"result":{"role":"assistant","content":{"type":"text","text":reply.content},
            "model":frozen.model,"stopReason":"endTurn"},"usage":reply.usage,"received_cost_metadata":reply.received_cost_metadata,
            "attempt_id":id,"request_sha256":logical,"wire_sha256":wire,"workflow_response":workflow_response});
        self.runtime
            .store
            .capture_mcp_interaction_sampling(
                self.permit,
                &interaction.view.id,
                &protected,
                self.keys,
                self.key,
            )
            .await?;
        self.runtime
            .store
            .finish_workflow_mcp_sampling(
                self.project,
                self.run,
                self.lease,
                &id,
                workflow_response,
                reply.received.as_ref(),
            )
            .await?;
        self.current().await
    }
}
impl McpInteractionExecution for Execution<'_> {
    fn response_bearer(&self) -> BoxFuture<'_, Result<Option<zeroize::Zeroizing<String>>>> {
        Box::pin(async move {
            self.current().await?;
            let original = self
                .runtime
                .store
                .workflow_mcp_task_context(
                    self.project,
                    self.run,
                    self.lease,
                    self.turn,
                    self.raw,
                    self.key,
                )
                .await?;
            let binding = cowork_domain::mcp::mcp_binding(&original.record, &original.tool)?;
            let digest = canonical_hash(binding);
            let bearer = self
                .runtime
                .native_mcp_task_bearer(self.project, binding)
                .await?;
            self.current().await?;
            let fresh = self
                .runtime
                .store
                .workflow_mcp_task_context(
                    self.project,
                    self.run,
                    self.lease,
                    self.turn,
                    self.raw,
                    self.key,
                )
                .await?;
            if canonical_hash(cowork_domain::mcp::mcp_binding(&fresh.record, &fresh.tool)?)
                != digest
            {
                return Err(Error::UncertainEffect);
            }
            Ok(bearer)
        })
    }
    fn validate(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(self.current())
    }
    fn sample<'a>(&'a self, interaction: &'a McpInteractionRecord) -> BoxFuture<'a, Result<()>> {
        Box::pin(self.sample_original(interaction))
    }
}

impl Runtime {
    #[expect(
        clippy::too_many_arguments,
        reason = "await_workflow_mcp_task: Keep workflow project/run/lease/turn, original MCP task permit and native key/session authority explicit"
    )]
    pub(super) async fn await_workflow_mcp_task(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        turn: &str,
        raw: &str,
        session: &mut Session,
        first: CallResult,
        source: cowork_storage::WorkflowMcpSource,
        keys: &VerifiedNativeKeys,
        key: KeyId,
        stop: &CancellationToken,
    ) -> Result<CallResult> {
        let mut ack = first.task.as_ref().ok_or(Error::Forbidden)?.clone();
        let scope = source.checkpoint_scope().clone();
        let cipher = SessionCipher::new(
            &key.cipher_id().map_err(|_| Error::NativeKeyUnavailable)?,
            keys.load(&key).map_err(|_| Error::NativeKeyUnavailable)?,
        )
        .map_err(|_| Error::NativeKeyUnavailable)?;
        let mut task = self
            .store
            .capture_workflow_mcp_task(source, &first, session, Some((&cipher, scope)), keys, key)
            .await?;
        loop {
            if stop.is_cancelled() || ack.expires_at_ms().is_some_and(|v| v <= now()) {
                return Err(Error::UncertainEffect);
            }
            let _current = self
                .store
                .workflow_mcp_task_context(project, run, lease, turn, raw, key)
                .await?;
            let cancel = self.store.native_mcp_task_cancel_requested(&task).await?;
            let operation = if cancel && !ack.status.terminal() {
                TaskOperation::Cancel
            } else if ack.status.terminal() || ack.status == cowork_mcp::TaskStatus::InputRequired {
                TaskOperation::Result
            } else {
                TaskOperation::Get
            };
            if operation == TaskOperation::Get {
                let wait_until =
                    tokio::time::Instant::now() + Duration::from_millis(ack.poll_interval_ms);
                loop {
                    tokio::select! { biased;
                        _ = stop.cancelled() => return Err(Error::UncertainEffect),
                        _ = tokio::time::sleep_until(wait_until) => break,
                        _ = tokio::time::sleep(Duration::from_millis(250)) => {
                            self.store.workflow_mcp_task_context(project, run, lease, turn, raw, key).await?;
                        }
                    }
                }
            }
            let before_refresh = self
                .store
                .workflow_mcp_task_context(project, run, lease, turn, raw, key)
                .await?;
            let binding =
                cowork_domain::mcp::mcp_binding(&before_refresh.record, &before_refresh.tool)?;
            let original_binding = canonical_hash(binding);
            self.refresh_native_mcp_task_bearer(project, binding, session)
                .await?;
            if stop.is_cancelled() {
                return Err(Error::UncertainEffect);
            }
            let current = self
                .store
                .workflow_mcp_task_context(project, run, lease, turn, raw, key)
                .await?;
            if canonical_hash(cowork_domain::mcp::mcp_binding(
                &current.record,
                &current.tool,
            )?) != original_binding
            {
                return Err(Error::UncertainEffect);
            }
            let remaining = current
                .deadline
                .min(ack.expires_at_ms().unwrap_or(i64::MAX))
                .saturating_sub(now())
                .max(0) as u64;
            let prepared = session
                .prepare_task_operation_bounded(&ack, operation, remaining.min(86_400_000))
                .map_err(|_| Error::McpSessionUnavailable)?;
            let permit = self
                .store
                .begin_workflow_mcp_task_operation(
                    project, run, lease, turn, raw, &task, &prepared, keys, key,
                )
                .await?;
            let execution = Execution {
                runtime: self,
                project,
                run,
                lease,
                turn,
                raw,
                permit: &permit,
                keys,
                key,
                stop,
            };
            let handler = ScopedTaskInteractions {
                store: &self.store,
                permit: &permit,
                keys,
                key,
                execution: &execution,
                deadline: current.deadline,
            };
            execution.current().await?;
            let outcome = {
                let request = session.task_operation_interactive(prepared, &ack, &handler);
                tokio::pin!(request);
                let mut pulse = tokio::time::interval(Duration::from_millis(250));
                loop {
                    tokio::select! { biased;
                        _ = stop.cancelled() => break Err(Error::UncertainEffect),
                        _ = pulse.tick() => { if let Err(error) = execution.current().await { break Err(error); } },
                        result = &mut request => break Ok(result),
                    }
                }
            };
            let result = match outcome {
                Ok(Ok(result)) => result,
                Ok(Err(failure)) => {
                    if let Some(response) = failure.response() {
                        self.store.capture_mcp_task_operation(&permit, &json!({"kind":"unconfirmed","response":response,"code":failure.code.to_string(),"effects_unknown":true}), keys, key).await?;
                        self.store.finish_mcp_task_evidence_unknown(&permit).await?;
                    } else {
                        self.store.unknown_mcp_task_operation(&permit).await?;
                    }
                    return Err(Error::UncertainEffect);
                }
                Err(error) => {
                    self.store.unknown_mcp_task_operation(&permit).await?;
                    return Err(error);
                }
            };
            let response = match &result {
                TaskOperationResult::Status(value) => json!({"kind":"status","ack":value}),
                TaskOperationResult::Result(value) => {
                    json!({"kind":"result","response":value.response,"response_sha256":value.response_sha256})
                }
                TaskOperationResult::Error { response, sha256 } => {
                    json!({"kind":"error","response":response,"response_sha256":sha256})
                }
            };
            self.store
                .capture_mcp_task_operation(&permit, &response, keys, key)
                .await?;
            task = self
                .store
                .commit_workflow_mcp_task_operation(
                    project, run, lease, turn, raw, &permit, keys, key,
                )
                .await?;
            match result {
                TaskOperationResult::Status(value) => ack = value,
                TaskOperationResult::Result(value)
                    if !matches!(
                        ack.status,
                        cowork_mcp::TaskStatus::Failed | cowork_mcp::TaskStatus::Cancelled
                    ) && value.response["isError"] != true =>
                {
                    return Ok(value);
                }
                _ => return Err(Error::UncertainEffect),
            }
        }
    }
}
