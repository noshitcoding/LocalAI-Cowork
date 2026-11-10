//! Database-only workflow broker. External IO requires its own durable admission.
use super::*;
use cowork_policy::{Decision, evaluate};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkflowToolReceipt {
    /// Domain-separated run/node/turn/call identity; raw provider ID stays in tool.
    pub id: String,
    pub tool: ToolCall,
    pub result: Value,
    pub result_sha256: String,
    pub succeeded: bool,
}
#[derive(Clone, Debug)]
pub enum WorkflowToolExecution {
    Completed(WorkflowToolReceipt),
    AwaitingApproval(Approval),
    AwaitingInput(cowork_workflow_contracts::WorkflowInputView),
}
pub(crate) enum ToolGate {
    Request {
        turn: String,
        raw_id: String,
        approval: String,
    },
    Resolve {
        approval: String,
    },
    InputOpen {
        turn: String,
        raw_id: String,
    },
    InputAnswer {
        request: String,
        fingerprint: String,
    },
    Effect {
        turn: String,
        raw_id: String,
        succeeded: bool,
    },
    ExternalStart {
        turn: String,
        raw_id: String,
    },
    ExternalFinish {
        turn: String,
        raw_id: String,
    },
    ExternalCheck {
        turn: String,
        raw_id: String,
    },
}
pub(super) fn broker_id(run: &str, turn: &WorkflowModelTurn, raw: &str) -> String {
    canonical_hash(
        &json!({"format":"cowork-workflow-tool-v1","run":run,"node":turn.claim.node_id,"turn":turn.id,"raw_id":raw}),
    )
}
pub(crate) fn proposal<'a>(
    workflow: &'a StoredWorkflow,
    turn_id: &str,
    raw: &str,
) -> Result<(&'a WorkflowModelTurn, ToolCall)> {
    let turn = agents::find_turn(workflow, turn_id).ok_or(Error::NotFound)?;
    let response = turn.response.as_ref().ok_or(Error::UncertainEffect)?;
    if response.rejection_code.is_some() {
        return Err(Error::Forbidden);
    }
    let call = response
        .tools
        .iter()
        .find(|t| t.id == raw)
        .ok_or(Error::NotFound)?;
    let mut scoped = call.clone();
    scoped.id = broker_id(&workflow.run_id, turn, raw);
    Ok((turn, scoped))
}
pub(crate) fn scoped_record(
    run: &RunRecord,
    workflow: &StoredWorkflow,
    turn: &WorkflowModelTurn,
) -> Result<RunRecord> {
    let mut projected = run.clone();
    projected.spec = agents::spec_for(run, workflow, &turn.claim.node_id)?;
    Ok(projected)
}
pub(crate) fn pending(
    workflow: &StoredWorkflow,
    turn: &WorkflowModelTurn,
    raw: &str,
) -> Result<()> {
    execution_claim(workflow, turn)?;
    if workflow.agents[&turn.claim.node_id]
        .turns
        .last()
        .map(|t| &t.id)
        != Some(&turn.id)
    {
        return Err(Error::Conflict);
    }
    let response = turn.response.as_ref().ok_or(Error::UncertainEffect)?;
    if response
        .tools
        .get(turn.tool_results.len())
        .map(|t| t.id.as_str())
        != Some(raw)
    {
        return Err(Error::Conflict);
    }
    Ok(())
}
pub(crate) fn execution_claim<'a>(
    workflow: &'a StoredWorkflow,
    turn: &'a WorkflowModelTurn,
) -> Result<&'a WorkflowClaim> {
    if agents::active(workflow, &turn.claim).is_ok() {
        return Ok(&turn.claim);
    }
    workflow
        .progress
        .human_authority(&turn.id, &turn.claim)
        .or_else(|_| workflow.progress.settled_authority(&turn.id, &turn.claim))
}
pub(crate) fn current_authority(parent: &StoredRun) -> Result<()> {
    if parent.record.state != RunState::Running {
        return Err(Error::Conflict);
    }
    if time_budget_exhausted(&parent.record, now()) {
        return Err(Error::BudgetExceeded);
    }
    let lease = parent.lease.as_ref().ok_or(Error::StaleLease)?;
    parent.assert_lease(&lease.token, now())
}
fn exact_approval(a: &Approval, run: &RunRecord, tool: &ToolCall) -> bool {
    a.run_id == run.id
        && a.tool == *tool
        && a.target == run.spec.executor
        && a.policy_revision == run.spec.policy_revision
        && a.fingerprint == approval_fingerprint(run, tool)
}
pub(crate) fn reject_stopped(workflow: &StoredWorkflow) -> Result<()> {
    if workflow.plan.definition.failure == WorkflowFailure::Stop
        && workflow.progress.nodes().values().any(|n| {
            matches!(
                n.state,
                WorkflowNodeState::Failed { .. } | WorkflowNodeState::Blocked
            )
        })
    {
        return Err(Error::Provider("workflow_stopped".into()));
    }
    Ok(())
}
pub(crate) fn tool_deadline(
    parent: &StoredRun,
    workflow: &StoredWorkflow,
    turn: &WorkflowModelTurn,
) -> Result<i64> {
    let deadline = record_tool_deadline(&parent.record, workflow, turn)?;
    if now() >= deadline {
        return Err(Error::BudgetExceeded);
    }
    Ok(deadline)
}
pub(super) fn record_tool_deadline(
    record: &RunRecord,
    workflow: &StoredWorkflow,
    turn: &WorkflowModelTurn,
) -> Result<i64> {
    let mut deadline = record
        .started_at
        .ok_or(Error::Forbidden)?
        .saturating_add(record.spec.budgets.max_seconds.saturating_mul(1000) as i64);
    let mut node_id = Some(turn.claim.node_id.as_str());
    while let Some(id) = node_id {
        let node = &workflow.progress.nodes()[id];
        let end = node
            .started_at
            .ok_or(Error::Forbidden)?
            .saturating_mul(1000)
            .saturating_add(node.plan.budgets.max_seconds.saturating_mul(1000) as i64);
        deadline = deadline.min(end);
        node_id = node.plan.parent.as_deref();
    }
    Ok(deadline)
}
/// Derives UI attention from current durable authority; never grants IO.
pub(super) fn actionable_approvals(
    parent: &StoredRun,
    workflow: &StoredWorkflow,
    revoked: bool,
) -> Result<Vec<(cowork_workflow_contracts::WorkflowRunApproval, i64)>> {
    if revoked || current_authority(parent).is_err() || reject_stopped(workflow).is_err() {
        return Ok(vec![]);
    }
    let lease_until = parent.lease.as_ref().ok_or(Error::StaleLease)?.until;
    let mut result = Vec::new();
    for approval in parent
        .checkpoint
        .approvals
        .iter()
        .filter(|a| a.decision.is_none())
    {
        for turn in workflow.agents.values().flat_map(|a| &a.turns) {
            let Some(response) = &turn.response else {
                continue;
            };
            if response.rejection_code.is_some() {
                continue;
            }
            for call in &response.tools {
                if broker_id(&workflow.run_id, turn, &call.id) != approval.tool.id {
                    continue;
                }
                if pending(workflow, turn, &call.id).is_err() {
                    continue;
                }
                let Ok(deadline) = tool_deadline(parent, workflow, turn) else {
                    continue;
                };
                let scoped = scoped_record(&parent.record, workflow, turn)?;
                let (_, tool) = proposal(workflow, &turn.id, &call.id)?;
                if !exact_approval(approval, &scoped, &tool)
                    || evaluate(&scoped, &tool, &[], &[], &parent.checkpoint.approvals)
                        != Decision::ApprovalRequired
                {
                    continue;
                }
                let node = &workflow.progress.nodes()[&turn.claim.node_id];
                result.push((
                    cowork_workflow_contracts::WorkflowRunApproval {
                        approval: approval.clone(),
                        node_id: node.plan.id.clone(),
                        role_id: node.plan.role_id.clone(),
                    },
                    deadline.min(lease_until),
                ));
            }
        }
    }
    Ok(result)
}
impl Store {
    /// This method accepts only a saved proposal identity, never caller arguments.
    /// Confirmation and artifact creation share one fenced transaction. A replay
    /// returns the original receipt, never another side effect or step charge.
    pub async fn execute_workflow_artifact_tool(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        turn_id: &str,
        raw_id: &str,
    ) -> Result<WorkflowToolExecution> {
        let parent = self.stored_run(run).await?;
        if parent.record.spec.project_id != project {
            return Err(Error::NotFound);
        }
        let before = self.stored_workflow(project, run).await?;
        let (turn, tool) = proposal(&before, turn_id, raw_id)?;
        if let Some(receipt) = turn.tool_results.iter().find(|r| r.id == tool.id) {
            return Ok(WorkflowToolExecution::Completed(receipt.clone()));
        }
        pending(&before, turn, raw_id)?;
        parent.assert_lease(lease, now())?;
        current_authority(&parent)?;
        reject_stopped(&before)?;
        tool_deadline(&parent, &before, turn)?;
        if !matches!(
            tool.name.as_str(),
            "delegate_agents"
                | "create_artifact"
                | "list_artifacts"
                | "read_artifact"
                | "request_input"
        ) {
            return Err(Error::Provider("workflow_tools_unavailable".into()));
        }
        let record = scoped_record(&parent.record, &before, turn)?;
        let decision = evaluate(&record, &tool, &[], &[], &parent.checkpoint.approvals);
        if decision == Decision::ApprovalRequired {
            if self
                .workflow(project, &before.binding.workflow_id)
                .await?
                .revoked
            {
                return Err(Error::Forbidden);
            }
            if let Some(a) = parent
                .checkpoint
                .approvals
                .iter()
                .find(|a| exact_approval(a, &record, &tool))
            {
                return Ok(WorkflowToolExecution::AwaitingApproval(a.clone()));
            }
            let a = Approval {
                id: id(),
                run_id: run.into(),
                tool: tool.clone(),
                target: record.spec.executor.clone(),
                policy_revision: record.spec.policy_revision,
                fingerprint: approval_fingerprint(&record, &tool),
                decision: None,
            };
            self.mutate(run, Some(lease), |r| {
                current_authority(r)?;
                if r.checkpoint.approvals.iter().any(|old| old.tool.id == tool.id) { return Err(Error::Conflict); }
                r.checkpoint.approvals.push(a.clone());
                let mut c = Change::event("approval_requested", json!({"approval":a,"node_id":turn.claim.node_id,"model_turn_id":turn_id,"raw_tool_call_id":raw_id}));
                c.workflow_tool_gate = Some(ToolGate::Request { turn: turn_id.into(), raw_id: raw_id.into(), approval: a.id.clone() });
                Ok(c)
            }).await?;
            return Ok(WorkflowToolExecution::AwaitingApproval(a));
        }
        if decision == Decision::Allow && tool.name == "request_input" {
            return self
                .open_workflow_input(project, run, lease, turn_id, raw_id)
                .await
                .map(WorkflowToolExecution::AwaitingInput);
        }
        let (result, artifact, succeeded) = if decision == Decision::Deny {
            (json!({"error":"policy_denied"}), None, false)
        } else {
            match if tool.name == "delegate_agents" {
                (|| {
                    if turn.tool_results.len() + 1
                        != turn.response.as_ref().ok_or(Error::Forbidden)?.tools.len()
                    {
                        return Err(Error::Validation(
                            "Delegation must be the last tool call in a turn".into(),
                        ));
                    }
                    let requests = serde_json::from_value::<Vec<WorkflowDelegation>>(
                        tool.arguments
                            .get("requests")
                            .cloned()
                            .ok_or(Error::Forbidden)?,
                    )
                    .map_err(|_| Error::Validation("Invalid delegation requests".into()))?;
                    if !record.spec.allowed_tools.contains(&tool.name) || requests.is_empty() {
                        return Err(Error::Forbidden);
                    }
                    let mut checked = before.progress.clone();
                    checked
                        .delegate(
                            &before.plan,
                            checked.revision(),
                            execution_claim(&before, turn)?,
                            &tool.id,
                            requests.clone(),
                            seconds(),
                        )
                        .map_err(|_| {
                            Error::Validation(
                                "Delegation exceeds configured roles, rights or limits".into(),
                            )
                        })?;
                    Ok((
                        json!({"delegation_id":tool.id,"requests":requests,"waiting_for_children":true}),
                        None,
                    ))
                })()
            } else {
                self.artifact_value(&record, &tool).await
            } {
                Ok((result, artifact)) => (result, artifact, true),
                Err(Error::Validation(_)) => (json!({"error":"validation"}), None, false),
                Err(Error::NotFound) => (json!({"error":"not_found"}), None, false),
                Err(e) => return Err(e),
            }
        };
        let raw_tool =
            turn.response.as_ref().ok_or(Error::Forbidden)?.tools[turn.tool_results.len()].clone();
        let receipt = WorkflowToolReceipt {
            id: tool.id.clone(),
            tool: raw_tool,
            result_sha256: canonical_hash(&result),
            result,
            succeeded,
        };
        let mut next = before.clone();
        if tool.name == "delegate_agents" && succeeded {
            let requests = serde_json::from_value::<Vec<WorkflowDelegation>>(
                tool.arguments["requests"].clone(),
            )
            .map_err(|_| Error::Forbidden)?;
            next.progress.record_delegation_result(
                &next.plan,
                before.progress.revision(),
                execution_claim(&before, turn)?,
                &receipt.id,
                &canonical_hash(&tool.arguments),
                &canonical_hash(&receipt),
                requests,
                seconds(),
            )?;
        } else {
            next.progress.record_tool_result(
                &next.plan,
                before.progress.revision(),
                execution_claim(&before, turn)?,
                &receipt.id,
                &tool.name,
                &canonical_hash(&tool.arguments),
                &canonical_hash(&receipt),
                succeeded,
                seconds(),
            )?;
        }
        next.agents
            .get_mut(&turn.claim.node_id)
            .ok_or(Error::Forbidden)?
            .turns
            .last_mut()
            .ok_or(Error::Forbidden)?
            .tool_results
            .push(receipt.clone());
        next.format_version = next.format_version.max(2);
        if json_string(&next)?.len() > 32 * 1024 * 1024 {
            return Err(Error::BudgetExceeded);
        }
        self.mutate(run, Some(lease), |r| {
            current_authority(r)?;
            let mut c = Change::event("workflow_tool_completed", json!({"node_id":turn.claim.node_id,"model_turn_id":turn_id,"receipt":receipt}));
            c.workflow_write = Some(WorkflowWrite { new_action: true, expected_revision: before.progress.revision(), expected_sha256: canonical_hash(&before.progress), next: next.clone() });
            c.workflow_tool_gate = Some(ToolGate::Effect { turn: turn_id.into(), raw_id: raw_id.into(), succeeded });
            if let Some(a) = &artifact {
                c.artifacts.push(a.clone());
                c.events.push(("artifact_created".into(), json!({"id":a.id,"name":a.name,"sha256":a.sha256,"node_id":turn.claim.node_id,"tool_call_id":tool.id})));
            }
            Ok(c)
        }).await?;
        Ok(WorkflowToolExecution::Completed(receipt))
    }
    async fn artifact_value(
        &self,
        run: &RunRecord,
        tool: &ToolCall,
    ) -> Result<(Value, Option<Artifact>)> {
        match tool.name.as_str() {
            "create_artifact" => {
                let name = tool.arguments["name"]
                    .as_str()
                    .ok_or_else(|| Error::Validation("Artifact name missing".into()))?;
                let content = tool.arguments["content"]
                    .as_str()
                    .ok_or_else(|| Error::Validation("Artifact content missing".into()))?;
                valid_text(name, 200)?;
                valid_text(content, 1_000_000)?;
                let a = Artifact {
                    id: id(),
                    run_id: run.id.clone(),
                    order_id: run.spec.order_id.clone(),
                    name: name.into(),
                    media_type: "text/plain; charset=utf-8".into(),
                    size: content.len() as u64,
                    sha256: hash(content.as_bytes()),
                    content: content.into(),
                    created_at: now(),
                };
                Ok((json!({"artifact_id":a.id,"sha256":a.sha256}), Some(a)))
            }
            "list_artifacts" | "read_artifact" => {
                // The explicit read capability covers immutable results of this
                // authorized order, including earlier runs; never another order.
                let all = self.artifacts(&run.spec.order_id).await?;
                if all.len() > 512 {
                    return Err(Error::BudgetExceeded);
                }
                for a in &all {
                    history::verify_artifact(a)?;
                }
                if tool.name == "list_artifacts" {
                    Ok((json!(all.iter().map(|a| json!({"id":a.id,"name":a.name,"sha256":a.sha256,"size":a.size})).collect::<Vec<_>>()), None))
                } else {
                    let aid = tool.arguments["id"]
                        .as_str()
                        .ok_or_else(|| Error::Validation("Artifact id missing".into()))?;
                    let a = all.iter().find(|a| a.id == aid).ok_or(Error::NotFound)?;
                    Ok((json!({"content":a.content,"sha256":a.sha256}), None))
                }
            }
            _ => Err(Error::Forbidden),
        }
    }
    pub(crate) async fn resolve_workflow_tool_approval(
        &self,
        run: &str,
        approval: &str,
        input: ResolveApproval,
    ) -> Result<RunRecord> {
        let parent = self.stored_run(run).await?;
        let existing = parent
            .checkpoint
            .approvals
            .iter()
            .find(|a| a.id == approval)
            .ok_or(Error::NotFound)?;
        if existing.fingerprint != input.fingerprint {
            return Err(Error::Conflict);
        }
        if let Some(decision) = existing.decision {
            return if decision == input.allow {
                Ok(parent.record)
            } else {
                Err(Error::Conflict)
            };
        }
        let lease = parent
            .lease
            .as_ref()
            .ok_or(Error::StaleLease)?
            .token
            .clone();
        let r = self
            .mutate(run, Some(&lease), |r| {
                let a = r
                    .checkpoint
                    .approvals
                    .iter()
                    .find(|a| a.id == approval)
                    .ok_or(Error::NotFound)?;
                if a.fingerprint != input.fingerprint {
                    return Err(Error::Conflict);
                }
                if let Some(decision) = a.decision {
                    return if decision == input.allow {
                        Ok(Change::default())
                    } else {
                        Err(Error::Conflict)
                    };
                }
                current_authority(r)?;
                let a = r
                    .checkpoint
                    .approvals
                    .iter_mut()
                    .find(|a| a.id == approval)
                    .ok_or(Error::NotFound)?;
                a.decision = Some(input.allow);
                let mut c = Change::event(
                    "approval_resolved",
                    json!({"approval_id":approval,"allow":input.allow}),
                );
                c.workflow_tool_gate = Some(ToolGate::Resolve {
                    approval: approval.into(),
                });
                Ok(c)
            })
            .await?;
        Ok(r.record)
    }
}
pub(super) async fn verify_gate(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    parent: &StoredRun,
    workflow: &StoredWorkflow,
    gate: &ToolGate,
) -> Result<i64> {
    current_authority(parent)?;
    registry::lock_head(tx, &workflow.project_id, &workflow.binding.workflow_id).await?;
    current_authority(parent)?;
    if registry::read_entry(tx, &workflow.project_id, &workflow.binding.workflow_id)
        .await?
        .revoked
    {
        return Err(Error::Forbidden);
    }
    let (turn, tool, approval_id) = match gate {
        ToolGate::Request {
            turn,
            raw_id,
            approval,
        } => {
            let (t, c) = proposal(workflow, turn, raw_id)?;
            pending(workflow, t, raw_id)?;
            (t, c, Some(approval))
        }
        ToolGate::InputOpen { turn, raw_id } => {
            let (t, c) = proposal(workflow, turn, raw_id)?;
            pending(workflow, t, raw_id)?;
            if c.name != "request_input" {
                return Err(Error::Forbidden);
            }
            (t, c, None)
        }
        ToolGate::InputAnswer {
            request,
            fingerprint,
        } => {
            let q = workflow.inputs.get(request).ok_or(Error::NotFound)?;
            if &q.fingerprint != fingerprint || q.receipt.is_some() {
                return Err(Error::Conflict);
            }
            let (t, c) = proposal(workflow, &q.turn_id, &q.raw_tool_call_id)?;
            pending(workflow, t, &q.raw_tool_call_id)?;
            if c.name != "request_input" {
                return Err(Error::Forbidden);
            }
            (t, c, None)
        }
        ToolGate::Effect { turn, raw_id, .. }
        | ToolGate::ExternalStart { turn, raw_id }
        | ToolGate::ExternalFinish { turn, raw_id }
        | ToolGate::ExternalCheck { turn, raw_id } => {
            let (t, c) = proposal(workflow, turn, raw_id)?;
            pending(workflow, t, raw_id)?;
            (t, c, None)
        }
        ToolGate::Resolve { approval } => {
            let a = parent
                .checkpoint
                .approvals
                .iter()
                .find(|a| &a.id == approval)
                .ok_or(Error::NotFound)?;
            let mut found = None;
            for t in workflow.agents.values().flat_map(|a| &a.turns) {
                if let Some(response) = &t.response {
                    for call in &response.tools {
                        if broker_id(&workflow.run_id, t, &call.id) == a.tool.id {
                            pending(workflow, t, &call.id)?;
                            found = Some(proposal(workflow, &t.id, &call.id)?);
                        }
                    }
                }
            }
            let (t, c) = found.ok_or(Error::Conflict)?;
            (t, c, Some(approval))
        }
    };
    reject_stopped(workflow)?;
    let deadline = tool_deadline(parent, workflow, turn)?;
    let scoped = scoped_record(&parent.record, workflow, turn)?;
    let mut approvals = parent.checkpoint.approvals.clone();
    if let Some(id) = approval_id {
        let a = approvals
            .iter_mut()
            .find(|a| &a.id == id)
            .ok_or(Error::NotFound)?;
        if !exact_approval(a, &scoped, &tool) {
            return Err(Error::Conflict);
        }
        if matches!(gate, ToolGate::Request { .. }) && a.decision.is_some() {
            return Err(Error::Conflict);
        }
        a.decision = None;
    }
    let decision = evaluate(&scoped, &tool, &[], &[], &approvals);
    if matches!(
        gate,
        ToolGate::ExternalStart { .. }
            | ToolGate::ExternalFinish { .. }
            | ToolGate::ExternalCheck { .. }
    ) {
        if !(matches!(
            tool.name.as_str(),
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
        ) || tool.name.starts_with("mcp_"))
            || decision != Decision::Allow
            || ((matches!(tool.name.as_str(), "browser" | "office" | "github")
                || (tool.name == "execute_program" && scoped.spec.environment.is_none()))
                && !approvals
                    .iter()
                    .any(|a| exact_approval(a, &scoped, &tool) && a.decision == Some(true)))
        {
            return Err(Error::Forbidden);
        }
        if matches!(
            gate,
            ToolGate::ExternalFinish { .. } | ToolGate::ExternalCheck { .. }
        ) {
            let attempt = workflow
                .progress
                .attempts()
                .get(&tool.id)
                .ok_or(Error::Forbidden)?;
            if attempt.operation
                != (WorkflowOperation::Tool {
                    tool_id: tool.name.clone(),
                    arguments_sha256: canonical_hash(&tool.arguments),
                })
                || attempt.claim != *execution_claim(workflow, turn)?
                || attempt.outcome.is_some()
            {
                return Err(Error::UncertainEffect);
            }
        }
    }
    match gate {
        ToolGate::Request { .. } | ToolGate::Resolve { .. }
            if decision != Decision::ApprovalRequired =>
        {
            Err(Error::Conflict)
        }
        ToolGate::InputOpen { .. } | ToolGate::InputAnswer { .. }
            if decision != Decision::Allow =>
        {
            Err(Error::Conflict)
        }
        ToolGate::Effect {
            succeeded: true, ..
        } if decision != Decision::Allow => Err(Error::Conflict),
        ToolGate::Effect { .. } if decision == Decision::ApprovalRequired => Err(Error::Conflict),
        _ => Ok(deadline),
    }
}
pub(super) fn validate_turn(
    workflow: &StoredWorkflow,
    run: &RunRecord,
    turn: &WorkflowModelTurn,
) -> Result<()> {
    if turn.tool_results.is_empty() {
        return Ok(());
    }
    let response = turn.response.as_ref().ok_or(Error::Forbidden)?;
    if response.rejection_code.is_some()
        || turn.tool_results.len() > response.tools.len()
        || !matches!(workflow.format_version, 2..=4)
    {
        return Err(Error::Forbidden);
    }
    let mut previous_step = workflow
        .progress
        .attempts()
        .get(&turn.id)
        .ok_or(Error::Forbidden)?
        .step;
    for (i, r) in turn.tool_results.iter().enumerate() {
        if response.tools[i] != r.tool
            || r.id != broker_id(&run.id, turn, &r.tool.id)
            || r.result_sha256 != canonical_hash(&r.result)
            || json_string(&r.result)?.len() > 2 * 1024 * 1024
        {
            return Err(Error::Forbidden);
        }
        let a = workflow
            .progress
            .attempts()
            .get(&r.id)
            .ok_or(Error::Forbidden)?;
        if a.step <= previous_step
            || !workflow
                .progress
                .receipt_claim_valid(&turn.id, &turn.claim, &a.claim, i)
            || a.operation
                != (WorkflowOperation::Tool {
                    tool_id: r.tool.name.clone(),
                    arguments_sha256: canonical_hash(&r.tool.arguments),
                })
            || a.response_sha256.as_ref() != Some(&canonical_hash(r))
            || a.outcome
                != Some(if r.succeeded {
                    WorkflowOutcome::Succeeded
                } else {
                    WorkflowOutcome::Failed
                })
        {
            return Err(Error::Forbidden);
        }
        previous_step = a.step;
    }
    Ok(())
}

pub(super) fn validate_receipts(workflow: &StoredWorkflow) -> Result<()> {
    let receipts: std::collections::BTreeSet<_> = workflow
        .agents
        .values()
        .flat_map(|a| &a.turns)
        .flat_map(|t| &t.tool_results)
        .map(|r| r.id.as_str())
        .collect();
    for a in workflow.progress.attempts().values() {
        if matches!(a.operation, WorkflowOperation::Tool { .. })
            && a.response_sha256.is_some()
            && !receipts.contains(a.id.as_str())
        {
            return Err(Error::Forbidden);
        }
    }
    Ok(())
}
