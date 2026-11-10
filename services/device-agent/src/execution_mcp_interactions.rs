//! Original device run/model context; remote JSON never supplies execution authority.
use super::*;
use cowork_mcp::{TaskInteractionHandler, TaskServerRequest, TaskServerResponse};
use futures_util::future::BoxFuture;
#[derive(Clone)]
struct RunSource {
    spec: Value,
    lease: Uuid,
}
#[derive(Clone)]
struct ModelSource {
    model: String,
    messages: Vec<Value>,
}
tokio::task_local! {static ORIGINAL_RUN:RunSource;static ORIGINAL_MODEL:ModelSource;}
pub(in crate::execution) async fn with_run<F, T>(spec: Value, lease: Uuid, future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    ORIGINAL_RUN.scope(RunSource { spec, lease }, future).await
}
pub(in crate::execution) async fn with_model<F, T>(
    model: String,
    messages: Vec<Value>,
    future: F,
) -> T
where
    F: std::future::Future<Output = T>,
{
    ORIGINAL_MODEL
        .scope(ModelSource { model, messages }, future)
        .await
}
pub(super) fn project() -> Option<Uuid> {
    ORIGINAL_RUN
        .try_with(|v| uuid(&v.spec, "project_id").ok())
        .ok()
        .flatten()
}
pub(in crate::execution) fn sampling_available() -> bool {
    ORIGINAL_MODEL
        .try_with(|v| !v.model.is_empty() && !v.messages.is_empty())
        .unwrap_or(false)
}
pub(in crate::execution) fn model_budget(engine: &ExecutionEngine, run: Uuid) -> Result<()> {
    let mut count = 0usize;
    let prefix = format!("{run}-");
    let mut inspected = 0usize;
    for e in fs::read_dir(&engine.journal_directory)? {
        let path = e?.path();
        if !path.file_name().is_some_and(|n| {
            n.to_string_lossy().starts_with(&prefix) && n.to_string_lossy().ends_with(".json")
        }) {
            continue;
        }
        inspected += 1;
        ensure!(inspected <= 4096, "device_original_journal_bound");
        let mut file = crate::config::private_file(&path, 4 * 1024 * 1024)?;
        let value: Value = serde_json::from_reader(&mut file)?;
        if value["phase"] == "model_dispatched" {
            count += 1;
        }
    }
    ensure!(
        count < run_configuration::current(engine).budgets.max_steps as usize,
        "device_original_shared_model_step_limit"
    );
    Ok(())
}
pub(super) async fn current(
    control: &dyn DeviceControl,
    run: Uuid,
    lease: Uuid,
    deadline: i64,
    stop: &CancellationToken,
) -> Result<()> {
    ensure!(
        !stop.is_cancelled() && chrono::Utc::now().timestamp_millis() < deadline,
        "device_mcp_task_original_deadline_or_authority_lost"
    );
    ORIGINAL_RUN
        .try_with(|v| {
            ensure!(
                uuid(&v.spec, "id")? == run && v.lease == lease,
                "device_mcp_original_run_changed"
            );
            Ok::<_, anyhow::Error>(())
        })
        .context("device_mcp_original_run_context_missing")??;
    tokio::select! {_=stop.cancelled()=>bail!("device_run_canceled"),r=control.run_json(run,lease,"POST","heartbeat",json!({"lease_token":lease}))=>{r?;}}
    ensure!(
        !stop.is_cancelled() && chrono::Utc::now().timestamp_millis() < deadline,
        "device_mcp_task_original_deadline_or_authority_lost"
    );
    Ok(())
}
pub(super) struct Handler<'a> {
    pub engine: &'a ExecutionEngine,
    pub control: &'a dyn DeviceControl,
    pub run: Uuid,
    pub lease: Uuid,
    pub source: Uuid,
    pub session_id: String,
    pub epoch: u64,
    pub task_id: String,
    pub operation_rpc: String,
    pub operation_id: Uuid,
    pub journal: &'a journal::Journal,
    pub deadline: i64,
    pub stop: &'a CancellationToken,
}
impl Handler<'_> {
    async fn fresh(&self) -> Result<()> {
        current(self.control, self.run, self.lease, self.deadline, self.stop).await
    }
    async fn input(&self, id: Uuid, prompt: &Value, key: &str) -> Result<Value> {
        ensure!(
            serde_json::to_vec(prompt)?.len() <= 96 * 1024,
            "device_mcp_original_input_prompt_bound"
        );
        self.journal.put(
            &format!("{key}/input-intent"),
            "task_interaction_decision",
            &json!({"source":id,"prompt":prompt,"deadline":self.deadline}),
        )?;
        self.fresh().await?;
        let first=self.control.run_json(self.run,self.lease,"POST","input-requests",json!({"lease_token":self.lease,"source_request_id":id,"prompt":prompt,"expires_at":chrono::DateTime::from_timestamp_millis(self.deadline).context("device_mcp_deadline_invalid")?.to_rfc3339()})).await?;
        self.journal.put(
            &format!("{key}/input-created"),
            "task_interaction_decision",
            &first,
        )?;
        let input_id = uuid(&first, "id")?;
        loop {
            self.fresh().await?;
            let row = self
                .control
                .run_json(
                    self.run,
                    self.lease,
                    "GET",
                    &format!("input-requests/{input_id}"),
                    Value::Null,
                )
                .await?;
            ensure!(
                row["prompt"] == *prompt,
                "device_mcp_original_input_changed"
            );
            match row["state"].as_str() {
                Some("submitted") => {
                    self.journal.put(
                        &format!("{key}/input-received"),
                        "task_interaction_decision",
                        &row,
                    )?;
                    return Ok(row);
                }
                Some("pending") => {}
                _ => bail!("device_mcp_original_input_not_received"),
            };
            tokio::select! {_=self.stop.cancelled()=>bail!("device_run_canceled"),_=tokio::time::sleep(Duration::from_millis(500))=>{}}
        }
    }
    async fn decision_current(&self, row: &Value) -> Result<()> {
        self.fresh().await?;
        let id = uuid(row, "id")?;
        let current = self
            .control
            .run_json(
                self.run,
                self.lease,
                "GET",
                &format!("input-requests/{id}"),
                Value::Null,
            )
            .await?;
        ensure!(
            current["state"] == "submitted"
                && current["revision"] == row["revision"]
                && current["prompt"] == row["prompt"]
                && current["response"] == row["response"]
                && current["responded_by_user_id"] == row["responded_by_user_id"],
            "device_mcp_original_human_decision_changed"
        );
        Ok(())
    }
    async fn respond_inner(&self, request: TaskServerRequest) -> Result<TaskServerResponse> {
        self.fresh().await?;
        ensure!(
            request.connection_id() == self.session_id
                && request.connection_epoch() == self.epoch
                && request.task_id() == self.task_id
                && request.operation_rpc_id() == self.operation_rpc,
            "device_mcp_original_task_request_changed"
        );
        let key = format!(
            "interaction/{}",
            digest(&serde_json::to_vec(request.rpc_id())?)
        );
        self.journal.put(&format!("{key}/request"),"task_interaction_request",&json!({"wire":request.wire(),"request_sha256":request.request_sha256(),"operation_id":self.operation_id,"operation_rpc":self.operation_rpc,"connection_id":self.session_id,"connection_epoch":self.epoch,"task_id":self.task_id}))?;
        let sampling = request.method() == "sampling/createMessage";
        if sampling {
            run_configuration::check_deadline()?;
            cowork_mcp::sampling_text_messages(request.params(), None)
                .map_err(|_| anyhow::anyhow!("device_mcp_sampling_request_unsupported"))?;
            ensure!(
                sampling_available(),
                "device_mcp_original_model_context_missing"
            );
        } else {
            let schema = &request.params()["requestedSchema"];
            ensure!(
                schema["type"] == "object" && serde_json::to_vec(schema)?.len() <= 32768,
                "device_mcp_form_invalid"
            );
        }
        let original_view = self
            .engine
            .bounded_payload(
                self.control,
                self.run,
                self.lease,
                request.wire().clone(),
                48 * 1024,
            )
            .await?;
        let referenced = original_view["format"] == "device-original-payload-reference/1";
        let prompt = json!({"format":"device-mcp-task-interaction/1","question":if sampling{"Review the original MCP request and explicitly approve one use of this device's original model. You may edit its text messages. The generated response requires a second approval before sending."}else{"Review the original MCP form. Choose accept, decline or cancel; accept requires your explicitly reviewed form values. Never enter credentials or payment data."},"method":request.method(),"task_record_id":self.source,"connection":self.session_id,"request_sha256":request.request_sha256(),"original_request":if referenced{Value::Null}else{original_view.clone()},"original_request_reference":if referenced{original_view}else{Value::Null},"response_format":if sampling{json!({"action":"approve_sampling|decline_sample","edited_messages":"optional original MCP text-message array"})}else{json!({"action":"accept|decline|cancel","content":"form object when accepted"})}});
        let decision = self
            .input(Uuid::new_v4(), &prompt, &format!("{key}/decision"))
            .await?;
        let answer: Value = serde_json::from_str(
            decision["response"]
                .as_str()
                .context("device_mcp_human_answer_invalid")?,
        )
        .context("device_mcp_human_answer_invalid")?;
        let meta = json!({"io.modelcontextprotocol/related-task":{"taskId":self.task_id}});
        let (body, last_decision) = if !sampling {
            let action = answer["action"]
                .as_str()
                .context("device_mcp_human_action_missing")?;
            ensure!(
                matches!(action, "accept" | "decline" | "cancel"),
                "device_mcp_human_action_invalid"
            );
            let mut result = json!({"action":action,"_meta":meta});
            if action == "accept" {
                cowork_mcp::validate_task_form(request.params(), &answer["content"])
                    .map_err(|_| anyhow::anyhow!("device_mcp_human_form_invalid"))?;
                result["content"] = answer["content"].clone();
            } else {
                ensure!(
                    answer.get("content").is_none_or(Value::is_null),
                    "device_mcp_declined_content_invalid"
                );
            }
            (json!({"result":result}), decision)
        } else if answer["action"] == "decline_sample" {
            (
                json!({"error":{"code":-1,"message":"User declined sampling","data":{"_meta":meta}}}),
                decision,
            )
        } else {
            ensure!(
                answer["action"] == "approve_sampling",
                "device_mcp_human_action_invalid"
            );
            let original = ORIGINAL_MODEL
                .try_with(Clone::clone)
                .context("device_mcp_original_model_context_missing")?;
            let edited = answer
                .get("edited_messages")
                .filter(|v| !v.is_null())
                .map(|v| v.as_array().context("device_mcp_human_messages_invalid"))
                .transpose()?;
            let mut messages = original
                .messages
                .iter()
                .filter(|m| m.get("tool_calls").is_none() && m["role"] != "tool")
                .cloned()
                .collect::<Vec<_>>();
            messages.extend(
                cowork_mcp::sampling_text_messages(request.params(), edited.map(Vec::as_slice))
                    .map_err(|_| anyhow::anyhow!("device_mcp_human_messages_invalid"))?,
            );
            let model = self
                .engine
                .config
                .model
                .as_ref()
                .context("device_own_model_not_configured")?;
            let endpoint = self
                .engine
                .model_endpoint
                .as_ref()
                .context("device_own_model_not_configured")?;
            let mut body = json!({"model":original.model,"messages":messages,"stream":false,"max_tokens":request.params()["maxTokens"].as_u64().context("device_mcp_max_tokens_missing")?.min(65536)});
            let settings = run_configuration::current(self.engine);
            let reserved_tokens = settings.apply(&mut body, model.api == "ollama")?;
            ensure!(
                serde_json::to_vec(&body)?.len() <= 2 * 1024 * 1024,
                "device_mcp_original_model_context_bound"
            );
            let sampling_id = Uuid::new_v4();
            let approval = json!({"format":"device-mcp-sampling/1","original_server_request_sha256":request.request_sha256(),"original_task_record_id":self.source,"original_model":original.model,"original_role_context_sha256":digest(&serde_json::to_vec(&original.messages)?),"endpoint":endpoint.as_str(),"body":body,"automatic_replay":false});
            ensure!(
                self.engine
                    .approval(
                        self.control,
                        self.run,
                        self.lease,
                        sampling_id,
                        approval,
                        self.stop
                    )
                    .await?,
                "device_mcp_sampling_declined"
            );
            self.decision_current(&decision).await?;
            model_budget(self.engine, self.run)?;
            settings.reserve(self.engine, self.run, reserved_tokens)?;
            self.journal.put(&format!("{key}/model-intent"),"task_interaction_model_intent",&json!({"body":body,"endpoint":endpoint.as_str(),"decision":decision,"source_request_id":sampling_id,"model_api":model.api,"original_context":original.messages}))?;
            self.engine.admit(self.run,&format!("mcp-sampling/{sampling_id}"),json!({"phase":"model_dispatched","scope":"mcp-task-sampling","request_sha256":digest(&serde_json::to_vec(&body)?),"reserved_tokens":reserved_tokens}))?;
            self.decision_current(&decision).await?;
            ensure!(!self.stop.is_cancelled(), "device_run_canceled");
            run_configuration::check_deadline()?;
            let wire = body.clone();
            let timeout = Duration::from_millis(
                settings
                    .agent
                    .generation
                    .timeout_ms
                    .unwrap_or(model.timeout_seconds.saturating_mul(1000))
                    .min(model.timeout_seconds.saturating_mul(1000)),
            );
            let timeout = settings
                .deadline
                .map(|deadline| {
                    timeout.min(deadline.saturating_duration_since(tokio::time::Instant::now()))
                })
                .unwrap_or(timeout);
            let mut sending = self
                .engine
                .model_http
                .post(endpoint.clone())
                .timeout(timeout)
                .json(&wire);
            if let Some(token) = &model.api_key {
                sending = sending.bearer_auth(token);
            }
            let mut response = tokio::select! {_=self.stop.cancelled()=>bail!("device_mcp_sampling_effect_unknown"),r=sending.send()=>r.context("device_mcp_sampling_send_unknown_no_replay")?};
            let status = response.status().as_u16();
            let mut raw = Vec::new();
            loop {
                let chunk = tokio::select! {_=self.stop.cancelled()=>bail!("device_mcp_sampling_receive_unknown"),r=response.chunk()=>r?};
                let Some(chunk) = chunk else { break };
                ensure!(
                    raw.len() + chunk.len() <= 1024 * 1024,
                    "device_mcp_sampling_original_response_bound"
                );
                raw.extend_from_slice(&chunk);
            }
            let value: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
            self.journal.put(&format!("{key}/model-received"),"task_interaction_model_response",&json!({"http_status":status,"original_provider_response":value,"original_provider_bytes_hex":raw.iter().map(|v|format!("{v:02x}")).collect::<String>(),"original_request_sha256":digest(&serde_json::to_vec(&wire)?)}))?;
            ensure!(
                (200..300).contains(&status),
                "device_mcp_sampling_provider_error_original_retained"
            );
            let message = if model.api == "ollama" {
                &value["message"]
            } else {
                &value["choices"][0]["message"]
            };
            ensure!(
                message["tool_calls"].as_array().is_none_or(Vec::is_empty),
                "device_mcp_sampling_remote_tools_forbidden"
            );
            let text = message["content"]
                .as_str()
                .context("device_mcp_sampling_original_text_missing")?;
            let generated = json!({"role":"assistant","content":{"type":"text","text":text},"model":original.model,"stopReason":"endTurn","_meta":meta});
            let generated_view = self
                .engine
                .bounded_payload(
                    self.control,
                    self.run,
                    self.lease,
                    generated.clone(),
                    48 * 1024,
                )
                .await?;
            let referenced = generated_view["format"] == "device-original-payload-reference/1";
            let release_prompt = json!({"format":"device-mcp-task-release/1","question":"Review this exact generated model response. Release sends it to the original MCP server; decline_sample withholds it. No additional model invocation is permitted.","task_record_id":self.source,"request_sha256":request.request_sha256(),"generated_response":if referenced{Value::Null}else{generated_view.clone()},"generated_response_reference":if referenced{generated_view}else{Value::Null},"response_format":{"action":"release_sample|decline_sample"}});
            let release = self
                .input(Uuid::new_v4(), &release_prompt, &format!("{key}/release"))
                .await?;
            let answer: Value = serde_json::from_str(
                release["response"]
                    .as_str()
                    .context("device_mcp_human_release_invalid")?,
            )?;
            ensure!(
                matches!(
                    answer["action"].as_str(),
                    Some("release_sample" | "decline_sample")
                ),
                "device_mcp_human_release_invalid"
            );
            if answer["action"] == "release_sample" {
                (json!({"result":generated}), release)
            } else {
                (
                    json!({"error":{"code":-1,"message":"User withheld sampling response","data":{"_meta":meta}}}),
                    release,
                )
            }
        };
        self.decision_current(&last_decision).await?;
        let receipt_id = Uuid::new_v4().to_string();
        self.journal.put(&format!("response/{receipt_id}/intent"),"task_interaction_response",&json!({"operation_id":self.operation_id,"operation_rpc":self.operation_rpc,"original_request_sha256":request.request_sha256(),"body":body,"decision":last_decision}))?;
        TaskServerResponse::prepared(receipt_id, body)
            .map_err(|_| anyhow::anyhow!("device_mcp_response_invalid"))
    }
}
fn transport_error(_: anyhow::Error) -> cowork_mcp::Error {
    cowork_mcp::Error::SessionUnavailable
}
impl TaskInteractionHandler for Handler<'_> {
    fn respond<'a>(
        &'a self,
        request: TaskServerRequest,
    ) -> BoxFuture<'a, cowork_mcp::Result<TaskServerResponse>> {
        Box::pin(async move { self.respond_inner(request).await.map_err(transport_error) })
    }
    fn validate_response<'a>(
        &'a self,
        response: &'a TaskServerResponse,
    ) -> BoxFuture<'a, cowork_mcp::Result<()>> {
        Box::pin(async move {
            self.fresh().await.map_err(transport_error)?;
            let value = self
                .journal
                .exact(
                    &format!("response/{}/intent", response.receipt_id()),
                    "task_interaction_response",
                )
                .map_err(transport_error)?;
            if value["body"] != *response.body()
                || value["operation_id"] != self.operation_id.to_string()
                || value["operation_rpc"] != self.operation_rpc
            {
                return Err(cowork_mcp::Error::SessionUnavailable);
            }
            self.decision_current(&value["decision"])
                .await
                .map_err(transport_error)
        })
    }
    fn acknowledge<'a>(
        &'a self,
        response: &'a TaskServerResponse,
        receipt: Value,
    ) -> BoxFuture<'a, cowork_mcp::Result<()>> {
        Box::pin(async move {
            self.journal.put(&format!("response/{}/received",response.receipt_id()),"task_interaction_response",&json!({"response_sha256":digest(&serde_json::to_vec(response.body()).map_err(|_|cowork_mcp::Error::Protocol)?),"transport_receipt":receipt,"remote_effects_unknown":true})).map(|_|()).map_err(transport_error)
        })
    }
    fn unknown<'a>(
        &'a self,
        response: &'a TaskServerResponse,
    ) -> BoxFuture<'a, cowork_mcp::Result<()>> {
        Box::pin(async move {
            self.journal
                .put(
                    &format!("response/{}/unknown", response.receipt_id()),
                    "task_interaction_response",
                    &json!({"outcome_unknown":true,"automatic_replay":false}),
                )
                .map(|_| ())
                .map_err(transport_error)
        })
    }
}
