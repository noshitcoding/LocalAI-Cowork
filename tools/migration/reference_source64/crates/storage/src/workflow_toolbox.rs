//! Durable native tool admission for an existing saved workflow proposal.
//! A pending admission can never be reconstructed as another dispatch permit.
use super::*;
use crate::workflows::{WorkflowWrite, tools};
use cowork_domain::workflows::{WorkflowAdmission, WorkflowOperation};
use cowork_policy::{Decision, evaluate};

pub enum NativeWorkflowAdmission {
    Dispatch { record: RunRecord, tool: ToolCall },
    Completed(WorkflowToolReceipt),
    AwaitingApproval(Approval),
}
fn exact(record: &RunRecord, tool: &ToolCall, approval: &Approval) -> bool {
    approval.run_id == record.id
        && approval.tool == *tool
        && approval.target == record.spec.executor
        && approval.policy_revision == record.spec.policy_revision
        && approval.fingerprint == approval_fingerprint(record, tool)
}
fn native(name: &str) -> bool {
    matches!(
        name,
        "execute_program"
            | "browser"
            | "office"
            | "github"
            | "list_files"
            | "read_text_file"
            | "stat_file"
            | "write_text_file"
            | "create_directory"
            | "rename_file"
            | "remove_file"
            | "restore_file"
    ) || name.starts_with("mcp_")
}
impl Store {
    pub async fn admit_workflow_toolbox(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        turn_id: &str,
        raw_id: &str,
    ) -> Result<NativeWorkflowAdmission> {
        self.assert_workflow_clock_authority(project, run, lease)
            .await?;
        let parent = self.stored_run(run).await?;
        if parent.record.spec.project_id != project {
            return Err(Error::NotFound);
        }
        parent.assert_lease(lease, now())?;
        tools::current_authority(&parent)?;
        let before = self.stored_workflow(project, run).await?;
        let (turn, tool) = tools::proposal(&before, turn_id, raw_id)?;
        if let Some(receipt) = turn.tool_results.iter().find(|r| r.id == tool.id) {
            return Ok(NativeWorkflowAdmission::Completed(receipt.clone()));
        }
        tools::pending(&before, turn, raw_id)?;
        tools::reject_stopped(&before)?;
        tools::tool_deadline(&parent, &before, turn)?;
        let record = tools::scoped_record(&parent.record, &before, turn)?;
        if !native(&tool.name) {
            return Err(Error::Forbidden);
        }
        let decision = evaluate(&record, &tool, &[], &[], &parent.checkpoint.approvals);
        if decision == Decision::ApprovalRequired {
            if let Some(a) = parent
                .checkpoint
                .approvals
                .iter()
                .find(|a| exact(&record, &tool, a))
            {
                return Ok(NativeWorkflowAdmission::AwaitingApproval(a.clone()));
            }
            let approval = Approval {
                id: id(),
                run_id: run.into(),
                tool: tool.clone(),
                target: record.spec.executor.clone(),
                policy_revision: record.spec.policy_revision,
                fingerprint: approval_fingerprint(&record, &tool),
                decision: None,
            };
            self.mutate(run,Some(lease),|r|{tools::current_authority(r)?;if r.checkpoint.approvals.iter().any(|a|a.tool.id==tool.id){return Err(Error::Conflict);}r.checkpoint.approvals.push(approval.clone());let mut change=Change::event("approval_requested",json!({"approval":approval,"node_id":turn.claim.node_id,"model_turn_id":turn_id,"raw_tool_call_id":raw_id,"mode":"owner_native","filesystem_isolation":false}));change.workflow_tool_gate=Some(tools::ToolGate::Request{turn:turn_id.into(),raw_id:raw_id.into(),approval:approval.id.clone()});Ok(change)}).await?;
            return Ok(NativeWorkflowAdmission::AwaitingApproval(approval));
        }
        if decision != Decision::Allow
            || ((matches!(tool.name.as_str(), "browser" | "office" | "github")
                || (tool.name == "execute_program" && record.spec.environment.is_none()))
                && !parent
                    .checkpoint
                    .approvals
                    .iter()
                    .any(|a| exact(&record, &tool, a) && a.decision == Some(true)))
        {
            return Err(Error::Forbidden);
        }
        if before.progress.attempts().contains_key(&tool.id) {
            return Err(Error::UncertainEffect);
        }
        let mut next = before.clone();
        let admission = next.progress.admit(
            &next.plan,
            before.progress.revision(),
            tools::execution_claim(&before, turn)?,
            &tool.id,
            WorkflowOperation::Tool {
                tool_id: tool.name.clone(),
                arguments_sha256: canonical_hash(&tool.arguments),
            },
            now().div_euclid(1000),
        )?;
        if !matches!(admission, WorkflowAdmission::Dispatch(_)) {
            return Err(Error::UncertainEffect);
        }
        self.mutate(run,Some(lease),|r|{tools::current_authority(r)?;let mut change=Change::event("workflow_native_tool_started",json!({"node_id":turn.claim.node_id,"model_turn_id":turn_id,"tool_call_id":tool.id,"name":tool.name,"mode":"owner_native","filesystem_isolation":false}));change.workflow_write=Some(WorkflowWrite{new_action:true,expected_revision:before.progress.revision(),expected_sha256:canonical_hash(&before.progress),next:next.clone()});change.workflow_tool_gate=Some(tools::ToolGate::ExternalStart{turn:turn_id.into(),raw_id:raw_id.into()});Ok(change)}).await?;
        Ok(NativeWorkflowAdmission::Dispatch { record, tool })
    }
    pub async fn assert_workflow_toolbox_dispatch(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        turn: &str,
        raw_id: &str,
    ) -> Result<()> {
        self.assert_workflow_clock_authority(project, run, lease)
            .await?;
        // Reuse installed mutation lock ordering and the final clock fence.
        self.mutate(run, Some(lease), |parent| {
            if parent.record.spec.project_id != project {
                return Err(Error::NotFound);
            }
            tools::current_authority(parent)?;
            Ok(Change {
                workflow_tool_gate: Some(tools::ToolGate::ExternalCheck {
                    turn: turn.into(),
                    raw_id: raw_id.into(),
                }),
                ..Default::default()
            })
        })
        .await?;
        Ok(())
    }
    pub async fn finish_workflow_toolbox(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        turn_id: &str,
        raw_id: &str,
        result: Value,
    ) -> Result<WorkflowToolReceipt> {
        if json_string(&result)?.len() > 2 * 1024 * 1024 {
            return Err(Error::Validation("Native tool response too large".into()));
        }
        let parent = self.stored_run(run).await?;
        parent.assert_lease(lease, now())?;
        tools::current_authority(&parent)?;
        let before = self.stored_workflow(project, run).await?;
        let (turn, tool) = tools::proposal(&before, turn_id, raw_id)?;
        if let Some(receipt) = turn.tool_results.iter().find(|r| r.id == tool.id) {
            return Ok(receipt.clone());
        }
        tools::pending(&before, turn, raw_id)?;
        let raw = turn
            .response
            .as_ref()
            .ok_or(Error::Forbidden)?
            .tools
            .get(turn.tool_results.len())
            .ok_or(Error::Forbidden)?
            .clone();
        let succeeded = result
            .get("_workflow_success")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| {
                result.pointer("/outcome/exit_code").and_then(Value::as_i64) == Some(0)
                    && result
                        .pointer("/outcome/termination")
                        .and_then(Value::as_str)
                        == Some("exited")
                    && result.get("state").and_then(Value::as_str) == Some("completed")
            });
        let receipt = WorkflowToolReceipt {
            id: tool.id.clone(),
            tool: raw,
            result_sha256: canonical_hash(&result),
            result,
            succeeded,
        };
        let mut next = before.clone();
        next.progress.settle_tool_response(
            &next.plan,
            before.progress.revision(),
            tools::execution_claim(&before, turn)?,
            &tool.id,
            &canonical_hash(&receipt),
            succeeded,
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
        self.mutate(run, Some(lease), |r| {
            tools::current_authority(r)?;
            let mut change = Change::event(
                "workflow_native_tool_completed",
                json!({"node_id":turn.claim.node_id,"model_turn_id":turn_id,"receipt":receipt}),
            );
            change.workflow_write = Some(WorkflowWrite {
                new_action: false,
                expected_revision: before.progress.revision(),
                expected_sha256: canonical_hash(&before.progress),
                next: next.clone(),
            });
            change.workflow_tool_gate = Some(tools::ToolGate::ExternalFinish {
                turn: turn_id.into(),
                raw_id: raw_id.into(),
            });
            if let Some(value) = receipt.result.get("change") {
                if let Ok(change_record) = serde_json::from_value::<FileChange>(value.clone()) {
                    change.file_changes.push(change_record);
                }
            }
            if let Some(value) = receipt.result.get("operation_receipt") {
                if let Ok(operation) = serde_json::from_value::<FileOperation>(value.clone()) {
                    change.file_operations.push(operation);
                }
            }
            Ok(change)
        })
        .await?;
        Ok(receipt)
    }
}
