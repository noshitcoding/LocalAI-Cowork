//! Actual model/workflow tool awaiting, with a durable intent for every task RPC.
use super::native_secrets::VerifiedNativeKeys;
use super::*;
use cowork_mcp::{CallResult, Session, SessionCipher, TaskOperation, TaskOperationResult};
use cowork_secrets::KeyId;
impl Runtime {
    pub(super) async fn native_mcp_task_bearer(
        &self,
        project: &str,
        binding: &McpToolBinding,
    ) -> Result<Option<zeroize::Zeroizing<String>>> {
        let server = self.store.mcp_server(project, &binding.server_id).await?;
        if !server.configuration.enabled
            || server.revision != binding.server_revision
            || !server.bindings()?.contains(binding)
        {
            return Err(Error::Forbidden);
        }
        let reference = server
            .configuration
            .credential_reference
            .as_deref()
            .filter(|v| v.starts_with("oauth:"));
        if let Some(reference) = reference {
            let bearer = self
                .native_mcp_oauth_bearer(
                    project,
                    &server.id,
                    &server.configuration.endpoint,
                    reference,
                    server.configuration.credential_revision,
                )
                .await?;
            let fresh = self.store.mcp_server(project, &binding.server_id).await?;
            if canonical_hash(&fresh) != canonical_hash(&server)
                || !fresh.configuration.enabled
                || !fresh.bindings()?.contains(binding)
            {
                return Err(Error::Forbidden);
            }
            return Ok(Some(bearer));
        }
        Ok(None)
    }
    pub(super) async fn refresh_native_mcp_task_bearer(
        &self,
        project: &str,
        binding: &McpToolBinding,
        session: &mut Session,
    ) -> Result<()> {
        if let Some(bearer) = self.native_mcp_task_bearer(project, binding).await? {
            session
                .replace_bearer(bearer.as_str())
                .map_err(|_| Error::McpSessionUnavailable)?;
        }
        Ok(())
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "await_native_mcp_task: Keep original MCP run/lease/call, session/task response and cipher/native key custody explicit"
    )]
    pub(super) async fn await_native_mcp_task(
        &self,
        run: &str,
        lease: &str,
        tool: &ToolCall,
        session: &mut Session,
        first: CallResult,
        cipher: Option<&SessionCipher>,
        keys: &VerifiedNativeKeys,
        key: KeyId,
        stop: &CancellationToken,
    ) -> Result<CallResult> {
        let mut ack = first.task.ok_or(Error::Conflict)?;
        let mut task = self
            .store
            .capture_native_mcp_task(
                run,
                lease,
                &tool.id,
                &ack,
                &first.response,
                session,
                cipher,
                keys,
                key,
            )
            .await?;
        loop {
            if ack.expires_at_ms().is_some_and(|v| v <= now()) {
                return Err(Error::UncertainEffect);
            }
            let cancel = self.store.native_mcp_task_cancel_requested(&task).await?;
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
                let wait = tokio::time::sleep(Duration::from_millis(ack.poll_interval_ms));
                self.await_mcp_task_edge(run, lease, stop, async {
                    wait.await;
                    Ok(())
                })
                .await?;
                if self.store.native_mcp_task_cancel_requested(&task).await? {
                    continue;
                }
            }
            let current = self.store.stored_run(run).await?;
            current.assert_lease(lease, now())?;
            self.refresh_native_mcp_task_bearer(
                &current.record.spec.project_id,
                cowork_domain::mcp::mcp_binding(&current.record, tool)?,
                session,
            )
            .await?;
            if stop.is_cancelled() {
                return Err(Error::UncertainEffect);
            }
            let remaining = remaining_run_millis(&current.record, now()).min(
                task.view
                    .expires_at
                    .map_or(u64::MAX, |v| v.saturating_sub(now()).max(0) as u64),
            );
            let prepared = session
                .prepare_task_operation_bounded(&ack, operation, remaining.min(86_400_000))
                .map_err(|_| Error::McpSessionUnavailable)?;
            let permit = self
                .store
                .begin_native_mcp_task_operation(run, lease, &task, &prepared, keys, key)
                .await?;
            self.store
                .validate_native_mcp_task_operation(run, lease, &permit, key)
                .await?;
            let handler = super::mcp_interactions::NativeTaskInteractions {
                runtime: self,
                run,
                lease,
                permit: &permit,
                keys,
                key,
                stop,
                deadline: now().saturating_add(remaining.min(86_400_000) as i64),
            };
            let request = session.task_operation_interactive(prepared, &ack, &handler);
            let outcome = self
                .await_mcp_task_edge(run, lease, stop, async { Ok(request.await) })
                .await;
            let result = match outcome {
                Ok(Ok(result)) => result,
                Ok(Err(failure)) => {
                    if let Some(response) = failure.response() {
                        self.store.capture_mcp_task_operation(&permit,&json!({"kind":"unconfirmed","response":response,"code":failure.code.to_string(),"effects_unknown":true}),keys,key).await?;
                        self.store.finish_mcp_task_evidence_unknown(&permit).await?;
                    } else {
                        let _ = self.store.unknown_mcp_task_operation(&permit).await;
                    }
                    return Err(Error::UncertainEffect);
                }
                Err(error) => {
                    let _ = self.store.unknown_mcp_task_operation(&permit).await;
                    return Err(error);
                }
            };
            let response = match &result {
                TaskOperationResult::Status(ack) => json!({"kind":"status","ack":ack}),
                TaskOperationResult::Result(result) => {
                    json!({"kind":"result","response":result.response,"response_sha256":result.response_sha256})
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
                .commit_native_mcp_task_operation(run, lease, &permit, keys, key)
                .await?;
            match result {
                TaskOperationResult::Status(next) => ack = next,
                TaskOperationResult::Result(result) => {
                    if matches!(
                        ack.status,
                        cowork_mcp::TaskStatus::Failed | cowork_mcp::TaskStatus::Cancelled
                    ) || result.response["isError"] == true
                    {
                        return Err(Error::UncertainEffect);
                    }
                    return Ok(result);
                }
                TaskOperationResult::Error { .. } => return Err(Error::UncertainEffect),
            }
        }
    }
    async fn await_mcp_task_edge<F, T>(
        &self,
        run: &str,
        lease: &str,
        stop: &CancellationToken,
        future: F,
    ) -> Result<T>
    where
        F: std::future::Future<Output = Result<T>>,
    {
        let initial = self.store.stored_run(run).await?;
        initial.assert_lease(lease, now())?;
        let deadline = tokio::time::Instant::now()
            + Duration::from_millis(remaining_run_millis(&initial.record, now()));
        tokio::pin!(future);
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        let mut heartbeat = now();
        loop {
            tokio::select! {biased;
                _=stop.cancelled()=>return Err(Error::UncertainEffect),
                _=tokio::time::sleep_until(deadline)=>return Err(Error::BudgetExceeded),
                _ = interval.tick() => {
                    let current = self.store.stored_run(run).await?;
                    current.assert_lease(lease, now())?;
                    if current.record.state != RunState::Running
                        || time_budget_exhausted(&current.record, now())
                    {
                        return Err(Error::UncertainEffect);
                    }
                    if now() - heartbeat >= 3000 {
                        self.store.heartbeat(run, lease).await?;
                        heartbeat = now();
                    }
                },
                result=&mut future=>return result,
            }
        }
    }
}
