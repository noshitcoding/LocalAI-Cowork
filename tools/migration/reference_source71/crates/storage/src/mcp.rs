use super::*;
use cowork_domain::mcp::*;
use cowork_policy::{Decision, evaluate};

/// Owns the original in-memory prepared request. Never regenerated on replay.
pub struct McpDispatchPermit {
    attempt: McpAttempt,
    prepared: cowork_mcp::PreparedCall,
}
impl McpDispatchPermit {
    pub fn attempt(&self) -> &McpAttempt {
        &self.attempt
    }
    /// Trusted dispatcher must recheck its live lease immediately before IO and
    /// retain uncertainty if the process dies after admission, even before send.
    pub fn into_prepared(self) -> cowork_mcp::PreparedCall {
        self.prepared
    }
}
pub enum McpAdmission {
    Dispatch(McpDispatchPermit),
    Existing(McpAttempt),
}
#[derive(Clone)]
pub struct McpWrite {
    pub(crate) stored: StoredMcpAttempt,
    pub(crate) expected_revision: Option<i64>,
    prepared_epoch: Option<u64>,
    completion: Option<McpSessionCompletion>,
}
pub enum McpResolution {
    Completed {
        result: McpResult,
        evidence_sha256: String,
    },
    NotExecuted {
        evidence_sha256: String,
    },
}
impl Store {
    /// Authenticated owner review preserves the complete bound receipt, not only
    /// a claimed digest. It never grants a replay of an uncertain remote effect.
    pub async fn reconcile_reviewed_mcp(
        &self,
        run: &str,
        call: &str,
        expected_revision: i64,
        request_sha256: &str,
        review: Value,
    ) -> Result<McpAttempt> {
        let attempt = self
            .stored_mcp_attempt(run, call)
            .await?
            .ok_or(Error::NotFound)?;
        let stored = self.stored_run(run).await?;
        if pending(&attempt.record.state)
            && (attempt.record.revision != expected_revision
                || stored.record.state != RunState::Interrupted
                || stored.lease.is_some())
        {
            return Err(Error::Conflict);
        }
        if review["owner_verified"] != true
            || review["rpc_request_id"] != attempt.record.rpc_request_id
            || review["server_id"] != attempt.record.binding.server_id
            || review["request_sha256"] != request_sha256
            || attempt.record.request_sha256 != request_sha256
            || !review["receipt"].is_object()
            || review["source"]
                .as_str()
                .is_none_or(|s| s.is_empty() || s.len() > 2048)
        {
            return Err(Error::Forbidden);
        }
        let raw = serde_json::to_vec(&review).map_err(storage_error)?;
        if raw.len() > 4 * 1024 * 1024 {
            return Err(Error::Forbidden);
        }
        let evidence_sha256 = canonical_hash(&review);
        let resolution = match review["outcome"].as_str() {
            Some("completed") => {
                let response = review["receipt"]["result"].clone();
                let result = McpResult {
                    sha256: canonical_hash(&response),
                    response,
                };
                checked_result(&attempt.record.binding, &result)?;
                if review["receipt"]["id"] != attempt.record.rpc_request_id {
                    return Err(Error::Forbidden);
                }
                McpResolution::Completed {
                    result,
                    evidence_sha256: evidence_sha256.clone(),
                }
            }
            Some("not_executed")
                if review["receipt"]["not_executed"] == true
                    && review["receipt"]["id"] == attempt.record.rpc_request_id =>
            {
                McpResolution::NotExecuted {
                    evidence_sha256: evidence_sha256.clone(),
                }
            }
            _ => return Err(Error::Forbidden),
        };
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        sqlx::query("INSERT INTO mcp_resolution_reviews(run_id,tool_call_id,evidence_sha256,payload,created_at) VALUES($1,$2,$3,$4,$5) ON CONFLICT(run_id,tool_call_id) DO NOTHING").bind(run).bind(call).bind(&evidence_sha256).bind(json_string(&review)?).bind(now()).execute(&mut *tx).await.map_err(storage_error)?;
        let existing:String=sqlx::query_scalar("SELECT evidence_sha256 FROM mcp_resolution_reviews WHERE run_id=$1 AND tool_call_id=$2").bind(run).bind(call).fetch_one(&mut *tx).await.map_err(storage_error)?;
        if existing != evidence_sha256 {
            return Err(Error::Conflict);
        }
        tx.commit().await.map_err(storage_error)?;
        self.reconcile_mcp(run, call, expected_revision, request_sha256, resolution)
            .await
    }
    async fn stored_mcp_attempt(&self, run: &str, call: &str) -> Result<Option<StoredMcpAttempt>> {
        let stored = self.stored_run(run).await?;
        sqlx::query("SELECT id,tool_call_id,revision,payload FROM mcp_attempts WHERE run_id=$1 AND tool_call_id=$2")
            .bind(run).bind(call).fetch_optional(&self.pool).await.map_err(storage_error)?
            .map(|row|checked_attempt(&row,&stored.record)).transpose()
    }
    pub async fn mcp_attempts(&self, run: &str) -> Result<Vec<McpAttempt>> {
        let stored = self.stored_run(run).await?;
        let rows = sqlx::query(
            "SELECT id,tool_call_id,revision,payload FROM mcp_attempts WHERE run_id=$1 ORDER BY id",
        )
        .bind(run)
        .fetch_all(&self.pool)
        .await
        .map_err(storage_error)?;
        records(rows, &stored.record)
    }
    pub async fn admit_mcp(
        &self,
        run: &str,
        token: &str,
        tool: &ToolCall,
        prepared: cowork_mcp::PreparedCall,
    ) -> Result<McpAdmission> {
        let initial = self.stored_run(run).await?;
        initial.assert_lease(token, now())?;
        let request = mcp_request_hash(&initial.record, tool);
        if let Some(existing) = self.stored_mcp_attempt(run, &tool.id).await? {
            return replay(existing, &request);
        }
        valid_text(&tool.id, 128)?;
        let binding = mcp_binding(&initial.record, tool)?.clone();
        let name = binding.definition["name"]
            .as_str()
            .ok_or(Error::Forbidden)?;
        if prepared.endpoint() != binding.endpoint
            || prepared.catalog_sha256() != binding.catalog_sha256
            || prepared.tool_name() != name
            || prepared.arguments() != &tool.arguments
        {
            return Err(Error::Forbidden);
        }
        tool_catalog(&binding)?
            .validate_arguments(name, &tool.arguments)
            .map_err(|_| Error::Validation("Invalid MCP arguments".into()))?;
        let timeout = prepared.timeout_ms();
        let prepared_epoch = prepared.epoch();
        let timestamp = now();
        let attempt = StoredMcpAttempt {
            lease_sha256: hash(token.as_bytes()),
            record: McpAttempt {
                id: id(),
                run_id: run.into(),
                project_id: initial.record.spec.project_id.clone(),
                tool: tool.clone(),
                binding,
                request_sha256: request.clone(),
                transport_request_sha256: prepared.request_sha256().into(),
                transport_protocol:(prepared.protocol()!=MCP_PROTOCOL_VERSION).then(||prepared.protocol().to_owned()),
                transport_task_ttl_ms:prepared.task_ttl_ms(),
                rpc_request_id: prepared.rpc_request_id().into(),
                connection_id: prepared.connection_id(),
                state: McpAttemptState::DispatchCommitted,
                revision: 1,
                created_at: timestamp,
                updated_at: timestamp,
            },
        };
        attempt.validate_binding(&initial.record)?;
        let commit=self.mutate(run,Some(token),|r|{
            pending_tool(r,tool)?;
            if r.record.state!=RunState::Running || r.checkpoint.in_flight.is_some() {return Err(Error::UncertainEffect);}
            if mcp_request_hash(&r.record,tool)!=request {return Err(Error::Conflict);}
            if evaluate(&r.record,tool,&[],&[],&r.checkpoint.approvals)!=Decision::Allow {return Err(Error::Forbidden);}
            if time_budget_exhausted(&r.record,now()) || r.record.steps>=r.record.spec.budgets.max_steps
                || timeout>remaining_run_millis(&r.record,now()) {return Err(Error::BudgetExceeded);}
            r.checkpoint.in_flight=Some(tool.clone());
            let mut change=Change::event("mcp_dispatch_committed",json!({"attempt_id":attempt.record.id,"tool_call_id":tool.id,
                "request_sha256":request,"transport_request_sha256":attempt.record.transport_request_sha256,"rpc_request_id":attempt.record.rpc_request_id}));
            change.claim_folders=true;
            change.mcp_write=Some(McpWrite{stored:attempt.clone(),expected_revision:None,prepared_epoch:Some(prepared_epoch),completion:None});
            Ok(change)
        }).await;
        match commit {
            Ok(_) => Ok(McpAdmission::Dispatch(McpDispatchPermit {
                attempt: attempt.record,
                prepared,
            })),
            Err(error) => match self.stored_mcp_attempt(run, &tool.id).await? {
                Some(existing) => replay(existing, &request),
                None => Err(error),
            },
        }
    }
    /// Does not grant another dispatch. Used by the trusted dispatcher immediately before send.
    pub async fn validate_mcp_dispatch(
        &self,
        run: &str,
        token: &str,
        attempt: &McpAttempt,
    ) -> Result<()> {
        let stored = self.stored_run(run).await?;
        stored.assert_lease(token, now())?;
        let current = self
            .stored_mcp_attempt(run, &attempt.tool.id)
            .await?
            .ok_or(Error::NotFound)?;
        if current.record != *attempt
            || current.lease_sha256 != hash(token.as_bytes())
            || !matches!(current.record.state, McpAttemptState::DispatchCommitted)
        {
            return Err(Error::Conflict);
        }
        in_flight(&stored, &current.record)?;
        if stored.record.state != RunState::Running || time_budget_exhausted(&stored.record, now())
        {
            return Err(Error::Conflict);
        }
        let server = self
            .mcp_server(
                &stored.record.spec.project_id,
                &current.record.binding.server_id,
            )
            .await?;
        if !server.bindings()?.contains(&current.record.binding) {
            return Err(Error::Forbidden);
        }
        self.mcp_server_credential(&server).await?;
        if let Some(connection) = self
            .mcp_connection(run, &current.record.binding.server_id)
            .await?
            && (connection.state
                != McpConnectionState::Busy {
                    tool_call_id: current.record.tool.id.clone(),
                }
                || connection.connection_id.as_ref() != Some(&current.record.connection_id))
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    pub async fn finish_mcp(
        &self,
        run: &str,
        token: &str,
        call: &str,
        result: McpResult,
    ) -> Result<McpAttempt> {
        self.finish_mcp_inner(run, token, call, result, None).await
    }
    pub async fn finish_mcp_session(
        &self,
        run: &str,
        token: &str,
        call: &str,
        result: McpResult,
        completion: McpSessionCompletion,
    ) -> Result<McpAttempt> {
        self.finish_mcp_inner(run, token, call, result, Some(completion))
            .await
    }
    async fn finish_mcp_inner(
        &self,
        run: &str,
        token: &str,
        call: &str,
        result: McpResult,
        completion: Option<McpSessionCompletion>,
    ) -> Result<McpAttempt> {
        let mut attempt = self
            .stored_mcp_attempt(run, call)
            .await?
            .ok_or(Error::NotFound)?;
        checked_result(&attempt.record.binding, &result)?;
        let state = McpAttemptState::Completed {
            result: result.clone(),
        };
        if attempt.lease_sha256 != hash(token.as_bytes()) {
            return Err(Error::StaleLease);
        }
        if attempt.record.state != McpAttemptState::DispatchCommitted {
            return replay_state(attempt, state);
        }
        attempt.record.state = state;
        attempt.record.revision = 2;
        attempt.record.updated_at = now();
        attempt.validate()?;
        let commit=self.mutate(run,Some(token),|r|{
            in_flight(r,&attempt.record)?;
            if !matches!(r.record.state,RunState::Running|RunState::CancelRequested) {return Err(Error::Conflict);}
            settle(r,&attempt.record.tool,&result.response);
            let mut change=Change::event("mcp_completed",json!({"attempt_id":attempt.record.id,"tool_call_id":call,"response_sha256":result.sha256}));
            if r.record.state==RunState::CancelRequested {
                r.transition(RunState::Canceled)?;r.lease=None;
                change.events.push(("run_canceled".into(),json!({"state":"canceled"})));
            }
            change.mcp_write=Some(McpWrite{stored:attempt.clone(),expected_revision:Some(1),prepared_epoch:None,completion:completion.clone()});
            Ok(change)
        }).await;
        self.mcp_commit_result(commit, attempt.record).await
    }
    /// Even a locally rejected dispatch cannot prove an older admitted request had no effect.
    pub async fn interrupt_mcp(
        &self,
        run: &str,
        token: &str,
        call: &str,
        code: &str,
        response: Option<Value>,
    ) -> Result<McpAttempt> {
        let mut attempt = self
            .stored_mcp_attempt(run, call)
            .await?
            .ok_or(Error::NotFound)?;
        if attempt.lease_sha256 != hash(token.as_bytes()) {
            return Err(Error::StaleLease);
        }
        let state = McpAttemptState::OutcomeUncertain {
            code: code.into(),
            response_sha256: response.as_ref().map(canonical_hash),
            response,
        };
        if attempt.record.state != McpAttemptState::DispatchCommitted {
            return replay_state(attempt, state);
        }
        attempt.record.state = state;
        attempt.record.revision = 2;
        attempt.record.updated_at = now();
        attempt.validate()?;
        let commit = self
            .mutate(run, Some(token), |r| {
                in_flight(r, &attempt.record)?;
                if !matches!(
                    r.record.state,
                    RunState::Running | RunState::CancelRequested
                ) {
                    return Err(Error::Conflict);
                }
                r.transition(RunState::Interrupted)?;
                r.lease = None;
                r.record.error_code = Some("uncertain_effect".into());
                let mut change = Change::event(
                    "mcp_outcome_uncertain",
                    json!({"attempt_id":attempt.record.id,"tool_call_id":call,"code":code}),
                );
                change.mcp_write = Some(McpWrite {
                    stored: attempt.clone(),
                    expected_revision: Some(1),
                    prepared_epoch: None,
                    completion: None,
                });
                Ok(change)
            })
            .await;
        self.mcp_commit_result(commit, attempt.record).await
    }
    /// Private trusted review: evidence hashes are references, not authentication or proof of quiescence.
    pub async fn reconcile_mcp(
        &self,
        run: &str,
        call: &str,
        expected_revision: i64,
        request_sha256: &str,
        resolution: McpResolution,
    ) -> Result<McpAttempt> {
        if !(1..=2).contains(&expected_revision) {
            return Err(Error::Conflict);
        }
        let mut attempt = self
            .stored_mcp_attempt(run, call)
            .await?
            .ok_or(Error::NotFound)?;
        if attempt.record.request_sha256 != request_sha256 {
            return Err(Error::Conflict);
        }
        let (state, summary) = match resolution {
            McpResolution::Completed {
                result,
                evidence_sha256,
            } => {
                checked_result(&attempt.record.binding, &result)?;
                let summary = result.response.clone();
                (
                    McpAttemptState::ReconciledCompleted {
                        result,
                        evidence_sha256,
                    },
                    summary,
                )
            }
            McpResolution::NotExecuted { evidence_sha256 } => (
                McpAttemptState::ReconciledNotExecuted { evidence_sha256 },
                json!({"not_executed":true,"automatically_retried":false}),
            ),
        };
        if !pending(&attempt.record.state) {
            if attempt.record.revision != expected_revision + 1 {
                return Err(Error::Conflict);
            }
            return replay_state(attempt, state);
        }
        if attempt.record.revision != expected_revision {
            return Err(Error::Conflict);
        }
        attempt.record.state = state;
        attempt.record.revision += 1;
        attempt.record.updated_at = now();
        attempt.validate()?;
        let commit=self.mutate(run,None,|r|{
            if r.record.state!=RunState::Interrupted || r.lease.is_some() {return Err(Error::Conflict);}
            in_flight(r,&attempt.record)?;
            settle(r,&attempt.record.tool,&summary);
            r.record.error_code=Some("mcp_interrupted".into());
            let mut change=Change::event("mcp_reconciled",json!({"attempt_id":attempt.record.id,"tool_call_id":call,"automatic_retry":false}));
            change.mcp_write=Some(McpWrite{stored:attempt.clone(),expected_revision:Some(expected_revision),prepared_epoch:None,completion:None});
            Ok(change)
        }).await;
        self.mcp_commit_result(commit, attempt.record).await
    }
    async fn mcp_commit_result(
        &self,
        commit: Result<StoredRun>,
        proposed: McpAttempt,
    ) -> Result<McpAttempt> {
        match commit {
            Ok(_) => Ok(proposed),
            Err(error) => {
                if let Some(saved) = self
                    .stored_mcp_attempt(&proposed.run_id, &proposed.tool.id)
                    .await?
                    && saved.record.state == proposed.state
                {
                    return Ok(saved.record);
                }
                Err(error)
            }
        }
    }
}
fn replay(attempt: StoredMcpAttempt, request: &str) -> Result<McpAdmission> {
    if attempt.record.request_sha256 != request {
        return Err(Error::Conflict);
    }
    Ok(McpAdmission::Existing(attempt.record))
}
fn replay_state(attempt: StoredMcpAttempt, state: McpAttemptState) -> Result<McpAttempt> {
    if attempt.record.state == state {
        Ok(attempt.record)
    } else {
        Err(Error::Conflict)
    }
}
fn pending_tool(run: &StoredRun, tool: &ToolCall) -> Result<()> {
    if run.checkpoint.pending_tools.first() == Some(tool) {
        Ok(())
    } else {
        Err(Error::Conflict)
    }
}
fn in_flight(run: &StoredRun, attempt: &McpAttempt) -> Result<()> {
    pending_tool(run, &attempt.tool)?;
    if run.checkpoint.in_flight.as_ref() != Some(&attempt.tool)
        || mcp_request_hash(&run.record, &attempt.tool) != attempt.request_sha256
    {
        return Err(Error::Conflict);
    }
    Ok(())
}
fn settle(run: &mut StoredRun, tool: &ToolCall, result: &Value) {
    run.checkpoint.pending_tools.remove(0);
    run.checkpoint.in_flight = None;
    run.record.steps += 1;
    run.checkpoint
        .messages
        .push(json!({"role":"tool","tool_call_id":tool.id,"content":result.to_string()}));
}
fn tool_catalog(binding: &McpToolBinding) -> Result<cowork_mcp::Catalog> {
    cowork_mcp::Catalog::from_definitions(vec![binding.definition.clone()])
        .map_err(|_| Error::Forbidden)
}
fn checked_result(binding: &McpToolBinding, result: &McpResult) -> Result<()> {
    validate_result(result)?;
    if result.response.get("isError") == Some(&Value::Bool(true)) {
        return Err(Error::UncertainEffect);
    }
    tool_catalog(binding)?
        .validate_result(
            binding.definition["name"]
                .as_str()
                .ok_or(Error::Forbidden)?,
            &result.response,
        )
        .map_err(|_| Error::Validation("Invalid MCP result".into()))
}
fn checked_attempt(row: &sqlx::any::AnyRow, run: &RunRecord) -> Result<StoredMcpAttempt> {
    let attempt: StoredMcpAttempt =
        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    attempt.validate_binding(run)?;
    match &attempt.record.state {
        McpAttemptState::Completed { result }
        | McpAttemptState::ReconciledCompleted { result, .. } => {
            checked_result(&attempt.record.binding, result)?;
        }
        _ => {}
    }
    if attempt.record.id != row.try_get::<String, _>("id").map_err(storage_error)?
        || attempt.record.tool.id
            != row
                .try_get::<String, _>("tool_call_id")
                .map_err(storage_error)?
        || attempt.record.revision != row.try_get::<i64, _>("revision").map_err(storage_error)?
    {
        return Err(Error::Forbidden);
    }
    Ok(attempt)
}
pub(super) fn records(rows: Vec<sqlx::any::AnyRow>, run: &RunRecord) -> Result<Vec<McpAttempt>> {
    rows.iter()
        .map(|row| checked_attempt(row, run).map(|a| a.record))
        .collect()
}
pub(super) async fn write_attempt(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    run: &StoredRun,
    write: &McpWrite,
) -> Result<()> {
    let stored = &write.stored;
    stored.validate_binding(&run.record)?;
    let a = &stored.record;
    let changed = match write.expected_revision {
        None => {
            if a.state != McpAttemptState::DispatchCommitted
                || run.record.state != RunState::Running
                || run.checkpoint.in_flight.as_ref() != Some(&a.tool)
                || run.checkpoint.pending_tools.first() != Some(&a.tool)
                || run
                    .lease
                    .as_ref()
                    .map(|l| hash(l.token.as_bytes()))
                    .as_ref()
                    != Some(&stored.lease_sha256)
                || evaluate(&run.record, &a.tool, &[], &[], &run.checkpoint.approvals)
                    != Decision::Allow
            {
                return Err(Error::Forbidden);
            }
            sqlx::query("INSERT INTO mcp_attempts(id,run_id,tool_call_id,revision,payload) VALUES($1,$2,$3,1,$4) ON CONFLICT(run_id,tool_call_id) DO NOTHING")
                .bind(&a.id).bind(&a.run_id).bind(&a.tool.id).bind(json_string(stored)?)
                .execute(&mut **tx).await.map_err(storage_error)?.rows_affected()
        }
        Some(revision) => {
            let row=sqlx::query("SELECT id,tool_call_id,revision,payload FROM mcp_attempts WHERE id=$1 AND run_id=$2 AND revision=$3")
                .bind(&a.id).bind(&a.run_id).bind(revision).fetch_optional(&mut **tx).await.map_err(storage_error)?.ok_or(Error::Conflict)?;
            let mut old = checked_attempt(&row, &run.record)?;
            if !pending(&old.record.state)
                || a.revision != revision + 1
                || a.state == McpAttemptState::DispatchCommitted
                || (matches!(old.record.state, McpAttemptState::OutcomeUncertain { .. })
                    && !matches!(
                        a.state,
                        McpAttemptState::ReconciledCompleted { .. }
                            | McpAttemptState::ReconciledNotExecuted { .. }
                    ))
                || (pending(&a.state)
                    && (run.checkpoint.in_flight.as_ref() != Some(&a.tool)
                        || run.record.state != RunState::Interrupted))
                || (!pending(&a.state)
                    && (run.checkpoint.in_flight.is_some()
                        || run
                            .checkpoint
                            .pending_tools
                            .iter()
                            .any(|t| t.id == a.tool.id)))
            {
                return Err(Error::Conflict);
            }
            old.record.state = a.state.clone();
            old.record.updated_at = a.updated_at;
            old.record.revision = a.revision;
            if canonical_hash(&old) != canonical_hash(stored) {
                return Err(Error::Forbidden);
            }
            sqlx::query("UPDATE mcp_attempts SET revision=$1,payload=$2 WHERE id=$3 AND run_id=$4 AND revision=$5")
                .bind(a.revision).bind(json_string(stored)?).bind(&a.id).bind(&a.run_id).bind(revision)
                .execute(&mut **tx).await.map_err(storage_error)?.rows_affected()
        }
    };
    if changed != 1 {
        return Err(Error::Conflict);
    }
    match write.expected_revision {
        None => {
            mcp_connections::admit(
                tx,
                &run.record,
                a,
                write.prepared_epoch.ok_or(Error::Forbidden)?,
            )
            .await?
        }
        Some(_) => mcp_connections::settle(tx, &run.record, a, write.completion.as_ref()).await?,
    }
    Ok(())
}
