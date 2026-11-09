//! Continue only the original actor/node tool ACK. No replacement tool dispatch.
use super::*;
use cowork_mcp::{CallResult, TaskOperation, TaskOperationResult};
#[path = "android_private_mcp_interactions.rs"]
mod interactions;
impl AndroidDeviceServices {
    pub(super) async fn private_mcp_task_await(
        &self,
        lease: &cowork_storage::android_private_server::PrivateServerLease,
        sequence: i64,
        active: &mut PrivateMcpSession,
        first: CallResult,
        stop: &CancellationToken,
    ) -> Result<CallResult> {
        let mut ack = first.task.as_ref().ok_or_else(failed)?.clone();
        let cipher = if active.transport == "streamable_http" {
            Some(self.private_mcp_cipher(None).await?)
        } else {
            None
        };
        let mut task = self
            .store
            .android_private_mcp_task_capture(
                lease.run,
                lease.lease,
                sequence,
                &first,
                &active.session,
                cipher.as_ref(),
                self.keys.as_ref(),
                self.key_id,
            )
            .await?;
        loop {
            let cp = self
                .store
                .android_private_loop_checkpoint(lease.run, lease.lease, self.keys.as_ref())
                .await?;
            let deadline = cp["deadline"].as_i64().ok_or_else(invalid)?;
            let remaining = deadline
                .min(ack.expires_at_ms().unwrap_or(i64::MAX))
                .saturating_sub(chrono::Utc::now().timestamp_millis())
                .max(0) as u64;
            if remaining == 0 || stop.is_cancelled() {
                return Err(failed());
            }
            let cancel = self.store.scoped_mcp_task_cancel_requested(&task).await?;
            let operation = if cancel && !ack.status.terminal() {
                TaskOperation::Cancel
            } else if ack.status.terminal() || ack.status == cowork_mcp::TaskStatus::InputRequired {
                TaskOperation::Result
            } else {
                TaskOperation::Get
            };
            if operation == TaskOperation::Get {
                self.private_mcp_task_wait(lease, None, remaining, stop, async {
                    tokio::time::sleep(Duration::from_millis(ack.poll_interval_ms.min(remaining)))
                        .await;
                    Ok(())
                })
                .await?;
            }
            if active.transport != "stdio" {
                let configuration = self
                    .store
                    .android_private_mcp_open_for_run(
                        lease.run,
                        lease.lease,
                        &active.frozen,
                        self.keys.as_ref(),
                    )
                    .await?;
                let original_configuration_sha = canonical_hash(&configuration);
                let configuration =
                    cowork_storage::android_private_mcp::configuration(&configuration)?;
                if let Some(oauth) = &configuration.oauth {
                    let owner = Uuid::parse_str(
                        lease.value["spec"]["creator_user_id"]
                            .as_str()
                            .ok_or_else(invalid)?,
                    )
                    .map_err(|_| invalid())?;
                    let credential = self
                        .store
                        .mcp_oauth_bearer_for(
                            &cowork_storage::mcp_oauth::McpOAuthOwner::PersonalExecution {
                                owner,
                                run: lease.run,
                                lease: lease.lease,
                                key: self.key_id,
                            },
                            &oauth.id,
                            self.keys.as_ref(),
                        )
                        .await?;
                    let fresh = self
                        .store
                        .android_private_mcp_open_for_run(
                            lease.run,
                            lease.lease,
                            &active.frozen,
                            self.keys.as_ref(),
                        )
                        .await?;
                    if canonical_hash(&fresh) != original_configuration_sha {
                        return Err(Error::Forbidden);
                    }
                    active
                        .session
                        .replace_bearer(credential.as_str())
                        .map_err(|_| failed())?;
                }
            }
            if stop.is_cancelled() {
                return Err(failed());
            }
            let prepared = active
                .session
                .prepare_task_operation_bounded(&ack, operation, remaining.min(86_400_000))
                .map_err(|_| failed())?;
            let permit = self
                .store
                .android_private_mcp_task_begin(
                    lease.run,
                    lease.lease,
                    sequence,
                    &task,
                    &prepared,
                    self.keys.as_ref(),
                    self.key_id,
                )
                .await?;
            self.store
                .android_private_mcp_task_validate(
                    lease.run,
                    lease.lease,
                    sequence,
                    &permit,
                    self.keys.as_ref(),
                    self.key_id,
                )
                .await?;
            let execution = interactions::PrivateTaskInteractions {
                service: self,
                lease,
                sequence,
                permit: &permit,
                stop,
            };
            let handler = interactions::handler(
                &execution,
                deadline.min(ack.expires_at_ms().unwrap_or(i64::MAX)),
            );
            let outcome = self
                .private_mcp_task_wait(lease, Some((sequence, &permit)), remaining, stop, async {
                    Ok(active
                        .session
                        .task_operation_interactive(prepared, &ack, &handler)
                        .await)
                })
                .await;
            let result = match outcome {
                Ok(Ok(result)) => result,
                Ok(Err(failure)) => {
                    if let Some(response) = failure.response() {
                        self.store.capture_mcp_task_operation(&permit,&json!({"kind":"unconfirmed","response":response,"code":failure.code.to_string(),"effects_unknown":true}),self.keys.as_ref(),self.key_id).await?;
                        self.store.finish_mcp_task_evidence_unknown(&permit).await?;
                    } else {
                        let _ = self.store.unknown_mcp_task_operation(&permit).await;
                    }
                    return Err(failed());
                }
                Err(error) => {
                    let _ = self.store.unknown_mcp_task_operation(&permit).await;
                    return Err(error);
                }
            };
            let protected = match &result {
                TaskOperationResult::Status(next) => json!({"kind":"status","ack":next}),
                TaskOperationResult::Result(result) => {
                    json!({"kind":"result","response":result.response,"response_sha256":result.response_sha256})
                }
                TaskOperationResult::Error { response, sha256 } => {
                    json!({"kind":"error","response":response,"response_sha256":sha256})
                }
            };
            self.store
                .capture_mcp_task_operation(&permit, &protected, self.keys.as_ref(), self.key_id)
                .await?;
            task = self
                .store
                .android_private_mcp_task_commit(
                    lease.run,
                    lease.lease,
                    sequence,
                    &permit,
                    self.keys.as_ref(),
                    self.key_id,
                )
                .await?;
            match result {
                TaskOperationResult::Status(next) => ack = next,
                TaskOperationResult::Result(result) => {
                    if matches!(
                        ack.status,
                        cowork_mcp::TaskStatus::Failed | cowork_mcp::TaskStatus::Cancelled
                    ) || result.response["isError"] == true
                    {
                        return Err(failed());
                    }
                    return Ok(result);
                }
                TaskOperationResult::Error { .. } => return Err(failed()),
            }
        }
    }
    async fn private_mcp_task_wait<F, T>(
        &self,
        lease: &cowork_storage::android_private_server::PrivateServerLease,
        operation: Option<(i64, &cowork_storage::mcp_tasks::McpTaskOperationPermit)>,
        remaining: u64,
        stop: &CancellationToken,
        future: F,
    ) -> Result<T>
    where
        F: std::future::Future<Output = Result<T>>,
    {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(remaining);
        tokio::pin!(future);
        let mut pulse = tokio::time::interval(Duration::from_millis(250));
        let mut heartbeat = tokio::time::Instant::now();
        loop {
            tokio::select! {biased;
                _=stop.cancelled()=>return Err(failed()),
                _=tokio::time::sleep_until(deadline)=>return Err(failed()),
                _=pulse.tick()=>{if let Some((sequence,permit))=operation{self.store.android_private_mcp_task_validate(lease.run,lease.lease,sequence,permit,self.keys.as_ref(),self.key_id).await?;}else{self.store.android_private_loop_checkpoint(lease.run,lease.lease,self.keys.as_ref()).await?;}
                    if heartbeat.elapsed()>=Duration::from_secs(3){self.store.android_private_server_renew(lease.run,lease.lease).await?;heartbeat=tokio::time::Instant::now();}},
                result=&mut future=>return result,
            }
        }
    }
}
