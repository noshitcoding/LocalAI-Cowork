//! Human-approved Offline sampling uses only the original grant's own model.
use super::offline_mcp_tasks::OfflineTaskExecution;
use super::*;
use cowork_secrets::{CredentialScope, open_bearer};
use cowork_storage::mcp_interactions::{
    McpInteractionExecution, McpInteractionRecord, sampling_messages,
};
use futures_util::future::BoxFuture;
impl McpInteractionExecution for OfflineTaskExecution<'_> {
    fn validate(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(self.current())
    }
    fn sample<'a>(&'a self, record: &'a McpInteractionRecord) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.current().await?;
            let e = self.runtime.store.offline_local_execution(self.run).await?;
            let g = &e.ticket.grant;
            if g.device.id != self.key.instance
                || e.revoked
                || e.run.state != "running"
                || now() >= e.run.deadline_at
                || record.task.owner_kind != "offline"
                || record.task.view.run_id != self.run.to_string()
            {
                return Err(Error::Forbidden);
            }
            let decision = record.decision.as_ref().ok_or(Error::Forbidden)?;
            // Preserve the original complete parent dialogue. Exclude the current
            // unfinished multi-tool proposal and its partial replies as one unit.
            let parent = e
                .messages
                .iter()
                .rposition(|m| {
                    m["role"] == "assistant"
                        && m["tool_calls"].as_array().is_some_and(|calls| {
                            calls
                                .iter()
                                .any(|c| c["id"] == record.task.view.tool_call_id)
                        })
                })
                .ok_or(Error::Forbidden)?;
            let mut messages = e.messages[..parent].to_vec();
            messages.extend(sampling_messages(
                &record.view.original_request["params"],
                decision["edited_messages"].as_array().map(Vec::as_slice),
            )?);
            let max = record.view.original_request["params"]["maxTokens"]
                .as_u64()
                .ok_or(Error::Forbidden)?;
            let body = json!({"model":g.model,"messages":messages,"stream":false,"max_tokens":max});
            if serde_json::to_vec(&body).map_err(storage_error)?.len() > 3 * 1024 * 1024 {
                return Err(Error::ContextLimit);
            }
            let credential = match &e.credential {
                Some(value) => Some(
                    open_bearer(
                        self.keys,
                        &CredentialScope {
                            project_id: g.project_id.to_string(),
                            server_id: "offline-litellm/v1".into(),
                            reference: g.id.to_string(),
                            revision: 1,
                            endpoint: g.offline_base_url.clone(),
                        },
                        value,
                    )
                    .map_err(|_| Error::NativeKeyUnavailable)?,
                ),
                None if e.allow_no_key => None,
                None => return Err(Error::Forbidden),
            };
            let intent = json!({"format":1,"interaction_id":record.view.id,"task_record_id":record.task.view.id,"operation_id":self.permit.id,"source_sha256":self.source.source_sha256(),"ticket_sha256":canonical_hash(&e.ticket),"endpoint":g.offline_base_url,"model":g.model,"body":body,"wire_sha256":canonical_hash(&body),"remote_preferences_are_untrusted":true});
            self.runtime
                .store
                .admit_offline_mcp_sampling(
                    self.source,
                    self.permit,
                    &record.view.id,
                    &intent,
                    self.keys,
                    self.key,
                )
                .await?;
            self.current().await?;
            // The same original offline model-request allowance was charged in the TX.
            // No sampling preference can change endpoint/model, add tools or use a server key.
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(10))
                .build()
                .map_err(|_| Error::UncertainEffect)?;
            let mut request = client
                .post(format!(
                    "{}/chat/completions",
                    g.offline_base_url.trim_end_matches('/')
                ))
                .header("content-type", "application/json")
                .body(serde_json::to_vec(&body).map_err(storage_error)?);
            if let Some(key) = credential {
                request = request.bearer_auth(key.as_str())
            }
            self.current().await?;
            let receive = async {
                let mut response = request.send().await.map_err(|_| Error::UncertainEffect)?;
                let status = response.status();
                if response
                    .content_length()
                    .is_some_and(|n| n > 2 * 1024 * 1024)
                {
                    return Err(Error::ContextLimit);
                }
                let mut bytes = Vec::new();
                while let Some(chunk) =
                    response.chunk().await.map_err(|_| Error::UncertainEffect)?
                {
                    if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
                        return Err(Error::ContextLimit);
                    }
                    bytes.extend_from_slice(&chunk)
                }
                let value: Value =
                    serde_json::from_slice(&bytes).map_err(|_| Error::UncertainEffect)?;
                Ok((status, value))
            };
            tokio::pin!(receive);
            let end = tokio::time::Instant::now()
                + Duration::from_millis(self.deadline.saturating_sub(now()).max(0) as u64);
            let mut pulse = tokio::time::interval(Duration::from_millis(250));
            let (status, original) = loop {
                tokio::select! {biased;v=&mut receive=>break v?,_=self.stop.cancelled()=>return Err(Error::UncertainEffect),_=tokio::time::sleep_until(end)=>return Err(Error::UncertainEffect),_=pulse.tick()=>self.current().await?}
            };
            let message = &original["choices"][0]["message"];
            let content = message["content"].as_str();
            let valid = status.is_success()
                && message["role"] == "assistant"
                && content.is_some_and(|v| v.len() <= 1024 * 1024 && !v.contains('\0'))
                && message
                    .get("tool_calls")
                    .is_none_or(|v| v.as_array().is_some_and(Vec::is_empty));
            let protected = if valid {
                json!({"result":{"role":"assistant","content":{"type":"text","text":content},"model":g.model,"stopReason":"endTurn"},"original_provider_response":original,"wire_sha256":canonical_hash(&body),"source_sha256":self.source.source_sha256()})
            } else {
                json!({"error":{"code":-1,"message":"Original offline sampling response rejected"},"original_provider_response":original,"wire_sha256":canonical_hash(&body),"source_sha256":self.source.source_sha256()})
            };
            self.runtime
                .store
                .capture_mcp_interaction_sampling(
                    self.permit,
                    &record.view.id,
                    &protected,
                    self.keys,
                    self.key,
                )
                .await?;
            self.current().await?;
            if !valid {
                return Err(Error::UncertainEffect);
            }
            Ok(())
        })
    }
}
