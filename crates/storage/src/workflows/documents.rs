//! Trusted document precheck results. This path can record failure, never IO.
use super::*;

pub struct WorkflowDocumentContext {
    pub record: RunRecord,
    pub tool: ToolCall,
    pub completed: Option<WorkflowToolReceipt>,
    pub deadline_ms: i64,
    pub engine_revision: Option<String>,
}

pub(crate) fn document_tool(name: &str) -> bool {
    matches!(name, "document_read" | "document_write" | "image_edit")
}

/// Separate bounded document destination. The process-free broker predicate is
/// intentionally unchanged. A scope check never selects or authorizes a runtime.
pub fn document_execution_scope(spec: &RunSpec) -> bool {
    spec.environment.is_none()
        && spec.executor == "local-v2"
        && spec.mcp_tools.is_empty()
        && spec.file_action.is_none()
        && spec.allowed_tools.iter().all(|tool| {
            cowork_domain::run_workspaces::BROKER_TOOLS.contains(&tool.as_str())
                || document_tool(tool)
        })
}

/// Logical targets must belong to the already narrowed node scope. The native
/// broker independently checks the real copy, current grant, revision and path.
pub(crate) fn document_targets(record: &RunRecord, tool: &ToolCall) -> Result<()> {
    let call = cowork_domain::documents::parse_document_call(tool)?;
    if call.inputs.iter().any(|input| {
        !record
            .spec
            .folders
            .iter()
            .any(|folder| folder.id == input.folder_id)
    }) {
        return Err(Error::Forbidden);
    }
    if let Some(output) = call.output {
        let folder = match output.folder_id {
            Some(id) => record.spec.folders.iter().find(|folder| folder.id == id),
            None => record.spec.folders.iter().find(|folder| {
                folder.writable && folder.order_id.as_deref() == Some(record.spec.order_id.as_str())
            }),
        };
        if !folder.is_some_and(|folder| folder.writable) {
            return Err(Error::Forbidden);
        }
    }
    Ok(())
}

impl Store {
    /// Reads the saved proposal under its live lease. No dispatch permit is made.
    pub async fn workflow_document_context(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        turn_id: &str,
        raw_id: &str,
    ) -> Result<WorkflowDocumentContext> {
        self.assert_workflow_clock_authority(project, run, lease)
            .await?;
        let parent = self.stored_run(run).await?;
        if parent.record.spec.project_id != project {
            return Err(Error::NotFound);
        }
        parent.assert_lease(lease, now())?;
        tools::current_authority(&parent)?;
        let workflow = self.stored_workflow(project, run).await?;
        let (turn, tool) = tools::proposal(&workflow, turn_id, raw_id)?;
        if !document_tool(&tool.name) {
            return Err(Error::Forbidden);
        }
        let record = tools::scoped_record(&parent.record, &workflow, turn)?;
        if !record.spec.allowed_tools.contains(&tool.name) || record.spec.environment.is_some() {
            return Err(Error::Forbidden);
        }
        let deadline_ms = tools::tool_deadline(&parent, &workflow, turn)?;
        let completed = turn
            .tool_results
            .iter()
            .find(|receipt| receipt.id == tool.id)
            .cloned();
        if completed.is_none() {
            tools::pending(&workflow, turn, raw_id)?;
            tools::reject_stopped(&workflow)?;
        }
        let engine_revision = self.pinned_document_engine_revision(&record.spec).await?;
        Ok(WorkflowDocumentContext {
            record,
            tool,
            completed,
            deadline_ms,
            engine_revision,
        })
    }

    /// A failed precheck consumes one existing graph step and can be shown before
    /// approval. It never returns a permit, executes an engine or writes a file.
    pub async fn reject_workflow_document_precheck(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        turn_id: &str,
        raw_id: &str,
        error: &Error,
    ) -> Result<WorkflowToolReceipt> {
        let context = self
            .workflow_document_context(project, run, lease, turn_id, raw_id)
            .await?;
        if let Some(receipt) = context.completed {
            return Ok(receipt);
        }
        let before = self.stored_workflow(project, run).await?;
        let (turn, tool) = tools::proposal(&before, turn_id, raw_id)?;
        if before.progress.attempts().contains_key(&tool.id) {
            return Err(Error::UncertainEffect);
        }
        let raw = turn
            .response
            .as_ref()
            .ok_or(Error::Forbidden)?
            .tools
            .get(turn.tool_results.len())
            .ok_or(Error::Forbidden)?
            .clone();
        let result =
            json!({"error":error.code(),"message":error.to_string(),"precheck_rejected":true});
        let receipt = WorkflowToolReceipt {
            id: tool.id.clone(),
            tool: raw,
            result_sha256: canonical_hash(&result),
            result,
            succeeded: false,
        };
        let mut next = before.clone();
        next.progress.record_tool_result(
            &next.plan,
            before.progress.revision(),
            tools::execution_claim(&before, turn)?,
            &tool.id,
            &tool.name,
            &canonical_hash(&tool.arguments),
            &canonical_hash(&receipt),
            false,
            now().div_euclid(1000),
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
        self.mutate(run, Some(lease), |parent| {
            tools::current_authority(parent)?;
            let mut change = Change::event(
                "workflow_document_precheck_rejected",
                json!({"node_id":turn.claim.node_id,"model_turn_id":turn_id,"receipt":receipt}),
            );
            change.workflow_write = Some(WorkflowWrite {
                new_action: true,
                expected_revision: before.progress.revision(),
                expected_sha256: canonical_hash(&before.progress),
                next: next.clone(),
            });
            change.workflow_tool_gate = Some(tools::ToolGate::DocumentRejected {
                turn: turn_id.into(),
                raw_id: raw_id.into(),
            });
            Ok(change)
        })
        .await?;
        Ok(receipt)
    }
}
