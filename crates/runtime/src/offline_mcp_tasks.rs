//! Offline tasks use the held original HTTP session and the local device vault.
use super::*;
use cowork_mcp::{CallResult, Session, SessionCipher, TaskOperation, TaskOperationResult};
use cowork_secrets::{KeyId, KeyStore, NativeKeyManager};
use cowork_storage::mcp_tasks::{McpTaskOperationPermit, McpTaskRecord};
use cowork_storage::offline_mcp_tasks::OfflineMcpCallPermit;
use uuid::Uuid;
pub(super) struct OfflineTaskExecution<'a> {
    pub runtime: &'a Runtime,
    pub run: Uuid,
    pub source: &'a OfflineMcpCallPermit,
    pub permit: &'a McpTaskOperationPermit,
    pub keys: &'a dyn KeyStore,
    pub key: KeyId,
    pub stop: &'a CancellationToken,
    pub deadline: i64,
}
impl OfflineTaskExecution<'_> {
    pub(super) async fn current(&self) -> Result<()> {
        if self.stop.is_cancelled() || now() >= self.deadline {
            return Err(Error::UncertainEffect);
        }
        self.runtime
            .store
            .validate_offline_mcp_task_operation(self.source, self.permit, self.keys, self.key)
            .await?;
        if self.stop.is_cancelled() || now() >= self.deadline {
            return Err(Error::UncertainEffect);
        }
        Ok(())
    }
}
impl Runtime {
    pub(super) async fn offline_task_keys(&self) -> Result<(Arc<dyn NativeKeyManager>, KeyId)> {
        let (device, _) = self.offline_signer().await?;
        let key = device.key_id;
        let keys = self.offline_keys(&device).await?;
        Ok((keys, key))
    }
    pub(super) async fn execute_offline_mcp_call(
        &self,
        g: &OfflineGrant,
        run: Uuid,
        c: &cowork_storage::offline_execution::OfflineToolConfiguration,
        tool: &OfflinePendingTool,
        deadline: i64,
        stop: &CancellationToken,
    ) -> Result<Value> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Call {
            connection_id: String,
            tool_name: String,
            arguments: Value,
        }
        let call: Call =
            serde_json::from_value(tool.arguments.clone()).map_err(|_| Error::Forbidden)?;
        let binding = c
            .mcp_connections
            .iter()
            .find(|b| b.id == call.connection_id && b.allowed_tools.contains(&call.tool_name))
            .ok_or(Error::Forbidden)?;
        let secret = match &binding.credential {
            Some(v) => Some(
                self.offline_open_tool_secret(
                    g,
                    &format!("mcp/{}", binding.id),
                    &binding.endpoint,
                    v,
                )
                .await?,
            ),
            None if binding.allow_no_key => None,
            None => return Err(Error::Forbidden),
        };
        let (keys, key) = self.offline_task_keys().await?;
        if key.instance != g.device.id {
            return Err(Error::Forbidden);
        }
        let owned = keys.clone();
        let cipher = tokio::task::spawn_blocking(move || {
            let material = owned.load(&key).map_err(|_| Error::NativeKeyUnavailable)?;
            SessionCipher::new(
                &key.cipher_id().map_err(|_| Error::NativeKeyUnavailable)?,
                material,
            )
            .map_err(|_| Error::McpSessionUnavailable)
        })
        .await
        .map_err(storage_error)??;
        if stop.is_cancelled() || now() >= deadline {
            return Err(Error::UncertainEffect);
        }
        let endpoint =
            cowork_mcp::Endpoint::new(&binding.endpoint, secret.as_ref().map(|s| s.as_str()))
                .map_err(|_| Error::Forbidden)?;
        let remaining = deadline.saturating_sub(now()).max(0) as u64;
        let mut session = tokio::select! {biased;_=stop.cancelled()=>return Err(Error::UncertainEffect),result=Session::connect_interactive(endpoint,cowork_mcp::Limits{timeout_ms:remaining.min(120_000),..Default::default()})=>result.map_err(|_|Error::McpSessionUnavailable)?};
        let catalog = tokio::select! {biased;_=stop.cancelled()=>return Err(Error::UncertainEffect),result=session.discover()=>result.map_err(|_|Error::McpSessionUnavailable)?};
        if catalog.sha256() != binding.catalog_sha256 {
            return Err(Error::McpSessionUnavailable);
        }
        let prepared = session
            .prepare(&binding.catalog_sha256, &call.tool_name, call.arguments)
            .map_err(|_| Error::Forbidden)?;
        let source = self
            .store
            .prepare_offline_mcp_call(run, &prepared, keys.as_ref(), key)
            .await?;
        if stop.is_cancelled() || now() >= deadline {
            return Err(Error::UncertainEffect);
        }
        self.store
            .admit_offline_mcp_call(&source, keys.as_ref(), key)
            .await?;
        self.store
            .validate_offline_mcp_call(&source, keys.as_ref(), key)
            .await?;
        if stop.is_cancelled() || now() >= deadline {
            self.store.unknown_offline_mcp_call(&source).await?;
            return Err(Error::UncertainEffect);
        }
        let outcome = self
            .offline_task_edge(
                &source,
                keys.as_ref(),
                key,
                deadline,
                stop,
                session.call(prepared),
            )
            .await;
        let first = match outcome {
            Ok(Ok(v)) => v,
            Ok(Err(failure)) => {
                if let Some(response) = failure.response() {
                    self.store
                        .capture_offline_mcp_call_failure(
                            &source,
                            response,
                            &failure.code.to_string(),
                            keys.as_ref(),
                            key,
                        )
                        .await?;
                } else {
                    self.store.unknown_offline_mcp_call(&source).await?;
                }
                return Err(Error::UncertainEffect);
            }
            Err(e) => {
                self.store.unknown_offline_mcp_call(&source).await?;
                return Err(e);
            }
        };
        // Capture original response even if execution was revoked while receiving.
        let task = self
            .store
            .capture_offline_mcp_call(&source, &first, &session, &cipher, keys.as_ref(), key)
            .await?;
        if let Some(task) = task {
            self.await_offline_mcp_task(
                run,
                &source,
                task,
                first,
                &mut session,
                keys.as_ref(),
                key,
                deadline,
                stop,
            )
            .await?;
        }
        self.store
            .validate_offline_mcp_call(&source, keys.as_ref(), key)
            .await?;
        if stop.is_cancelled() {
            return Err(Error::UncertainEffect);
        }
        self.store
            .offline_mcp_original_result(run, tool, keys.as_ref())
            .await
    }
    async fn offline_task_edge<F, T>(
        &self,
        source: &OfflineMcpCallPermit,
        keys: &dyn KeyStore,
        key: KeyId,
        deadline: i64,
        stop: &CancellationToken,
        future: F,
    ) -> Result<std::result::Result<T, cowork_mcp::CallFailure>>
    where
        F: std::future::Future<Output = std::result::Result<T, cowork_mcp::CallFailure>>,
    {
        tokio::pin!(future);
        let mut pulse = tokio::time::interval(Duration::from_millis(250));
        let end = tokio::time::Instant::now()
            + Duration::from_millis(deadline.saturating_sub(now()).max(0) as u64);
        loop {
            tokio::select! {biased;result=&mut future=>return Ok(result),_=stop.cancelled()=>return Err(Error::UncertainEffect),_=tokio::time::sleep_until(end)=>return Err(Error::UncertainEffect),_=pulse.tick()=>self.store.validate_offline_mcp_call(source,keys,key).await?}
        }
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "await_offline_mcp_task: Keep original offline execution permit, MCP task/session and key/cipher custody explicit"
    )]
    async fn await_offline_mcp_task(
        &self,
        run: Uuid,
        source: &OfflineMcpCallPermit,
        mut task: McpTaskRecord,
        first: CallResult,
        session: &mut Session,
        keys: &dyn KeyStore,
        key: KeyId,
        deadline: i64,
        stop: &CancellationToken,
    ) -> Result<()> {
        let mut ack = first.task.ok_or(Error::Forbidden)?;
        loop {
            self.store
                .validate_offline_mcp_call(source, keys, key)
                .await?;
            if stop.is_cancelled()
                || now() >= deadline
                || ack.expires_at_ms().is_some_and(|t| t <= now())
            {
                return Err(Error::UncertainEffect);
            }
            let cancel = self.store.scoped_mcp_task_cancel_requested(&task).await?;
            let operation = if cancel && !ack.status.terminal() {
                TaskOperation::Cancel
            } else if matches!(
                ack.status,
                cowork_mcp::TaskStatus::Completed
                    | cowork_mcp::TaskStatus::Failed
                    | cowork_mcp::TaskStatus::Cancelled
                    | cowork_mcp::TaskStatus::InputRequired
            ) {
                TaskOperation::Result
            } else {
                TaskOperation::Get
            };
            if operation == TaskOperation::Get {
                tokio::select! {_=stop.cancelled()=>return Err(Error::UncertainEffect),_=tokio::time::sleep(Duration::from_millis(ack.poll_interval_ms.min(deadline.saturating_sub(now()).max(0) as u64)))=>{}}
            }
            let remaining = deadline.saturating_sub(now()).max(0) as u64;
            let prepared = session
                .prepare_task_operation_bounded(
                    &ack,
                    operation,
                    remaining.min(
                        ack.expires_at_ms()
                            .map_or(u64::MAX, |t| t.saturating_sub(now()).max(0) as u64),
                    ),
                )
                .map_err(|_| Error::McpSessionUnavailable)?;
            let permit = self
                .store
                .begin_offline_mcp_task_operation(source, &task, &prepared, keys, key)
                .await?;
            self.store
                .validate_offline_mcp_task_operation(source, &permit, keys, key)
                .await?;
            if stop.is_cancelled() || now() >= deadline {
                self.store.unknown_mcp_task_operation(&permit).await?;
                return Err(Error::UncertainEffect);
            }
            let execution = OfflineTaskExecution {
                runtime: self,
                run,
                source,
                permit: &permit,
                keys,
                key,
                stop,
                deadline,
            };
            let handler = cowork_storage::mcp_interactions::ScopedTaskInteractions {
                store: &self.store,
                permit: &permit,
                keys,
                key,
                execution: &execution,
                deadline,
            };
            let result = self
                .offline_task_edge(
                    source,
                    keys,
                    key,
                    deadline,
                    stop,
                    session.task_operation_interactive(prepared, &ack, &handler),
                )
                .await;
            let result = match result {
                Ok(Ok(v)) => v,
                Ok(Err(failure)) => {
                    if let Some(response) = failure.response() {
                        self.store.capture_mcp_task_operation(&permit,&json!({"kind":"unconfirmed","response":response,"code":failure.code.to_string(),"effects_unknown":true}),keys,key).await?;
                        self.store.finish_mcp_task_evidence_unknown(&permit).await?;
                    } else {
                        self.store.unknown_mcp_task_operation(&permit).await?;
                    }
                    return Err(Error::UncertainEffect);
                }
                Err(e) => {
                    self.store.unknown_mcp_task_operation(&permit).await?;
                    return Err(e);
                }
            };
            let original = match &result {
                TaskOperationResult::Status(v) => json!({"kind":"status","ack":v}),
                TaskOperationResult::Result(v) => {
                    json!({"kind":"result","response":v.response,"response_sha256":v.response_sha256})
                }
                TaskOperationResult::Error { response, sha256 } => {
                    json!({"kind":"error","response":response,"response_sha256":sha256})
                }
            };
            self.store
                .capture_mcp_task_operation(&permit, &original, keys, key)
                .await?;
            task = self
                .store
                .commit_offline_mcp_task_operation(source, &permit, keys, key)
                .await?;
            match result {
                TaskOperationResult::Status(next) => ack = next,
                TaskOperationResult::Result(_) | TaskOperationResult::Error { .. } => return Ok(()),
            }
        }
    }
    pub async fn offline_mcp_tasks(&self, run: Uuid) -> Result<Vec<McpAsyncTask>> {
        let (keys, _) = self.offline_task_keys().await?;
        self.store.offline_mcp_tasks(run, keys.as_ref()).await
    }
    pub async fn offline_mcp_task_detail(&self, run: Uuid, id: &str) -> Result<McpTaskDetail> {
        let (keys, _) = self.offline_task_keys().await?;
        self.store
            .offline_mcp_task_detail(run, id, keys.as_ref())
            .await
    }
    pub async fn offline_mcp_interactions(
        &self,
        run: Uuid,
        after: Option<&str>,
        only_waiting: bool,
    ) -> Result<OfflineMcpInteractionPage> {
        let (keys, _) = self.offline_task_keys().await?;
        self.store
            .offline_mcp_interactions(run, after, only_waiting, keys.as_ref())
            .await
    }
    pub async fn answer_offline_mcp_interaction(
        &self,
        run: Uuid,
        id: &str,
        input: &AnswerMcpTaskInteraction,
    ) -> Result<Value> {
        let (keys, key) = self.offline_task_keys().await?;
        self.store
            .answer_offline_mcp_interaction(run, id, input, keys.as_ref(), key)
            .await
    }
    pub async fn cancel_offline_mcp_task(
        &self,
        run: Uuid,
        id: &str,
        input: &McpAsyncTaskRequest,
    ) -> Result<Value> {
        let (keys, _) = self.offline_task_keys().await?;
        self.store
            .cancel_offline_mcp_task(run, id, input, keys.as_ref())
            .await
    }
    pub async fn offline_mcp_original_receipt(
        &self,
        run: Uuid,
        id: &str,
        interaction: bool,
        input: &McpAsyncTaskRequest,
    ) -> Result<Option<Value>> {
        let (keys, _) = self.offline_task_keys().await?;
        self.store
            .offline_mcp_original_receipt(run, id, interaction, input, keys.as_ref())
            .await
    }
}
