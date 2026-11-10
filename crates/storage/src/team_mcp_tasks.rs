//! Team task authority is the original run/effect and executing human approval.
use super::*;
use crate::mcp_scoped_tasks::{self, Facts};
use crate::mcp_tasks::McpTaskOperationPermit;
use cowork_mcp::{CallResult, TaskOperation, TaskOperationResult};
#[path = "team_mcp_interactions.rs"]
pub(super) mod interactions;
struct Context {
    approval: Uuid,
    action_sha: String,
    action: Value,
    server: Uuid,
}
impl TeamExecutionAuthority {
    async fn team_task_bearer(&self, context: &Context) -> Result<Option<Zeroizing<String>>> {
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        self.task_facts(&mut tx, context).await?;
        let binding = self
            .binding
            .mcp_servers
            .get(&context.server)
            .ok_or(Error::Forbidden)?;
        let row=sqlx::query("SELECT * FROM team_mcp_servers WHERE id=$1 AND team_id=$2 AND revision=$3 AND enabled=1").bind(context.server.to_string()).bind(self.team_id().to_string()).bind(binding.revision).fetch_one(&mut *tx).await.map_err(storage_error)?;
        let configuration =
            mcp_configuration(self.keys.as_ref(), self.key_id, self.team_id(), &row)?;
        let original = canonical_hash(&configuration);
        let oauth = configuration.oauth.clone();
        tx.commit().await.map_err(storage_error)?;
        if let Some(oauth) = oauth {
            let bearer = self
                .store
                .mcp_oauth_bearer_for(
                    &crate::mcp_oauth::McpOAuthOwner::TeamExecution {
                        authority: self,
                        key: self.key_id,
                    },
                    &oauth.id,
                    self.keys.as_ref(),
                )
                .await?;
            let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
            self.task_facts(&mut tx, context).await?;
            let row=sqlx::query("SELECT * FROM team_mcp_servers WHERE id=$1 AND team_id=$2 AND revision=$3 AND enabled=1").bind(context.server.to_string()).bind(self.team_id().to_string()).bind(binding.revision).fetch_one(&mut *tx).await.map_err(storage_error)?;
            if canonical_hash(&mcp_configuration(
                self.keys.as_ref(),
                self.key_id,
                self.team_id(),
                &row,
            )?) != original
            {
                return Err(Error::Forbidden);
            }
            tx.commit().await.map_err(storage_error)?;
            return Ok(Some(bearer));
        }
        Ok(None)
    }
    async fn refresh_team_task_bearer(
        &self,
        context: &Context,
        session: &mut Session,
    ) -> Result<()> {
        if let Some(bearer) = self.team_task_bearer(context).await? {
            session
                .replace_bearer(bearer.as_str())
                .map_err(|_| Error::McpSessionUnavailable)?;
        }
        Ok(())
    }
    async fn task_facts(&self, tx: &mut Transaction<'_, Any>, context: &Context) -> Result<Facts> {
        validate_binding(&self.store, tx, &self.binding, &self.lease_owner).await?;
        crate::server_keys::server_admission(tx, self.key_id).await?;
        let row=sqlx::query("SELECT * FROM team_tool_approvals WHERE id=$1 AND run_id=$2 AND team_id=$3 AND lease_owner=$4").bind(context.approval.to_string()).bind(self.run_id().to_string()).bind(self.team_id().to_string()).bind(&self.lease_owner).fetch_one(&mut **tx).await.map_err(storage_error)?;
        let approval: TeamToolApproval =
            parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        if approval.state != "executing"
            || approval.action != context.action
            || approval.request_sha256 != context.action_sha
            || hash(json_string(&approval.action)?.as_bytes()) != context.action_sha
        {
            return Err(Error::Forbidden);
        }
        validate_approver(&self.store, tx, &row, &approval).await?;
        let effect = self.effect.as_ref().ok_or(Error::Forbidden)?;
        let binding = self
            .binding
            .mcp_servers
            .get(&context.server)
            .ok_or(Error::Forbidden)?;
        if context.action["server_revision"] != binding.revision
            || context.action["catalog_sha256"] != binding.catalog_sha256
        {
            return Err(Error::Forbidden);
        }
        let effect_count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM team_effect_intents WHERE run_id=$1 AND request_id=$2 AND dispatch_owner=$3 AND kind='tool' AND state='admitted' AND NOT EXISTS(SELECT 1 FROM team_effect_receipts r WHERE r.effect_id=team_effect_intents.id)").bind(self.run_id().to_string()).bind(effect.request_id()).bind(&self.lease_owner).fetch_one(&mut **tx).await.map_err(storage_error)?;
        if effect_count != 1 {
            return Err(Error::UncertainEffect);
        }
        Ok(Facts {
            owner_kind: "team".into(),
            owner_id: self.team_id().to_string(),
            project_id: self.project_id().to_string(),
            run_id: self.run_id().to_string(),
            call_id: effect.request_id().to_owned(),
            server_id: context.server.to_string(),
            server_revision: binding.revision,
            binding_sha256: canonical_hash(&json!([
                context.server,
                binding.revision,
                binding.catalog_sha256
            ])),
            spec_sha256: canonical_hash(&self.binding),
            connection_id: context.action["connection_id"]
                .as_str()
                .ok_or_else(invalid)?
                .into(),
            connection_epoch: context.action["epoch"]
                .as_u64()
                .and_then(|e| e.checked_add(1))
                .ok_or_else(invalid)?,
            original_rpc_id: context.action["rpc_request_id"]
                .as_str()
                .ok_or_else(invalid)?
                .into(),
            original_request_sha256: context.action_sha.clone(),
            transport_request_sha256: context.action["transport_request_sha256"]
                .as_str()
                .ok_or_else(invalid)?
                .into(),
            authority_sha256: canonical_hash(&json!([
                self.binding,
                self.lease_owner,
                context.approval,
                context.action_sha,
                row.try_get::<Option<String>, _>("approved_by")
                    .map_err(storage_error)?,
                row.try_get::<Option<i64>, _>("approved_session_revision")
                    .map_err(storage_error)?
            ])),
        })
    }
    async fn validate_team_task(
        &self,
        context: &Context,
        permit: &McpTaskOperationPermit,
    ) -> Result<()> {
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        let facts = self.task_facts(&mut tx, context).await?;
        mcp_scoped_tasks::validate(&mut tx, &facts, permit, self.key_id).await?;
        tx.commit().await.map_err(storage_error)
    }
    pub(super) async fn await_team_mcp_task(
        &self,
        server: Uuid,
        approval: Uuid,
        action: Value,
        action_sha: String,
        session: &mut Session,
        first: CallResult,
    ) -> Result<CallResult> {
        let context = Context {
            server,
            approval,
            action,
            action_sha,
        };
        let mut ack = first.task.as_ref().ok_or(Error::Conflict)?.clone();
        let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
        let facts = self.task_facts(&mut tx, &context).await?;
        let scope = CheckpointScope {
            project_id: facts.project_id.clone(),
            run_id: facts.run_id.clone(),
            server_id: facts.server_id.clone(),
            connection_record_id: format!(
                "mcp-task:{}",
                canonical_hash(&json!([facts.run_id, facts.server_id, facts.call_id]))
            ),
            server_revision: facts.server_revision,
            checkpoint_revision: 1,
            configuration_sha256: facts.binding_sha256.clone(),
            catalog_sha256: session.catalog().ok_or(Error::Forbidden)?.sha256().into(),
        };
        let session_cipher = cipher(self.keys.as_ref(), self.key_id)?;
        let mut task = mcp_scoped_tasks::capture(
            &mut tx,
            &facts,
            &first,
            session,
            Some((&session_cipher, scope)),
            self.keys.as_ref(),
            self.key_id,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        // An original ACK with no TTL is still bounded by this admitted wait.
        let original_deadline = now()
            .saturating_add(86_400_000)
            .min(ack.expires_at_ms().unwrap_or(i64::MAX));
        loop {
            let remaining = original_deadline.saturating_sub(now()).max(0) as u64;
            if remaining == 0 {
                return Err(Error::UncertainEffect);
            }
            self.revalidate_tool_approval(approval).await?;
            let cancel = self.store.scoped_mcp_task_cancel_requested(&task).await?;
            let operation = if cancel && !ack.status.terminal() {
                TaskOperation::Cancel
            } else if ack.status.terminal() || ack.status == cowork_mcp::TaskStatus::InputRequired {
                TaskOperation::Result
            } else {
                TaskOperation::Get
            };
            if operation == TaskOperation::Get {
                self.team_task_wait(&context, None, original_deadline, async {
                    tokio::time::sleep(Duration::from_millis(ack.poll_interval_ms.min(remaining)))
                        .await;
                    Ok(())
                })
                .await?;
            }
            self.refresh_team_task_bearer(&context, session).await?;
            let prepared = session
                .prepare_task_operation_bounded(
                    &ack,
                    operation,
                    original_deadline.saturating_sub(now()).max(0) as u64,
                )
                .map_err(|_| Error::McpSessionUnavailable)?;
            let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
            let facts = self.task_facts(&mut tx, &context).await?;
            let permit = mcp_scoped_tasks::begin(
                &mut tx,
                &facts,
                &task,
                &prepared,
                self.keys.as_ref(),
                self.key_id,
            )
            .await?;
            tx.commit().await.map_err(storage_error)?;
            self.validate_team_task(&context, &permit).await?;
            let execution = interactions::TeamTaskInteractions {
                authority: self,
                context: &context,
                permit: &permit,
            };
            let handler =
                interactions::handler(self, &context, &permit, &execution, original_deadline);
            let outcome = self
                .team_task_wait(&context, Some(&permit), original_deadline, async {
                    Ok(session
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
                    return Err(Error::UncertainEffect);
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
            let mut tx = self.store.pool.begin().await.map_err(storage_error)?;
            let facts = self.task_facts(&mut tx, &context).await?;
            task =
                mcp_scoped_tasks::commit(&mut tx, &facts, &permit, self.keys.as_ref(), self.key_id)
                    .await?;
            tx.commit().await.map_err(storage_error)?;
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
    async fn team_task_wait<F, T>(
        &self,
        context: &Context,
        permit: Option<&McpTaskOperationPermit>,
        deadline: i64,
        future: F,
    ) -> Result<T>
    where
        F: std::future::Future<Output = Result<T>>,
    {
        let bound = tokio::time::Instant::now()
            + Duration::from_millis(deadline.saturating_sub(now()).max(0) as u64);
        tokio::pin!(future);
        let mut pulse = tokio::time::interval(Duration::from_millis(250));
        loop {
            tokio::select! {biased;_=tokio::time::sleep_until(bound)=>return Err(Error::UncertainEffect),_=pulse.tick()=>{if let Some(permit)=permit{self.validate_team_task(context,permit).await?;}else{self.revalidate_tool_approval(context.approval).await?;}},result=&mut future=>return result}
        }
    }
}
