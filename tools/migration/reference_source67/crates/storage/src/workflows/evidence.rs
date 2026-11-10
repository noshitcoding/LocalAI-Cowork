//! Source-only outcome journal. No HTTP/MCP input can manufacture a completed
//! evidence row, and reading one never grants another external dispatch.
use super::*;
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowExternalEvidence {
    pub run_id: String,
    pub turn_id: String,
    pub raw_id: String,
    pub tool: ToolCall,
    pub scope_sha256: String,
    pub kind: String,
    pub preparation: Value,
    pub preparation_sha256: String,
    pub result: Option<Value>,
    pub result_sha256: Option<String>,
}
impl WorkflowExternalEvidence {
    pub fn sha256(&self) -> String {
        canonical_hash(self)
    }
    fn validate(&self) -> Result<()> {
        if !matches!(
            self.kind.as_str(),
            "file_write"
                | "file_operation"
                | "file_read"
                | "process_runner"
                | "mcp"
                | "native_toolbox"
        ) || self.preparation_sha256
            != canonical_hash(
                &json!({"run":self.run_id,"turn":self.turn_id,"raw":self.raw_id,"tool":self.tool,"scope":self.scope_sha256,"kind":self.kind,"preparation":self.preparation}),
            )
            || self.result.as_ref().map(canonical_hash) != self.result_sha256
            || json_string(self)?.len() > 4 * 1024 * 1024
        {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
}
impl Store {
    pub async fn workflow_external_evidence(
        &self,
        project: &str,
        run: &str,
        call: &str,
    ) -> Result<Option<WorkflowExternalEvidence>> {
        let parent = self.stored_run(run).await?;
        if parent.record.spec.project_id != project {
            return Err(Error::NotFound);
        }
        let row=sqlx::query("SELECT preparation_sha256,result_sha256,payload FROM workflow_external_evidence WHERE run_id=$1 AND tool_call_id=$2").bind(run).bind(call).fetch_optional(&self.pool).await.map_err(storage_error)?;
        let Some(row) = row else { return Ok(None) };
        let record: WorkflowExternalEvidence =
            parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
        record.validate()?;
        if record.run_id != run
            || record.tool.id != call
            || record.preparation_sha256
                != row
                    .try_get::<String, _>("preparation_sha256")
                    .map_err(storage_error)?
            || record.result_sha256
                != row
                    .try_get::<Option<String>, _>("result_sha256")
                    .map_err(storage_error)?
        {
            return Err(Error::Forbidden);
        }
        let workflow = self.stored_workflow(project, run).await?;
        let (turn, tool) = tools::proposal(&workflow, &record.turn_id, &record.raw_id)?;
        if tool != record.tool
            || record.scope_sha256
                != canonical_hash(&tools::scoped_record(&parent.record, &workflow, turn)?.spec)
        {
            return Err(Error::Forbidden);
        }
        Ok(Some(record))
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn retain_workflow_external_preparation(
        &self,
        project: &str,
        run: &str,
        turn_id: &str,
        raw_id: &str,
        kind: &str,
        preparation: Value,
    ) -> Result<WorkflowExternalEvidence> {
        let parent = self.stored_run(run).await?;
        if parent.record.spec.project_id != project {
            return Err(Error::NotFound);
        }
        let workflow = self.stored_workflow(project, run).await?;
        let (turn, tool) = tools::proposal(&workflow, turn_id, raw_id)?;
        let attempt = workflow
            .progress
            .attempts()
            .get(&tool.id)
            .ok_or(Error::Forbidden)?;
        if attempt.outcome.is_some()
            || attempt.operation
                != (WorkflowOperation::Tool {
                    tool_id: tool.name.clone(),
                    arguments_sha256: canonical_hash(&tool.arguments),
                })
        {
            return Err(Error::Conflict);
        }
        let mut record = WorkflowExternalEvidence {
            run_id: run.into(),
            turn_id: turn_id.into(),
            raw_id: raw_id.into(),
            tool,
            scope_sha256: canonical_hash(
                &tools::scoped_record(&parent.record, &workflow, turn)?.spec,
            ),
            kind: kind.into(),
            preparation,
            preparation_sha256: String::new(),
            result: None,
            result_sha256: None,
        };
        record.preparation_sha256 = canonical_hash(
            &json!({"run":record.run_id,"turn":record.turn_id,"raw":record.raw_id,"tool":record.tool,"scope":record.scope_sha256,"kind":record.kind,"preparation":record.preparation}),
        );
        record.validate()?;
        sqlx::query("INSERT INTO workflow_external_evidence(run_id,tool_call_id,preparation_sha256,payload) VALUES($1,$2,$3,$4) ON CONFLICT(run_id,tool_call_id) DO NOTHING").bind(run).bind(&record.tool.id).bind(&record.preparation_sha256).bind(json_string(&record)?).execute(&self.pool).await.map_err(storage_error)?;
        let retained = self
            .workflow_external_evidence(project, run, &record.tool.id)
            .await?
            .ok_or(Error::Forbidden)?;
        if retained.preparation_sha256 != record.preparation_sha256 {
            return Err(Error::Conflict);
        }
        Ok(retained)
    }
    /// Called by the physically owned source adapter before releasing its lock.
    /// This can survive loss of the parent lease, but does not resume the workflow.
    pub async fn retain_workflow_external_outcome(
        &self,
        project: &str,
        run: &str,
        call: &str,
        result: Value,
    ) -> Result<WorkflowExternalEvidence> {
        let mut record = self
            .workflow_external_evidence(project, run, call)
            .await?
            .ok_or(Error::Forbidden)?;
        let digest = canonical_hash(&result);
        if record
            .result_sha256
            .as_ref()
            .is_some_and(|old| old != &digest)
        {
            return Err(Error::Conflict);
        }
        record.result = Some(result);
        record.result_sha256 = Some(digest.clone());
        record.validate()?;
        sqlx::query("UPDATE workflow_external_evidence SET result_sha256=$1,payload=$2 WHERE run_id=$3 AND tool_call_id=$4 AND preparation_sha256=$5 AND (result_sha256 IS NULL OR result_sha256=$1)").bind(&digest).bind(json_string(&record)?).bind(run).bind(call).bind(&record.preparation_sha256).execute(&self.pool).await.map_err(storage_error)?;
        let retained = self
            .workflow_external_evidence(project, run, call)
            .await?
            .ok_or(Error::Forbidden)?;
        if retained.result_sha256.as_ref() != Some(&digest) {
            return Err(Error::Conflict);
        }
        Ok(retained)
    }
    /// Accepts only the protected source row; caller cannot supply outcome JSON.
    pub async fn reconcile_workflow_external_outcome(
        &self,
        project: &str,
        run: &str,
        call: &str,
    ) -> Result<RunRecord> {
        let evidence = self
            .workflow_external_evidence(project, run, call)
            .await?
            .ok_or(Error::NotFound)?;
        let result = evidence.result.clone().ok_or(Error::UncertainEffect)?;
        let succeeded = result
            .get("_workflow_success")
            .and_then(Value::as_bool)
            .ok_or(Error::Forbidden)?;
        let parent = self.stored_run(run).await?;
        if parent.record.state != RunState::Interrupted || parent.lease.is_some() {
            return Err(Error::Conflict);
        }
        let before = self.stored_workflow(project, run).await?;
        let (turn, tool) = tools::proposal(&before, &evidence.turn_id, &evidence.raw_id)?;
        if turn.tool_results.iter().any(|r| r.id == tool.id) {
            return Ok(parent.record);
        }
        let raw = turn
            .response
            .as_ref()
            .ok_or(Error::Forbidden)?
            .tools
            .get(turn.tool_results.len())
            .ok_or(Error::Forbidden)?
            .clone();
        if raw.id != evidence.raw_id {
            return Err(Error::Conflict);
        }
        let receipt = WorkflowToolReceipt {
            id: call.into(),
            tool: raw,
            result_sha256: canonical_hash(&result),
            result,
            succeeded,
        };
        let mut next = before.clone();
        next.progress.reconcile_external_tool(
            &next.plan,
            before.progress.revision(),
            call,
            &canonical_hash(&receipt),
            succeeded,
            &evidence.sha256(),
            seconds(),
        )?;
        next.agents
            .get_mut(&turn.claim.node_id)
            .ok_or(Error::Forbidden)?
            .turns
            .last_mut()
            .ok_or(Error::Forbidden)?
            .tool_results
            .push(receipt.clone());
        next.format_version = next.format_version.max(2);
        let changed=self.mutate(run,None,|r|{
            if r.record.revision!=parent.record.revision||r.record.state!=RunState::Interrupted||r.lease.is_some(){return Err(Error::Conflict);}
            let mut change=Change::event("workflow_external_effect_reconciled",json!({"node_id":turn.claim.node_id,"turn_id":evidence.turn_id,"receipt":receipt,"evidence_sha256":evidence.sha256(),"automatic_retry":false}));
            change.workflow_write=Some(WorkflowWrite{new_action:false,expected_revision:before.progress.revision(),expected_sha256:canonical_hash(&before.progress),next:next.clone()});
            if let Some(v)=receipt.result.get("change"){change.file_changes.push(serde_json::from_value(v.clone()).map_err(storage_error)?);}
            if let Some(v)=receipt.result.get("operation_receipt"){change.file_operations.push(serde_json::from_value(v.clone()).map_err(storage_error)?);}
            Ok(change)
        }).await?;
        Ok(changed.record)
    }
}
