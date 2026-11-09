//! Actual task-associated client requests, held original actor authority and
//! explicit human approval before model IO and again before releasing a sample.
use super::native_secrets::VerifiedNativeKeys;
use super::*;
use cowork_mcp::{TaskInteractionHandler, TaskServerRequest, TaskServerResponse};
use cowork_secrets::KeyId;
use cowork_storage::mcp_interactions::{
    McpInteractionRecord, interaction_response, sampling_messages,
};
use cowork_storage::mcp_tasks::McpTaskOperationPermit;
use futures_util::future::BoxFuture;
pub(super) struct NativeTaskInteractions<'a> {
    pub runtime: &'a Runtime,
    pub run: &'a str,
    pub lease: &'a str,
    pub permit: &'a McpTaskOperationPermit,
    pub keys: &'a VerifiedNativeKeys,
    pub key: KeyId,
    pub stop: &'a CancellationToken,
    pub deadline: i64,
}
fn primitive(_: Error) -> cowork_mcp::Error {
    cowork_mcp::Error::SessionUnavailable
}
impl NativeTaskInteractions<'_> {
    async fn current(&self) -> Result<()> {
        if self.stop.is_cancelled() {
            return Err(Error::UncertainEffect);
        }
        self.runtime
            .store
            .validate_native_mcp_task_operation(self.run, self.lease, self.permit, self.key)
            .await?;
        let run = self.runtime.store.stored_run(self.run).await?;
        let frozen = run.record.spec.provider.as_ref().ok_or(Error::Forbidden)?;
        self.runtime.personal_provider_credential(frozen).await?;
        self.runtime
            .store
            .registered_model(&run.record.spec)
            .await?;
        Ok(())
    }
    async fn wait_decision(&self, id: &str) -> Result<McpInteractionRecord> {
        loop {
            self.current().await?;
            let r = self
                .runtime
                .store
                .mcp_task_interaction(self.permit, id, self.keys)
                .await?;
            match r.view.state.as_str() {
                "approved" | "declined" => return Ok(r),
                "awaiting_user" | "awaiting_release" => {}
                _ => return Err(Error::UncertainEffect),
            }
            if now() >= self.deadline || r.view.expires_at <= now() {
                return Err(Error::UncertainEffect);
            }
            tokio::select! {_=self.stop.cancelled()=>return Err(Error::UncertainEffect),_=tokio::time::sleep(Duration::from_millis(250))=>{}}
        }
    }
    async fn sample(&self, r: &McpInteractionRecord) -> Result<()> {
        self.current().await?;
        let original = self.runtime.store.stored_run(self.run).await?;
        original.assert_lease(self.lease, now())?;
        ensure_running(&original)?;
        let mut spec = original.record.spec.clone();
        // Only the original selected deployment and its own system context are
        // used. Remote preferences, tools, credentials and other contexts never
        // become local authority.
        let frozen = spec.provider.clone().ok_or(Error::Forbidden)?;
        let credential = self.runtime.personal_provider_credential(&frozen).await?;
        let decision = r.decision.as_ref().ok_or(Error::Forbidden)?;
        let edited = decision["edited_messages"].as_array();
        let mut messages = original
            .checkpoint
            .messages
            .iter()
            .filter(|m| m["role"] == "system")
            .cloned()
            .collect::<Vec<_>>();
        messages.extend(sampling_messages(
            &r.view.original_request["params"],
            edited.map(Vec::as_slice),
        )?);
        spec.allowed_tools.clear();
        spec.mcp_tools.clear();
        spec.output = None;
        spec.file_action = None;
        spec.budgets.max_output_tokens = spec.budgets.max_output_tokens.min(
            u32::try_from(
                r.view.original_request["params"]["maxTokens"]
                    .as_u64()
                    .ok_or(Error::Forbidden)?,
            )
            .map_err(|_| Error::Forbidden)?,
        );
        let mut prepared = self.runtime.prepare_model(&spec, &messages).await?;
        let attempt = format!("mcp-sampling:{}", r.view.id);
        let logical = prepared.logical_fingerprint().to_owned();
        let wire = prepared.wire_sha256().to_owned();
        let parent_sha = canonical_hash(&original.record.spec);
        let intent = json!({"format":1,"interaction_id":r.view.id,"task_record_id":r.task.view.id,"operation_id":self.permit.id,"parent_spec_sha256":parent_sha,"provider_sha256":canonical_hash(&frozen),"spec":spec,"messages":messages,"logical_sha256":logical,"wire_sha256":wire,"context_measurement":prepared.context_measurement()});
        self.runtime
            .store
            .admit_mcp_interaction_sampling(self.permit, &r.view.id, &intent, self.keys, self.key)
            .await?;
        self.runtime.store.admit_prepared_model_attempt(self.run,self.lease,&attempt,&logical,&wire,|current|{
            ensure_running(current)?;if canonical_hash(&current.record.spec)!=parent_sha{return Err(Error::Forbidden);}prepared.assert_binding(&spec,&messages)?;
            reserve_model_tokens(&mut current.record,&logical,&attempt,now(),prepared.exact_reserved_tokens())?;
            Ok(Change::event("mcp_sampling_started",json!({"interaction_id":r.view.id,"attempt_id":attempt,"request_sha256":r.view.request_sha256,"wire_sha256":wire})))
        }).await?;
        self.current().await?;
        let dispatch = self.runtime.store.stored_run(self.run).await?;
        dispatch.assert_lease(self.lease, now())?;
        ensure_running(&dispatch)?;
        if canonical_hash(&dispatch.record.spec) != parent_sha
            || !dispatch.record.model_attempts.iter().any(|a| {
                a.id == attempt
                    && a.state == ModelAttemptState::InFlight
                    && a.request_sha256.as_deref() == Some(&logical)
            })
        {
            return Err(Error::UncertainEffect);
        }
        self.runtime
            .store
            .mcp_task_interaction(self.permit, &r.view.id, self.keys)
            .await?;
        self.runtime.store.registered_model(&spec).await?;
        prepared.bind_dispatch_id(self.run, &attempt)?;
        let money = self
            .runtime
            .store
            .monetary_dispatch_scope(self.run, self.lease, &attempt)
            .await?;
        let observer = provider::ProviderReceiveObserver::new(money);
        let (tx, mut rx) = mpsc::channel(16);
        let request =
            provider::complete_prepared_observed(prepared, credential, tx, observer.clone());
        tokio::pin!(request);
        let mut pulse = tokio::time::interval(Duration::from_millis(250));
        let bound = tokio::time::Instant::now()
            + Duration::from_millis(self.deadline.saturating_sub(now()).max(0) as u64);
        let outcome: Result<provider::Reply> = async {
            loop {
                tokio::select! {biased;
                    _=self.stop.cancelled()=>return Err(Error::UncertainEffect),
                    _=tokio::time::sleep_until(bound)=>return Err(Error::UncertainEffect),
                    _=pulse.tick()=>self.current().await?,
                    Some(_)=rx.recv()=>{},
                    result=&mut request=>return result,
                }
            }
        }
        .await;
        let reply = match outcome {
            Ok(reply) => reply,
            Err(error) => {
                self.runtime
                    .note_provider_uncertainty(self.run, &attempt, &observer)
                    .await?;
                return Err(error);
            }
        };
        let result = json!({"role":"assistant","content":{"type":"text","text":reply.content},"model":frozen.model,"stopReason":"endTurn"});
        let protected = json!({"result":result,"usage":reply.usage,"received_cost_metadata":reply.received_cost_metadata,"attempt_id":attempt,"request_sha256":logical,"wire_sha256":wire});
        self.runtime
            .store
            .capture_mcp_interaction_sampling(
                self.permit,
                &r.view.id,
                &protected,
                self.keys,
                self.key,
            )
            .await?;
        self.current().await?;
        self.runtime.store.settle_received_model_attempt(self.run,self.lease,&attempt,reply.received.as_ref(),|current|{
            ensure_running(current)?;if canonical_hash(&current.record.spec)!=parent_sha{return Err(Error::Forbidden);}settle_model(&mut current.record,&attempt,&reply.usage,now())?;
            if !reply.tools.is_empty()||token_budget(&current.record).limit_exceeded{return Err(Error::BudgetExceeded);}
            Ok(Change::event("mcp_sampling_received",json!({"interaction_id":r.view.id,"attempt_id":attempt,"response_sha256":canonical_hash(&protected),"awaiting_human_release":true})))
        }).await?;
        self.current().await?;
        Ok(())
    }
}
impl TaskInteractionHandler for NativeTaskInteractions<'_> {
    fn response_bearer(
        &self,
    ) -> BoxFuture<'_, cowork_mcp::Result<Option<zeroize::Zeroizing<String>>>> {
        Box::pin(async move {
            self.current().await.map_err(primitive)?;
            let current = self
                .runtime
                .store
                .stored_run(self.run)
                .await
                .map_err(primitive)?;
            let attempt = self
                .runtime
                .store
                .mcp_attempts(self.run)
                .await
                .map_err(primitive)?
                .into_iter()
                .find(|a| a.tool.id == self.permit.task.view.tool_call_id)
                .ok_or(cowork_mcp::Error::SessionUnavailable)?;
            let binding = cowork_domain::mcp::mcp_binding(&current.record, &attempt.tool)
                .map_err(primitive)?;
            let credential = self
                .runtime
                .native_mcp_task_bearer(&current.record.spec.project_id, binding)
                .await
                .map_err(primitive)?;
            self.current().await.map_err(primitive)?;
            Ok(credential)
        })
    }
    fn respond<'a>(
        &'a self,
        request: TaskServerRequest,
    ) -> BoxFuture<'a, cowork_mcp::Result<TaskServerResponse>> {
        Box::pin(async move {
            self.current().await.map_err(primitive)?;
            let r = self
                .runtime
                .store
                .capture_mcp_task_interaction(
                    self.permit,
                    &request,
                    self.deadline,
                    self.keys,
                    self.key,
                )
                .await
                .map_err(primitive)?;
            let mut r = self.wait_decision(&r.view.id).await.map_err(primitive)?;
            if r.view.method == "sampling/createMessage"
                && r.view.state == "approved"
                && r.decision
                    .as_ref()
                    .is_some_and(|d| d["action"] == "approve_sampling")
            {
                self.sample(&r).await.map_err(primitive)?;
                r = self.wait_decision(&r.view.id).await.map_err(primitive)?;
            }
            let body = interaction_response(&r).map_err(primitive)?;
            self.current().await.map_err(primitive)?;
            self.runtime
                .store
                .prepare_mcp_interaction_response(
                    self.permit,
                    &r.view.id,
                    &body,
                    self.keys,
                    self.key,
                )
                .await
                .map_err(primitive)
        })
    }
    fn validate_response<'a>(
        &'a self,
        response: &'a TaskServerResponse,
    ) -> BoxFuture<'a, cowork_mcp::Result<()>> {
        Box::pin(async move {
            self.current().await.map_err(primitive)?;
            self.runtime
                .store
                .validate_mcp_interaction_response(self.permit, response, self.keys)
                .await
                .map_err(primitive)
        })
    }
    fn acknowledge<'a>(
        &'a self,
        response: &'a TaskServerResponse,
        receipt: Value,
    ) -> BoxFuture<'a, cowork_mcp::Result<()>> {
        Box::pin(async move {
            self.runtime
                .store
                .finish_mcp_interaction_response(self.permit, response, Some(&receipt))
                .await
                .map_err(primitive)
        })
    }
    fn unknown<'a>(
        &'a self,
        response: &'a TaskServerResponse,
    ) -> BoxFuture<'a, cowork_mcp::Result<()>> {
        Box::pin(async move {
            self.runtime
                .store
                .finish_mcp_interaction_response(self.permit, response, None)
                .await
                .map_err(primitive)
        })
    }
}
impl Runtime {
    pub async fn native_mcp_task_detail(&self, run: &str, id: &str) -> Result<McpTaskDetail> {
        let (keys, key) = self.native_mcp_keys().await?;
        self.store
            .scoped_mcp_task_detail(
                &cowork_storage::mcp_oauth::McpOAuthOwner::Native { key },
                run,
                id,
                keys.as_ref(),
            )
            .await
    }
    pub async fn native_mcp_interactions(&self, run: &str) -> Result<Vec<McpTaskInteraction>> {
        let (keys, key) = self.native_mcp_keys().await?;
        self.store
            .mcp_task_interactions(
                &cowork_storage::mcp_oauth::McpOAuthOwner::Native { key },
                run,
                keys.as_ref(),
            )
            .await
    }
    pub async fn answer_native_mcp_interaction(
        &self,
        run: &str,
        id: &str,
        input: &AnswerMcpTaskInteraction,
    ) -> Result<Value> {
        let (keys, key) = self.native_mcp_keys().await?;
        self.store
            .answer_mcp_task_interaction(
                &cowork_storage::mcp_oauth::McpOAuthOwner::Native { key },
                run,
                id,
                input,
                keys.as_ref(),
            )
            .await
    }
}
