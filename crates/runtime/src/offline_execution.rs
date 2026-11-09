//! Native offline execution uses only signed snapshots and the local key vault.
//! It never calls the Cowork teamserver or creates server credentials/principals.
use super::*;
use cowork_secrets::{CredentialScope, KeyId, open_bearer, seal_bearer};
use cowork_storage::offline_execution::{self as offline, OfflineLocalDeviceRecord};
use zeroize::Zeroizing;
fn json_string(value: &impl Serialize) -> Result<String> {
    serde_json::to_string(value).map_err(storage_error)
}

fn credential_scope(g: &OfflineGrant) -> CredentialScope {
    CredentialScope {
        project_id: g.project_id.to_string(),
        server_id: "offline-litellm/v1".into(),
        reference: g.id.to_string(),
        revision: 1,
        endpoint: g.offline_base_url.clone(),
    }
}
#[derive(Default)]
pub(crate) struct OfflineWorkers {
    stop: CancellationToken,
    tasks: std::sync::Mutex<JoinSet<()>>,
    tokens: std::sync::Mutex<std::collections::BTreeMap<uuid::Uuid, CancellationToken>>,
    pub(super) services:
        std::sync::Mutex<std::collections::BTreeMap<uuid::Uuid, Arc<cowork_toolbox::Toolbox>>>,
}
impl Runtime {
    pub(super) async fn offline_keys(
        &self,
        record: &OfflineLocalDeviceRecord,
    ) -> Result<Arc<dyn cowork_secrets::NativeKeyManager>> {
        if !self.store.is_local_sqlite() {
            return Err(Error::Forbidden);
        }
        self.native_keys
            .as_ref()
            .ok_or(Error::NativeKeyUnavailable)?
            .open(record.key_id.instance)
            .map_err(|_| Error::NativeKeyUnavailable)
    }
    pub async fn offline_device(&self) -> Result<OfflineDevice> {
        if !self.store.is_local_sqlite() {
            return Err(Error::Forbidden);
        }
        let record = match self.store.offline_local_device().await? {
            Some(v) => v,
            None => {
                let key_id = KeyId {
                    instance: uuid::Uuid::new_v4(),
                    key: uuid::Uuid::new_v4(),
                };
                self.store
                    .save_offline_local_device(OfflineLocalDeviceRecord {
                        device: OfflineDevice {
                            id: key_id.instance,
                            public_key: "pending".into(),
                            name: "Native Offline-Gerät".into(),
                        },
                        key_id,
                    })
                    .await?
            }
        };
        let manager = self.offline_keys(&record).await?;
        let key_id = record.key_id;
        let pending = record.device.public_key == "pending";
        let master = tokio::task::spawn_blocking(move || match manager.load(&key_id) {
            Ok(value) => Ok(value),
            Err(cowork_secrets::Error::Missing) if pending => {
                manager.create(&key_id)?;
                manager.load(&key_id)
            }
            Err(e) => Err(e),
        })
        .await
        .map_err(storage_error)?
        .map_err(|_| Error::NativeKeyUnavailable)?;
        let key = offline::offline_key(master.as_ref(), "device", record.device.id)?;
        let public = offline::offline_public_key(&key);
        if record.device.public_key == "pending" {
            let value = OfflineLocalDeviceRecord {
                device: OfflineDevice {
                    public_key: public,
                    ..record.device
                },
                key_id,
            };
            return Ok(self
                .store
                .activate_offline_local_device(value)
                .await?
                .device);
        }
        if record.device.public_key != public {
            return Err(Error::NativeKeyUnavailable);
        }
        Ok(record.device)
    }
    pub(super) async fn offline_signer(
        &self,
    ) -> Result<(OfflineLocalDeviceRecord, ring::signature::Ed25519KeyPair)> {
        self.offline_device().await?;
        let record = self
            .store
            .offline_local_device()
            .await?
            .ok_or(Error::NativeKeyUnavailable)?;
        let manager = self.offline_keys(&record).await?;
        let key_id = record.key_id;
        let master = tokio::task::spawn_blocking(move || manager.load(&key_id))
            .await
            .map_err(storage_error)?
            .map_err(|_| Error::NativeKeyUnavailable)?;
        let signer = offline::offline_key(master.as_ref(), "device", record.device.id)?;
        Ok((record, signer))
    }
    pub async fn offline_device_proof(
        &self,
        request: OfflineDeviceProofRequest,
    ) -> Result<OfflineDeviceProof> {
        offline::validate_offline_origin(&request.issuer_origin)?;
        valid_text(&request.idempotency_key, 128)?;
        let (record, key) = self.offline_signer().await?;
        Ok(OfflineDeviceProof {
            signature: offline::offline_sign(
                &key,
                "device-proof",
                &(record.device.clone(), request.clone()),
            )?,
            device: record.device,
            request,
        })
    }
    pub async fn offline_install(&self, input: InstallOfflineGrant) -> Result<SignedOfflineGrant> {
        let device = self.offline_device().await?;
        self.store.install_offline_grant(input, &device).await
    }
    pub async fn offline_configure_credential(
        &self,
        input: ConfigureOfflineCredential,
    ) -> Result<()> {
        let (record, _) = self.offline_signer().await?;
        let ticket = self.store.offline_local_ticket(input.grant_id).await?;
        let g = &ticket.grant;
        if g.device != record.device {
            return Err(Error::Forbidden);
        }
        let credential = if let Some(value) = input.api_key {
            if input.explicitly_allow_no_key {
                return Err(Error::Forbidden);
            }
            let secret = Zeroizing::new(value);
            let manager = self.offline_keys(&record).await?;
            Some(
                seal_bearer(
                    manager.as_ref(),
                    record.key_id,
                    &credential_scope(g),
                    &secret,
                )
                .map_err(|_| Error::NativeKeyUnavailable)?,
            )
        } else {
            None
        };
        self.store
            .set_offline_local_credential(input.grant_id, credential, input.explicitly_allow_no_key)
            .await
    }
    pub async fn offline_start(&self, input: StartOfflineRun) -> Result<OfflineRun> {
        self.offline_signer().await?;
        let (run, start) = self.store.admit_offline_run(input).await?;
        if start {
            self.spawn_offline(run.id);
        }
        Ok(run)
    }
    fn spawn_offline(&self, id: uuid::Uuid) {
        let runtime = self.clone();
        let stop = self.offline_workers.stop.child_token();
        let Ok(mut tokens) = self.offline_workers.tokens.lock() else {
            return;
        };
        if tokens.contains_key(&id) {
            return;
        }
        tokens.insert(id, stop.clone());
        drop(tokens);
        let mut tasks = self
            .offline_workers
            .tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        while tasks.try_join_next().is_some() {}
        tasks.spawn(async move {
            let result=tokio::select!{_=stop.cancelled()=>Err(Error::UncertainEffect),result=runtime.execute_offline(id,&stop)=>result};
            if let Err(error) = result {
                let _ = runtime.finish_offline_unknown(id, error.code()).await;
            }
            let keep=runtime.store.offline_local_execution(id).await.is_ok_and(|e|e.run.state=="awaiting_approval");
            if !keep {
                let service=runtime.offline_workers.services.lock().unwrap_or_else(|e|e.into_inner()).remove(&id);
                if let Some(service)=service {let _=tokio::task::spawn_blocking(move||service.shutdown()).await;}
            }
            runtime.offline_workers.tokens.lock().unwrap_or_else(|e|e.into_inner()).remove(&id);
        });
    }
    pub async fn shutdown_offline(&self) {
        self.offline_workers.stop.cancel();
        let mut tasks = std::mem::take(
            &mut *self
                .offline_workers
                .tasks
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        );
        while tasks.join_next().await.is_some() {}
        let services = std::mem::take(
            &mut *self
                .offline_workers
                .services
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        );
        for (_, service) in services {
            let _ = tokio::task::spawn_blocking(move || service.shutdown()).await;
        }
    }
    pub async fn offline_cancel(&self, id: uuid::Uuid) -> Result<OfflineRun> {
        if let Some(stop) = self
            .offline_workers
            .tokens
            .lock()
            .map_err(|_| Error::Conflict)?
            .get(&id)
        {
            stop.cancel();
        }
        let mut e = self.store.offline_local_execution(id).await?;
        if !matches!(e.run.state.as_str(), "running" | "awaiting_approval") {
            return Ok(e.run);
        }
        let old = e.run.revision;
        let stamp = now().max(
            e.run
                .journal
                .last()
                .map(|j| j.at)
                .unwrap_or(e.run.started_at),
        );
        let uncertain = e.run.state == "running";
        e.run.state = if uncertain { "unknown" } else { "cancelled" }.into();
        e.run.finished_at = Some(stamp);
        e.run.error = Some(
            if uncertain {
                "offline_cancelled_effect_unconfirmed_no_replay"
            } else {
                "offline_cancelled_before_tool_dispatch"
            }
            .into(),
        );
        e.run.revision += 1;
        offline::append_offline_journal(
            &mut e.run,
            "cancel_requested",
            json!({"effect_unconfirmed":uncertain,"automatic_replay":false}),
            stamp,
        )?;
        self.store
            .save_offline_run(&e.run, &e.messages, old)
            .await?;
        while self
            .offline_workers
            .tokens
            .lock()
            .map_err(|_| Error::Conflict)?
            .contains_key(&id)
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let service = self
            .offline_workers
            .services
            .lock()
            .map_err(|_| Error::Conflict)?
            .remove(&id);
        if let Some(service) = service {
            tokio::task::spawn_blocking(move || service.shutdown())
                .await
                .map_err(storage_error)?;
        }
        Ok(e.run)
    }
    async fn finish_offline_unknown(&self, id: uuid::Uuid, code: &str) -> Result<()> {
        let mut e = self.store.offline_local_execution(id).await?;
        if e.run.state != "running" {
            return Ok(());
        }
        let old = e.run.revision;
        let stamp = now().max(
            e.run
                .journal
                .last()
                .map(|j| j.at)
                .unwrap_or(e.run.started_at),
        );
        e.run.state = "unknown".into();
        e.run.error = Some(format!("{code}:offline_result_unconfirmed_no_replay"));
        e.run.finished_at = Some(stamp);
        e.run.revision += 1;
        offline::append_offline_journal(
            &mut e.run,
            "unknown",
            json!({"code":code,"automatic_replay":false}),
            stamp,
        )?;
        self.store.save_offline_run(&e.run, &e.messages, old).await
    }
    async fn execute_offline(&self, id: uuid::Uuid, stop: &CancellationToken) -> Result<()> {
        self.execute_offline_pending(id, stop).await?;
        let first = self.store.offline_local_execution(id).await?;
        let remaining = first.run.deadline_at - now();
        if remaining <= 0 {
            return Err(Error::Forbidden);
        }
        let deadline = tokio::time::Instant::now() + Duration::from_millis(remaining as u64);
        let mut e = self.store.offline_local_execution(id).await?;
        let g = e.ticket.grant.clone();
        offline::validate_offline_ticket(&e.ticket)?;
        if e.run.state != "running" {
            return Ok(());
        }
        if e.revoked {
            return Err(Error::Forbidden);
        }
        if now()
            < e.run
                .journal
                .last()
                .map(|j| j.at)
                .unwrap_or(e.run.started_at)
            || now() >= e.run.deadline_at
            || tokio::time::Instant::now() >= deadline
            || e.run.model_requests >= g.maximum_model_requests
        {
            return Err(Error::Forbidden);
        }
        let (device, _) = self.offline_signer().await?;
        if device.device != g.device {
            return Err(Error::Forbidden);
        }
        let credential = if let Some(sealed) = e.credential {
            let manager = self.offline_keys(&device).await?;
            Some(
                open_bearer(manager.as_ref(), &credential_scope(&g), &sealed)
                    .map_err(|_| Error::NativeKeyUnavailable)?,
            )
        } else if e.allow_no_key {
            None
        } else {
            return Err(Error::Provider("offline_model_key_not_configured".into()));
        };
        let mut body = json!({"model":g.model,"messages":e.messages,"stream":false});
        if let Some(output) = &g.output {
            body["response_format"] = cowork_domain::structured_output::openai_format(output)?;
        }
        let configuration = self.store.offline_tool_configuration(g.id).await?;
        let allowed = crate::offline_tools::configured_tools(&g, configuration.as_ref());
        if !allowed.is_empty() {
            body["tools"] = Value::Array(
                allowed
                    .iter()
                    .map(|n| tool_definition(n, configuration.as_ref()))
                    .collect(),
            );
        }
        let expected = e.run.revision;
        let at = now();
        e.run.model_requests += 1;
        e.run.revision += 1;
        offline::append_offline_journal(
            &mut e.run,
            "model_dispatched",
            json!({"request_sha256":canonical_hash(&body),"endpoint":g.offline_base_url,"model":g.model,"source_sha256":g.source_sha256}),
            at,
        )?;
        self.store
            .save_offline_run(&e.run, &e.messages, expected)
            .await?;
        // All authority and the dispatch receipt are committed before opening the socket.
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| Error::Provider("offline_client_unavailable".into()))?;
        let url = format!(
            "{}/chat/completions",
            g.offline_base_url.trim_end_matches('/')
        );
        let mut request = client
            .post(url)
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&body).map_err(storage_error)?);
        if let Some(secret) = credential.as_ref() {
            request = request.bearer_auth(secret.as_str());
        }
        let received = tokio::time::timeout_at(deadline, async {
            let mut response = request.send().await.map_err(|_| {
                Error::Provider("offline_model_endpoint_unavailable_or_unconfirmed".into())
            })?;
            if !response.status().is_success()
                || response
                    .content_length()
                    .is_some_and(|v| v > 2 * 1024 * 1024)
            {
                return Err(Error::Provider("offline_model_response_rejected".into()));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| Error::Provider("offline_model_receive_interrupted".into()))?
            {
                if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
                    return Err(Error::Provider("offline_model_response_too_large".into()));
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(bytes)
        })
        .await
        .map_err(|_| Error::Provider("offline_original_deadline_expired".into()))??;
        let value: Value = serde_json::from_slice(&received)
            .map_err(|_| Error::Provider("offline_model_json_invalid".into()))?;
        let mut message = value
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|v| v.first())
            .and_then(|v| v.get("message"))
            .filter(|v| v.is_object())
            .cloned()
            .ok_or(Error::Provider("offline_model_message_missing".into()))?;
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            return Err(Error::Provider("offline_model_role_invalid".into()));
        }
        let text = message.get("content").and_then(Value::as_str).unwrap_or("");
        if text.len() > 1024 * 1024 || text.contains('\0') {
            return Err(Error::Provider("offline_model_content_too_large".into()));
        }
        e = self.store.offline_local_execution(id).await?;
        if e.run.state != "running" {
            return Err(Error::Conflict);
        }
        let old = e.run.revision;
        let at = now();
        e.run.revision += 1;
        let model_requests = e.run.model_requests;
        let received_payload = json!({"response_sha256":hash(&received),"model_request":model_requests,"request_sha256":canonical_hash(&body),"output_contract_sha256":e.run.output_contract_sha256,"usage":value.get("usage")});
        offline::append_offline_journal(&mut e.run, "model_received", received_payload, at)?;
        let calls = message
            .get("tool_calls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if calls.len() > 8 {
            return Err(Error::Provider("offline_tool_limit".into()));
        }
        let mut ids = std::collections::BTreeSet::new();
        for prior in &e.messages {
            if let Some(calls) = prior.get("tool_calls").and_then(Value::as_array) {
                for call in calls {
                    if let Some(id) = call.get("id").and_then(Value::as_str) {
                        ids.insert(id.to_owned());
                    }
                }
            }
        }
        for call in calls {
            let tool = call.get("function").ok_or(Error::Forbidden)?;
            let name = tool
                .get("name")
                .and_then(Value::as_str)
                .ok_or(Error::Forbidden)?
                .to_owned();
            let call_id = call
                .get("id")
                .and_then(Value::as_str)
                .ok_or(Error::Forbidden)?
                .to_owned();
            valid_text(&call_id, 160)?;
            if !allowed.contains(&name) || !ids.insert(call_id.clone()) {
                return Err(Error::Forbidden);
            }
            let arguments = tool
                .get("arguments")
                .and_then(Value::as_str)
                .ok_or(Error::Forbidden)?;
            if arguments.len() > 300 * 1024 {
                return Err(Error::Forbidden);
            }
            let arguments: Value = serde_json::from_str(arguments).map_err(|_| Error::Forbidden)?;
            let request_sha256 = canonical_hash(&(
                g.id,
                id,
                &call_id,
                &name,
                &arguments,
                canonical_hash(&configuration),
            ));
            e.run.pending_tools.push(OfflinePendingTool {
                id: call_id,
                name,
                arguments,
                request_sha256,
            });
        }
        // Keep only actual renderer/model fields, never response-side headers or authority.
        message
            .as_object_mut()
            .ok_or(Error::Forbidden)?
            .retain(|k, _| matches!(k.as_str(), "role" | "content" | "tool_calls"));
        e.messages.push(message.clone());
        if !e.run.pending_tools.is_empty() {
            e.run.state = "awaiting_approval".into();
            e.run.response = message
                .get("content")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let proposed = serde_json::to_value(&e.run.pending_tools).map_err(storage_error)?;
            offline::append_offline_journal(&mut e.run, "tools_proposed", proposed, at)?;
        } else {
            e.run.response = message
                .get("content")
                .and_then(Value::as_str)
                .map(str::to_owned);
            e.run.finished_at = Some(at);
            let response_sha256 = canonical_hash(&e.run.response);
            if let Some(error) = g.output.as_ref().and_then(|output| {
                cowork_domain::structured_output::validate_result(
                    output,
                    e.run.response.as_deref().unwrap_or(""),
                )
                .err()
            }) {
                e.run.state = "failed".into();
                let code = match error {
                    Error::Validation(code) => code,
                    error => error.code().into(),
                };
                e.run.error = Some(code.clone());
                let contract = e.run.output_contract_sha256.clone();
                offline::append_offline_journal(
                    &mut e.run,
                    "model_response_rejected",
                    json!({"code":code,"response_sha256":response_sha256,"output_contract_sha256":contract,"automatic_replay":false}),
                    at,
                )?;
            } else {
                e.run.state = "completed".into();
                offline::append_offline_journal(
                    &mut e.run,
                    "completed",
                    json!({"response_sha256":response_sha256}),
                    at,
                )?;
            }
        }
        self.store
            .save_offline_run(&e.run, &e.messages, old)
            .await?;
        Ok(())
    }
    pub async fn offline_approve_tools(
        &self,
        id: uuid::Uuid,
        input: ApproveOfflineTools,
    ) -> Result<OfflineRun> {
        let initial = self.store.offline_local_execution(id).await?;
        if initial.run.state != "awaiting_approval"
            || initial.run.revision != input.expected_revision
        {
            return Err(Error::Conflict);
        }
        // The model worker commits awaiting_approval before it finishes. Join its
        // reservation before starting the next owned worker for this same run.
        while self
            .offline_workers
            .tokens
            .lock()
            .map_err(|_| Error::Conflict)?
            .contains_key(&id)
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let mut e = self.store.offline_local_execution(id).await?;
        let g = &e.ticket.grant;
        if e.revoked
            || e.run.state != "awaiting_approval"
            || e.run.revision != input.expected_revision
            || input.request_sha256
                != e.run
                    .pending_tools
                    .iter()
                    .map(|t| t.request_sha256.clone())
                    .collect::<Vec<_>>()
        {
            return Err(Error::Conflict);
        }
        let old = e.run.revision;
        let at = now();
        if at
            < e.run
                .journal
                .last()
                .map(|j| j.at)
                .unwrap_or(e.run.started_at)
            || at >= e.run.deadline_at
            || at >= g.expires_at
        {
            return Err(Error::Forbidden);
        }
        self.offline_signer().await?;
        e.run.revision += 1;
        if !input.approve {
            e.run.state = "cancelled".into();
            e.run.finished_at = Some(at);
            e.run.error = Some("offline_tool_confirmation_declined".into());
            offline::append_offline_journal(
                &mut e.run,
                "tools_declined",
                json!({"automatic_replay":false}),
                at,
            )?;
        } else {
            let requests = e
                .run
                .pending_tools
                .iter()
                .map(|t| t.request_sha256.clone())
                .collect::<Vec<_>>();
            offline::append_offline_journal(
                &mut e.run,
                "tools_approved",
                json!({"request_sha256":requests}),
                at,
            )?;
            e.run.state = "running".into();
        }
        self.store
            .save_offline_run(&e.run, &e.messages, old)
            .await?;
        if input.approve {
            self.spawn_offline(id);
        } else {
            let service = self
                .offline_workers
                .services
                .lock()
                .map_err(|_| Error::Conflict)?
                .remove(&id);
            if let Some(service) = service {
                tokio::task::spawn_blocking(move || service.shutdown())
                    .await
                    .map_err(storage_error)?;
            }
        }
        Ok(e.run)
    }
    async fn execute_offline_pending(
        &self,
        id: uuid::Uuid,
        stop: &CancellationToken,
    ) -> Result<()> {
        loop {
            let mut e = self.store.offline_local_execution(id).await?;
            let Some(tool) = e.run.pending_tools.first().cloned() else {
                return Ok(());
            };
            if e.run.state != "running"
                || e.revoked
                || now() >= e.run.deadline_at
                || stop.is_cancelled()
            {
                return Err(Error::Forbidden);
            }
            let configuration = self
                .store
                .offline_tool_configuration(e.ticket.grant.id)
                .await?;
            let expected_hash = canonical_hash(&(
                e.ticket.grant.id,
                id,
                &tool.id,
                &tool.name,
                &tool.arguments,
                canonical_hash(&configuration),
            ));
            let legacy_safe = configuration.is_none()
                && matches!(
                    tool.name.as_str(),
                    "list_sources" | "read_source" | "create_artifact"
                )
                && tool.request_sha256
                    == canonical_hash(&(
                        e.ticket.grant.id,
                        id,
                        &tool.id,
                        &tool.name,
                        &tool.arguments,
                    ));
            if (!legacy_safe && tool.request_sha256 != expected_hash)
                || !crate::offline_tools::configured_tools(&e.ticket.grant, configuration.as_ref())
                    .contains(&tool.name)
                || !e.run.journal.iter().any(|j| {
                    j.kind == "tools_approved"
                        && j.payload
                            .get("request_sha256")
                            .and_then(Value::as_array)
                            .is_some_and(|a| {
                                a.iter()
                                    .any(|v| v.as_str() == Some(tool.request_sha256.as_str()))
                            })
                })
                || e.run.journal.iter().any(|j| {
                    j.kind == "tool_dispatched"
                        && j.payload.get("request_sha256").and_then(Value::as_str)
                            == Some(tool.request_sha256.as_str())
                })
            {
                return Err(Error::UncertainEffect);
            }
            let old = e.run.revision;
            e.run.revision += 1;
            offline::append_offline_journal(
                &mut e.run,
                "tool_dispatched",
                json!({"request_sha256":tool.request_sha256,"name":tool.name,"configuration_sha256":canonical_hash(&configuration),"automatic_replay":false}),
                now(),
            )?;
            self.store
                .save_offline_run(&e.run, &e.messages, old)
                .await?;
            let result = if matches!(
                tool.name.as_str(),
                "list_sources" | "read_source" | "create_artifact"
            ) {
                execute_safe_tool(&e.ticket.grant, &mut e.run.artifacts, &tool)?
            } else {
                self.offline_external_tool(&e.ticket.grant, id, &tool, e.run.deadline_at, stop)
                    .await?
            };
            if stop.is_cancelled() {
                return Err(Error::UncertainEffect);
            }
            let latest = self.store.offline_local_execution(id).await?;
            if tool.name == "mcp_call" {
                let (keys, _) = self.offline_task_keys().await?;
                let original = self
                    .store
                    .offline_mcp_original_result(id, &tool, keys.as_ref())
                    .await?;
                if original != result
                    || canonical_hash(&latest.ticket) != canonical_hash(&e.ticket)
                    || latest.revoked
                    || latest.run.state != "running"
                    || latest.run.id != e.run.id
                    || latest.run.ordinal != e.run.ordinal
                    || latest.run.deadline_at != e.run.deadline_at
                    || now() >= latest.run.deadline_at
                    || latest.run.pending_tools.first().map(|t| &t.request_sha256)
                        != Some(&tool.request_sha256)
                    || latest.run.model_requests < e.run.model_requests
                {
                    return Err(Error::UncertainEffect);
                }
                // Only the authenticated original MCP receipt permits adopting
                // the local sampling counter/journal update of this same run.
                e = latest;
            }
            let latest = self.store.offline_local_execution(id).await?;
            if latest.run.state != "running"
                || latest.run.revision != e.run.revision
                || latest.run.pending_tools.first().map(|t| &t.request_sha256)
                    != Some(&tool.request_sha256)
            {
                return Err(Error::UncertainEffect);
            }
            let old = e.run.revision;
            e.run.revision += 1;
            e.run.pending_tools.remove(0);
            offline::append_offline_journal(
                &mut e.run,
                "tool_confirmed",
                json!({"request_sha256":tool.request_sha256,"result_sha256":canonical_hash(&result),"result":result}),
                now(),
            )?;
            e.messages.push(
                json!({"role":"tool","tool_call_id":tool.id,"content":json_string(&result)?}),
            );
            self.store
                .save_offline_run(&e.run, &e.messages, old)
                .await?;
        }
    }
    pub async fn offline_export_run(&self, id: uuid::Uuid) -> Result<OfflineRunExport> {
        let e = self.store.offline_local_execution(id).await?;
        let (device, key) = self.offline_signer().await?;
        if e.ticket.grant.device != device.device {
            return Err(Error::Forbidden);
        }
        let value = OfflineRunExport {
            device_signature: offline::offline_sign(
                &key,
                if e.run.version == 2 {
                    "run-report-v2"
                } else {
                    "run-report"
                },
                &(&e.ticket, &e.run),
            )?,
            ticket: e.ticket,
            run: e.run,
        };
        offline::validate_offline_report(&value)?;
        Ok(value)
    }
    pub async fn offline_status_request(
        &self,
        grant: uuid::Uuid,
    ) -> Result<OfflineGrantStatusRequest> {
        let ticket = self.store.offline_local_ticket(grant).await?;
        let (device, key) = self.offline_signer().await?;
        if ticket.grant.device != device.device {
            return Err(Error::Forbidden);
        }
        let nonce = uuid::Uuid::new_v4();
        Ok(OfflineGrantStatusRequest {
            grant_id: grant,
            nonce,
            device_signature: offline::offline_sign(&key, "status-request", &(grant, nonce))?,
        })
    }
}
fn tool_definition(
    name: &str,
    config: Option<&cowork_storage::offline_execution::OfflineToolConfiguration>,
) -> Value {
    let parameters = match name {
        "read_source" => {
            json!({"type":"object","properties":{"id":{"type":"string"}},"required":["id"],"additionalProperties":false})
        }
        "create_artifact" => {
            json!({"type":"object","properties":{"name":{"type":"string"},"content":{"type":"string"}},"required":["name","content"],"additionalProperties":false})
        }
        "write_workspace_file" => {
            json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false})
        }
        "read_workspace_file" => {
            json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false})
        }
        "execute_program" => {
            json!({"type":"object","properties":{"program":{"type":"string","enum":config.map(|c|c.programs.clone()).unwrap_or_default()},"arguments":{"type":"array","items":{"type":"string"}},"cwd":{"type":"string"},"timeout_seconds":{"type":"integer","minimum":1,"maximum":600}},"required":["program","arguments","timeout_seconds"],"additionalProperties":false})
        }
        "browser" => {
            json!({"type":"object","properties":{"engine":{"type":"string","enum":["chrome","firefox"]},"mode":{"type":"string","enum":["managed","extension"]},"connection_id":{"type":"string"},"session_id":{"type":"string"},"operation":{"type":"string","enum":["navigate","dom","screenshot","click","fill","tabs","control","back","forward","reload"]},"url":{"type":"string"},"selector":{"type":"string"},"text":{"type":"string"}},"required":["operation"],"additionalProperties":false})
        }
        "office" => {
            json!({"type":"object","properties":{"operation":{"type":"string","enum":["read_text","read_docx","read_xlsx","read_pdf","read_pptx","write_text","create_docx","create_xlsx","create_pptx","create_pdf","convert"]},"input":{"type":"string"},"output":{"type":"string"},"text":{"type":"string"},"title":{"type":"string"}},"required":["operation"],"additionalProperties":false})
        }
        "github" => {
            json!({"type":"object","properties":{"arguments":{"type":"array","items":{"type":"string"}}},"required":["arguments"],"additionalProperties":false})
        }
        "mcp_call" => {
            json!({"type":"object","properties":{"connection_id":{"type":"string","enum":config.map(|c|c.mcp_connections.iter().map(|m|m.id.clone()).collect::<Vec<_>>()).unwrap_or_default()},"tool_name":{"type":"string"},"arguments":{"type":"object"}},"required":["connection_id","tool_name","arguments"],"additionalProperties":false})
        }
        _ => json!({"type":"object","properties":{},"additionalProperties":false}),
    };
    let description = if name == "mcp_call" {
        format!(
            "Call only explicitly configured device MCP connections. Exact human approval required. Connections and selected schemas: {}",
            serde_json::to_string(
                &config
                    .map(|c| c
                        .mcp_connections
                        .iter()
                        .map(|m| json!({"connection_id":m.id,"tools":m.definitions}))
                        .collect::<Vec<_>>())
                    .unwrap_or_default()
            )
            .unwrap_or_default()
        )
    } else {
        "Use only the signed grant and explicitly configured device tools. Each exact action needs human approval. Native programs have owner OS rights; input/output/cwd paths are relative to this run's managed workspace. Omit browser session_id to create a private session. Teamserver credentials are unavailable.".into()
    };
    json!({"type":"function","function":{"name":name,"description":description,"parameters":parameters}})
}
fn execute_safe_tool(
    g: &OfflineGrant,
    artifacts: &mut Vec<OfflineArtifact>,
    tool: &OfflinePendingTool,
) -> Result<Value> {
    let object = tool.arguments.as_object().ok_or(Error::Forbidden)?;
    match tool.name.as_str() {
        "list_sources" if object.is_empty() => Ok(json!(
            g.sources
                .iter()
                .map(|s| json!({"id":s.id,"name":s.name,"revision":s.revision,"sha256":s.sha256}))
                .collect::<Vec<_>>()
        )),
        "read_source" if object.len() == 1 => {
            let id = object
                .get("id")
                .and_then(Value::as_str)
                .ok_or(Error::Forbidden)?;
            Ok(json!(
                g.sources
                    .iter()
                    .find(|s| s.id == id)
                    .ok_or(Error::Forbidden)?
            ))
        }
        "create_artifact" if object.len() == 2 => {
            let name = object
                .get("name")
                .and_then(Value::as_str)
                .ok_or(Error::Forbidden)?;
            let content = object
                .get("content")
                .and_then(Value::as_str)
                .ok_or(Error::Forbidden)?;
            valid_text(name, 200)?;
            if name.contains(['/', '\\', '\0'])
                || name == "."
                || name == ".."
                || content.contains('\0')
                || content.len() > 256 * 1024
                || artifacts.len() >= 16
                || artifacts.iter().map(|a| a.content.len()).sum::<usize>() + content.len()
                    > g.maximum_artifact_bytes as usize
            {
                return Err(Error::Forbidden);
            }
            let value = OfflineArtifact {
                id: uuid::Uuid::new_v4(),
                name: name.into(),
                media_type: "text/plain".into(),
                content: content.into(),
                sha256: hash(content.as_bytes()),
            };
            let result = json!({"id":value.id,"name":value.name,"sha256":value.sha256,"size":value.content.len()});
            artifacts.push(value);
            Ok(result)
        }
        _ => Err(Error::Forbidden),
    }
}
