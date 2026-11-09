//! Nested sampling uses only the original private actor/Role deployment.
use super::*;
use cowork_runtime::provider;
use cowork_storage::mcp_interactions::{
    McpInteractionExecution, McpInteractionRecord, ScopedTaskInteractions, sampling_messages,
};
use cowork_storage::mcp_tasks::McpTaskOperationPermit;
use futures_util::future::BoxFuture;
use tokio::sync::mpsc;
pub(super) struct PrivateTaskInteractions<'a> {
    pub service: &'a AndroidDeviceServices,
    pub lease: &'a cowork_storage::android_private_server::PrivateServerLease,
    pub sequence: i64,
    pub permit: &'a McpTaskOperationPermit,
    pub stop: &'a CancellationToken,
}
impl McpInteractionExecution for PrivateTaskInteractions<'_> {
    fn response_bearer(&self) -> BoxFuture<'_, Result<Option<zeroize::Zeroizing<String>>>> {
        Box::pin(async move {
            self.validate().await?;
            let frozen = self.lease.value["spec"]["input"]["frozen_mcp_bindings"]
                .as_array()
                .and_then(|a| {
                    a.iter()
                        .find(|f| f["entity_id"] == self.permit.task.view.server_id)
                })
                .ok_or(Error::Forbidden)?;
            let original = self
                .service
                .store
                .android_private_mcp_open_for_run(
                    self.lease.run,
                    self.lease.lease,
                    frozen,
                    self.service.keys.as_ref(),
                )
                .await?;
            let configuration = cowork_storage::android_private_mcp::configuration(&original)?;
            if let Some(oauth) = &configuration.oauth {
                if configuration.transport == "stdio" {
                    return Err(Error::Forbidden);
                }
                let owner = Uuid::parse_str(
                    self.lease.value["spec"]["creator_user_id"]
                        .as_str()
                        .ok_or_else(invalid)?,
                )
                .map_err(|_| invalid())?;
                let bearer = self
                    .service
                    .store
                    .mcp_oauth_bearer_for(
                        &cowork_storage::mcp_oauth::McpOAuthOwner::PersonalExecution {
                            owner,
                            run: self.lease.run,
                            lease: self.lease.lease,
                            key: self.service.key_id,
                        },
                        &oauth.id,
                        self.service.keys.as_ref(),
                    )
                    .await?;
                let current = self
                    .service
                    .store
                    .android_private_mcp_open_for_run(
                        self.lease.run,
                        self.lease.lease,
                        frozen,
                        self.service.keys.as_ref(),
                    )
                    .await?;
                if canonical_hash(&current) != canonical_hash(&original) {
                    return Err(Error::Forbidden);
                }
                self.validate().await?;
                Ok(Some(bearer))
            } else {
                self.validate().await?;
                Ok(None)
            }
        })
    }
    fn validate(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if self.stop.is_cancelled() {
                return Err(failed());
            }
            self.service
                .store
                .android_private_mcp_task_validate(
                    self.lease.run,
                    self.lease.lease,
                    self.sequence,
                    self.permit,
                    self.service.keys.as_ref(),
                    self.service.key_id,
                )
                .await
        })
    }
    fn sample<'a>(&'a self, r: &'a McpInteractionRecord) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.validate().await?;
            let cp = self
                .service
                .store
                .android_private_loop_checkpoint(
                    self.lease.run,
                    self.lease.lease,
                    self.service.keys.as_ref(),
                )
                .await?;
            let profile = self
                .service
                .store
                .android_private_server_profile(self.lease.run, self.lease.lease)
                .await?;
            let mut spec: RunSpec =
                serde_json::from_value(cp["spec"].clone()).map_err(|_| invalid())?;
            let mut messages = cp["messages"]
                .as_array()
                .ok_or_else(invalid)?
                .iter()
                .filter(|m| m.get("tool_calls").is_none() && m["role"] != "tool")
                .cloned()
                .collect::<Vec<_>>();
            let decision = r.decision.as_ref().ok_or(Error::Forbidden)?;
            messages.extend(sampling_messages(
                &r.view.original_request["params"],
                decision["edited_messages"].as_array().map(Vec::as_slice),
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
                .map_err(|_| invalid())?,
            );
            let registered =
                if cowork_storage::android_private_workflows::current_private_workflow_node()
                    .is_some()
                {
                    self.service
                        .store
                        .android_private_workflow_registered_model(
                            self.lease.run,
                            self.lease.lease,
                            &serde_json::from_value(cp["spec"].clone()).map_err(|_| invalid())?,
                            self.service.keys.as_ref(),
                        )
                        .await?
                } else {
                    self.service
                        .store
                        .android_private_registered_model(
                            self.lease.run,
                            self.lease.lease,
                            &serde_json::from_value(cp["spec"].clone()).map_err(|_| invalid())?,
                        )
                        .await?
                };
            let mut prepared =
                provider::prepare_registered(&spec, &messages, false, registered.as_ref())?;
            if let Some(p) = spec
                .provider
                .as_ref()
                .filter(|p| matches!(p.kind, cowork_contracts::ProviderKind::Codex))
            {
                let owner = Uuid::parse_str(profile["owner_user_id"].as_str().ok_or_else(invalid)?)
                    .map_err(|_| invalid())?;
                prepared.bind_codex_transport(
                    provider::codex_gateway::transport(
                        &self.service.store,
                        self.service.codex.clone(),
                        format!("actor:{owner}"),
                        p,
                    )
                    .await?,
                )?;
            }
            let attempt = format!("mcp-sampling:{}", r.view.id);
            prepared.bind_dispatch_id(&self.lease.run.to_string(), &attempt)?;
            let intent = json!({"format":1,"interaction_id":r.view.id,"task_operation_id":self.permit.id,"parent_checkpoint_sha256":canonical_hash(&cp),"provider_sha256":canonical_hash(&profile),"workflow_node":cp["workflow_node"],"spec":spec,"messages":messages,"logical_sha256":prepared.logical_fingerprint(),"wire_sha256":prepared.wire_sha256(),"context_measurement":prepared.context_measurement(),"request_payload_sha256":cowork_domain::hash(prepared.payload()),"attempt_id":attempt});
            self.service
                .store
                .admit_mcp_interaction_sampling(
                    self.permit,
                    &r.view.id,
                    &intent,
                    self.service.keys.as_ref(),
                    self.service.key_id,
                )
                .await?;
            let credential = self
                .service
                .store
                .android_private_mcp_sampling_admit(
                    self.lease.run,
                    self.lease.lease,
                    self.sequence,
                    self.permit,
                    &r.view.id,
                    &intent,
                    self.service.keys.as_ref(),
                    self.service.key_id,
                )
                .await?;
            if let Some(model) = registered.as_ref() {
                provider::assert_qualified_deployment(
                    &spec,
                    model,
                    credential.as_ref().map(|v| v.as_str()),
                )
                .await?;
            }
            self.service
                .store
                .android_private_mcp_sampling_validate(
                    self.lease.run,
                    self.lease.lease,
                    self.sequence,
                    self.permit,
                    &r.view.id,
                    &intent,
                    self.service.keys.as_ref(),
                    self.service.key_id,
                )
                .await?;
            self.validate().await?;
            if self.stop.is_cancelled() {
                return Err(failed());
            }
            let (tx, mut rx) = mpsc::channel(16);
            let observer = provider::ProviderReceiveObserver::evidence_only();
            let request = provider::complete_prepared_observed(
                prepared,
                credential.map(|v| v.to_string()),
                tx,
                observer.clone(),
            );
            tokio::pin!(request);
            let deadline = cp["deadline"]
                .as_i64()
                .ok_or_else(invalid)?
                .min(r.view.expires_at);
            let bound = tokio::time::Instant::now()
                + Duration::from_millis(
                    deadline
                        .saturating_sub(chrono::Utc::now().timestamp_millis())
                        .max(0) as u64,
                );
            let mut pulse = tokio::time::interval(Duration::from_millis(250));
            let reply = loop {
                tokio::select! {biased;_=self.stop.cancelled()=>return Err(failed()),_=tokio::time::sleep_until(bound)=>return Err(failed()),_=pulse.tick()=>{self.validate().await?;self.service.store.android_private_mcp_sampling_validate(self.lease.run,self.lease.lease,self.sequence,self.permit,&r.view.id,&intent,self.service.keys.as_ref(),self.service.key_id).await?;},Some(_)=rx.recv()=>{},result=&mut request=>break result?}
            };
            let model = spec
                .provider
                .as_ref()
                .ok_or(Error::Forbidden)?
                .model
                .clone();
            let response = json!({"result":{"role":"assistant","content":{"type":"text","text":reply.content},"model":model,"stopReason":"endTurn"},"usage":reply.usage,"tools":reply.tools,"intent_sha256":canonical_hash(&intent),"wire_evidence":observer.received_evidence()?,"cost_metadata":reply.received_cost_metadata});
            self.service
                .store
                .capture_mcp_interaction_sampling(
                    self.permit,
                    &r.view.id,
                    &response,
                    self.service.keys.as_ref(),
                    self.service.key_id,
                )
                .await?;
            self.service
                .store
                .android_private_mcp_sampling_received(self.lease.run, &r.view.id, &response)
                .await?;
            if !reply.tools.is_empty()
                || reply
                    .usage
                    .output_tokens
                    .is_some_and(|n| n > u64::from(spec.budgets.max_output_tokens))
            {
                return Err(failed());
            }
            self.validate().await?;
            Ok(())
        })
    }
}
pub(super) fn handler<'a>(
    execution: &'a PrivateTaskInteractions<'a>,
    deadline: i64,
) -> ScopedTaskInteractions<'a> {
    ScopedTaskInteractions {
        store: &execution.service.store,
        permit: execution.permit,
        keys: execution.service.keys.as_ref(),
        key: execution.service.key_id,
        execution,
        deadline,
    }
}
