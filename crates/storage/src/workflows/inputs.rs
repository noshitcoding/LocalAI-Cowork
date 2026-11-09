//! Human waits use the same durable proposal and policy gates as artifact tools.
//! Answer text exists only in its agent tool receipt, never parent messages/events.
use super::*;
use cowork_policy::{Decision, evaluate};
use cowork_workflow_contracts::{
    SubmitWorkflowInput, WorkflowInputReceipt, WorkflowInputState, WorkflowInputView,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredWorkflowInput {
    pub request_id: String,
    pub fingerprint: String,
    pub node_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<cowork_domain::workflows::WorkflowReviewRequest>,
    pub turn_id: String,
    pub raw_tool_call_id: String,
    pub question: String,
    pub expires_at: i64,
    pub receipt: Option<WorkflowInputReceipt>,
    pub answer_key: Option<String>,
}
pub(crate) enum PreparedWorkflowInputAnswer {
    Existing(WorkflowInputReceipt),
    Write(Box<Change>, WorkflowInputReceipt),
}
fn question(tool: &ToolCall) -> Result<String> {
    let args = tool
        .arguments
        .as_object()
        .ok_or_else(|| Error::Validation("Input arguments must be an object".into()))?;
    if args.len() != 1 || !args.contains_key("question") {
        return Err(Error::Validation("Input accepts exactly a question".into()));
    }
    let text = args["question"]
        .as_str()
        .ok_or_else(|| Error::Validation("Input question must be text".into()))?;
    valid_text(text, 4000)?;
    Ok(text.to_owned())
}
fn fingerprint(
    run: &RunRecord,
    workflow: &StoredWorkflow,
    turn: &WorkflowModelTurn,
    tool: &ToolCall,
) -> Result<String> {
    let scoped = tools::scoped_record(run, workflow, turn)?;
    let node = workflow
        .progress
        .nodes()
        .get(&turn.claim.node_id)
        .ok_or(Error::Forbidden)?;
    Ok(canonical_hash(&json!({
        "format":"cowork-workflow-input-v1", "run":run.id,"project":run.spec.project_id,
        "workflow":workflow.binding,"node":turn.claim.node_id,"role":node.plan.role_id,
        "turn":turn.id,"raw_call":tool.id,"tool":tool,
        "executor":scoped.spec.executor,"policy_revision":scoped.spec.policy_revision,
        "access_mode":scoped.spec.access_mode,"tools":scoped.spec.allowed_tools,
        "folders":scoped.spec.folders,
    })))
}
fn view(
    parent: &StoredRun,
    workflow: &StoredWorkflow,
    q: &StoredWorkflowInput,
    revoked: bool,
) -> Result<WorkflowInputView> {
    let mut reason = None;
    let state = if q.receipt.is_some() {
        WorkflowInputState::Answered
    } else {
        reason = if let Some(code) = workflow
            .progress
            .input_checkpoints()
            .get(&q.request_id)
            .and_then(|c| c.closed_reason.as_deref())
        {
            Some(code)
        } else if revoked {
            Some("workflow_revoked")
        } else if now() >= q.expires_at {
            Some("budget_exceeded")
        } else if parent.record.state != RunState::Running {
            Some(match parent.record.state {
                RunState::CancelRequested | RunState::Canceled => "canceled",
                RunState::Expired => "budget_exceeded",
                RunState::Interrupted => "interrupted",
                _ => "run_inactive",
            })
        } else if tools::reject_stopped(workflow).is_err() {
            Some("workflow_stopped")
        } else if tools::current_authority(parent).is_err() {
            Some("stale_lease")
        } else {
            let (turn, tool) = tools::proposal(workflow, &q.turn_id, &q.raw_tool_call_id)?;
            let scoped = tools::scoped_record(&parent.record, workflow, turn)?;
            if tools::pending(workflow, turn, &q.raw_tool_call_id).is_err() {
                Some("input_not_pending")
            } else if evaluate(&scoped, &tool, &[], &[], &parent.checkpoint.approvals)
                != Decision::Allow
            {
                Some("policy_denied")
            } else {
                None
            }
        };
        if reason.is_some() {
            WorkflowInputState::Closed
        } else {
            WorkflowInputState::WaitingAnswer
        }
    };
    Ok(WorkflowInputView {
        request_id: q.request_id.clone(),
        fingerprint: q.fingerprint.clone(),
        node_id: q.node_id.clone(),
        role_id: q.role_id.clone(),
        review: q.review.clone(),
        question: q.question.clone(),
        actionable: state == WorkflowInputState::WaitingAnswer,
        state,
        expires_at: q.expires_at,
        receipt: q.receipt.clone(),
        closed_reason: reason.map(str::to_owned),
    })
}
/// Read scope only. Callers check registry/current principal inside their own tx.
/// This never grants a model/tool dispatch or permission to accept an answer.
pub(crate) fn read_binding(
    parent: &StoredRun,
    workflow: &StoredWorkflow,
    request: &str,
    expected: &str,
) -> Result<WorkflowInputView> {
    if parent.record.id != workflow.run_id || parent.record.spec.project_id != workflow.project_id {
        return Err(Error::NotFound);
    }
    let q = workflow.inputs.get(request).ok_or(Error::NotFound)?;
    if q.fingerprint != expected {
        return Err(Error::Conflict);
    }
    view(parent, workflow, q, false)
}
pub(super) fn views(
    parent: &StoredRun,
    workflow: &StoredWorkflow,
    revoked: bool,
) -> Result<Vec<WorkflowInputView>> {
    workflow
        .inputs
        .values()
        .map(|q| view(parent, workflow, q, revoked))
        .collect()
}
/// Pure helper for the intent owner's Store::mutate closure. The SQL intent ack
/// must be attached to this Change and committed with the workflow write/event.
pub(crate) fn answer_change(
    parent: &StoredRun,
    before: &StoredWorkflow,
    input: &SubmitWorkflowInput,
) -> Result<PreparedWorkflowInputAnswer> {
    valid_text(&input.input.answer, 32_000)?;
    valid_text(&input.idempotency_key, 128)?;
    let q = before
        .inputs
        .get(&input.input.request_id)
        .ok_or(Error::NotFound)?;
    if q.fingerprint != input.fingerprint {
        return Err(Error::Conflict);
    }
    if before.inputs.values().any(|other| {
        other.request_id != q.request_id
            && other.answer_key.as_deref() == Some(&input.idempotency_key)
    }) {
        return Err(Error::Conflict);
    }
    super::native_reviews::answer(q.review.as_ref(), &input.input.answer)?;
    let answer_sha256 = hash(input.input.answer.as_bytes());
    if let Some(receipt) = &q.receipt {
        return if receipt.answer_sha256 == answer_sha256
            && q.answer_key.as_deref() == Some(input.idempotency_key.as_str())
        {
            Ok(PreparedWorkflowInputAnswer::Existing(receipt.clone()))
        } else {
            Err(Error::Conflict)
        };
    }
    tools::current_authority(parent)?;
    tools::reject_stopped(before)?;
    let (turn, tool) = tools::proposal(before, &q.turn_id, &q.raw_tool_call_id)?;
    tools::pending(before, turn, &q.raw_tool_call_id)?;
    let deadline = tools::tool_deadline(parent, before, turn)?;
    let scoped = tools::scoped_record(&parent.record, before, turn)?;
    if evaluate(&scoped, &tool, &[], &[], &parent.checkpoint.approvals) != Decision::Allow {
        return Err(Error::Forbidden);
    }
    let result = json!({"answer":input.input.answer});
    let raw =
        turn.response.as_ref().ok_or(Error::Forbidden)?.tools[turn.tool_results.len()].clone();
    let tool_receipt = WorkflowToolReceipt {
        id: q.request_id.clone(),
        tool: raw,
        result_sha256: canonical_hash(&result),
        result,
        succeeded: true,
    };
    let receipt = WorkflowInputReceipt {
        request_id: q.request_id.clone(),
        fingerprint: q.fingerprint.clone(),
        receipt_id: tool_receipt.id.clone(),
        answer_sha256,
        confirmed_sequence: parent.record.last_sequence + 1,
    };
    let mut next = before.clone();
    let execution = tools::execution_claim(before, turn)?;
    let owner = &before
        .progress
        .input_checkpoints()
        .get(&q.request_id)
        .ok_or(Error::Forbidden)?
        .claim;
    let evidence = WorkflowInputAnswerEvidence {
        receipt_sha256: canonical_hash(&tool_receipt),
        confirmed_sequence: receipt.confirmed_sequence,
        answer_key_sha256: hash(input.idempotency_key.as_bytes()),
    };
    if execution == owner {
        next.progress.answer_input(
            &next.plan,
            before.progress.revision(),
            execution,
            &q.request_id,
            &evidence,
            seconds(),
        )?;
    } else {
        next.progress.answer_human(
            &next.plan,
            before.progress.revision(),
            execution,
            owner,
            &q.request_id,
            &evidence,
            seconds(),
        )?;
    }
    next.agents
        .get_mut(&q.node_id)
        .ok_or(Error::Forbidden)?
        .turns
        .last_mut()
        .ok_or(Error::Forbidden)?
        .tool_results
        .push(tool_receipt);
    let stored = next.inputs.get_mut(&q.request_id).ok_or(Error::Forbidden)?;
    stored.receipt = Some(receipt.clone());
    stored.answer_key = Some(input.idempotency_key.clone());
    next.format_version = next.format_version.max(3);
    if json_string(&next)?.len() > 32 * 1024 * 1024 {
        return Err(Error::BudgetExceeded);
    }
    let mut change = Change::event(
        "workflow_input_answered",
        json!({
            "request_id":q.request_id,"node_id":q.node_id,"model_turn_id":q.turn_id,
            "receipt_id":receipt.receipt_id,"answer_sha256":receipt.answer_sha256,
        }),
    );
    change.workflow_write = Some(WorkflowWrite {
        new_action: true,
        expected_revision: before.progress.revision(),
        expected_sha256: canonical_hash(&before.progress),
        next,
    });
    change.workflow_tool_gate = Some(tools::ToolGate::InputAnswer {
        request: q.request_id.clone(),
        fingerprint: q.fingerprint.clone(),
    });
    change.workflow_tool_deadline = Some(deadline);
    Ok(PreparedWorkflowInputAnswer::Write(
        Box::new(change),
        receipt,
    ))
}
impl Store {
    pub(super) async fn open_workflow_input(
        &self,
        project: &str,
        run: &str,
        lease: &str,
        turn_id: &str,
        raw_id: &str,
    ) -> Result<WorkflowInputView> {
        let parent = self.stored_run(run).await?;
        if parent.record.spec.project_id != project {
            return Err(Error::NotFound);
        }
        let before = self.stored_workflow(project, run).await?;
        let (turn, tool) = tools::proposal(&before, turn_id, raw_id)?;
        tools::pending(&before, turn, raw_id)?;
        parent.assert_lease(lease, now())?;
        tools::current_authority(&parent)?;
        tools::reject_stopped(&before)?;
        let expires_at = tools::tool_deadline(&parent, &before, turn)?;
        let text = question(&tool)?;
        let record = tools::scoped_record(&parent.record, &before, turn)?;
        if evaluate(&record, &tool, &[], &[], &parent.checkpoint.approvals) != Decision::Allow {
            return Err(Error::Forbidden);
        }
        if self
            .workflow(project, &before.binding.workflow_id)
            .await?
            .revoked
        {
            return Err(Error::Forbidden);
        }
        if let Some(saved) = before.inputs.get(&tool.id) {
            return view(&parent, &before, saved, false);
        }
        let q = StoredWorkflowInput {
            request_id: tool.id.clone(),
            fingerprint: fingerprint(&parent.record, &before, turn, &tool)?,
            node_id: turn.claim.node_id.clone(),
            role_id: before.progress.nodes()[&turn.claim.node_id]
                .plan
                .role_id
                .clone(),
            review: super::native_reviews::request(&before, turn)?,
            turn_id: turn.id.clone(),
            raw_tool_call_id: raw_id.into(),
            question: text,
            expires_at,
            receipt: None,
            answer_key: None,
        };
        let mut next = before.clone();
        next.progress.open_input(
            &next.plan,
            before.progress.revision(),
            tools::execution_claim(&before, turn)?,
            &q.request_id,
            &canonical_hash(&tool.arguments),
            seconds(),
        )?;
        next.inputs.insert(q.request_id.clone(), q.clone());
        next.format_version = next.format_version.max(3);
        if json_string(&next)?.len() > 32 * 1024 * 1024 {
            return Err(Error::BudgetExceeded);
        }
        let saved = self.mutate(run, Some(lease), |r| {
            tools::current_authority(r)?;
            let mut change = Change::event("workflow_input_requested", json!({
                "request_id":q.request_id,"fingerprint":q.fingerprint,"node_id":q.node_id,"model_turn_id":q.turn_id,
            }));
            change.workflow_write = Some(WorkflowWrite {
                new_action:true, expected_revision:before.progress.revision(),
                expected_sha256:canonical_hash(&before.progress), next:next.clone(),
            });
            change.workflow_tool_gate = Some(tools::ToolGate::InputOpen { turn:turn_id.into(), raw_id:raw_id.into() });
            Ok(change)
        }).await?;
        view(&saved, &next, &q, false)
    }
}
pub(super) fn validate(workflow: &StoredWorkflow, run: &RunRecord) -> Result<()> {
    let checkpoints = workflow.progress.input_checkpoints();
    if workflow.inputs.len() != checkpoints.len()
        || (!workflow.inputs.is_empty() && !matches!(workflow.format_version, 3 | 4))
    {
        return Err(Error::Forbidden);
    }
    for attempt in workflow.progress.attempts().values() {
        if matches!(&attempt.operation, WorkflowOperation::Tool {tool_id,..} if tool_id == "request_input")
            && (attempt.outcome.is_none() || attempt.outcome == Some(WorkflowOutcome::Succeeded))
            && !checkpoints.contains_key(&attempt.id)
        {
            return Err(Error::Forbidden);
        }
    }
    for (key, q) in &workflow.inputs {
        let (turn, tool) = tools::proposal(workflow, &q.turn_id, &q.raw_tool_call_id)?;
        let check = checkpoints.get(key).ok_or(Error::Forbidden)?;
        let node = workflow
            .progress
            .nodes()
            .get(&turn.claim.node_id)
            .ok_or(Error::Forbidden)?;
        if q.request_id != *key
            || tool.id != *key
            || tool.name != "request_input"
            || q.node_id != turn.claim.node_id
            || q.role_id != node.plan.role_id
            || q.review != super::native_reviews::request(workflow, turn)?
            || q.question != question(&tool)?
            || q.expires_at != tools::record_tool_deadline(run, workflow, turn)?
            || q.fingerprint != fingerprint(run, workflow, turn, &tool)?
            || !workflow.progress.receipt_claim_valid(
                &turn.id,
                &turn.claim,
                &check.claim,
                turn.response
                    .as_ref()
                    .ok_or(Error::Forbidden)?
                    .tools
                    .iter()
                    .position(|t| t.id == q.raw_tool_call_id)
                    .ok_or(Error::Forbidden)?,
            )
            || check.arguments_sha256 != canonical_hash(&tool.arguments)
        {
            return Err(Error::Forbidden);
        }
        let attempt = workflow
            .progress
            .attempts()
            .get(key)
            .ok_or(Error::Forbidden)?;
        if let Some(code) = &check.closed_reason {
            if attempt.outcome != Some(WorkflowOutcome::Failed)
                || attempt.response_sha256.is_some()
                || !matches!(&node.state,WorkflowNodeState::Failed {code: node_code} if node_code == code)
            {
                return Err(Error::Forbidden);
            }
        } else if q.receipt.is_none() && attempt.outcome.is_some() {
            return Err(Error::Forbidden);
        }
        let receipt = turn.tool_results.iter().find(|r| r.id == *key);
        match (&q.receipt, receipt) {
            (None, None)
                if q.answer_key.is_none()
                    && check.receipt_sha256.is_none()
                    && check.confirmed_sequence.is_none()
                    && check.answer_key_sha256.is_none() => {}
            (Some(public), Some(tool_receipt)) => {
                let answer = tool_receipt
                    .result
                    .get("answer")
                    .and_then(Value::as_str)
                    .ok_or(Error::Forbidden)?;
                valid_text(answer, 32_000)?;
                super::native_reviews::answer(q.review.as_ref(), answer)?;
                valid_text(q.answer_key.as_deref().ok_or(Error::Forbidden)?, 128)?;
                if check.closed_reason.is_some()
                    || public.request_id != *key
                    || public.fingerprint != q.fingerprint
                    || public.receipt_id != *key
                    || public.confirmed_sequence <= 0
                    || public.confirmed_sequence > run.last_sequence
                    || check.confirmed_sequence != Some(public.confirmed_sequence)
                    || check.answer_key_sha256.as_deref()
                        != Some(
                            hash(q.answer_key.as_ref().ok_or(Error::Forbidden)?.as_bytes())
                                .as_str(),
                        )
                    || tool_receipt.result.as_object().is_none_or(|o| o.len() != 1)
                    || public.answer_sha256 != hash(answer.as_bytes())
                    || !tool_receipt.succeeded
                    || check.receipt_sha256.as_deref()
                        != Some(canonical_hash(tool_receipt).as_str())
                {
                    return Err(Error::Forbidden);
                }
            }
            _ => return Err(Error::Forbidden),
        }
    }
    Ok(())
}
