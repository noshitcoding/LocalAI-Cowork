//! Whole-graph, server-derived human continuation evidence. No provider IO.
use super::*;
use cowork_workflow_contracts::WorkflowInputResumePreview;

pub(crate) fn cursor_hash(cursors: &[WorkflowHumanCursor]) -> String {
    canonical_hash(&json!({"format":"cowork-human-cursors-v1","cursors":cursors}))
}
pub(crate) async fn block_reason(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    parent: &StoredRun,
    workflow: &StoredWorkflow,
    at: i64,
    highwater: Option<i64>,
) -> Result<Option<String>> {
    if parent.record.state == RunState::Canceled {
        crate::workflow_input_drafts::known_cancellation(tx, &parent.record).await?;
        let rows = sqlx::query_scalar::<_, String>(
            "SELECT payload FROM events WHERE run_id=$1 AND sequence<=$2 ORDER BY sequence",
        )
        .bind(&parent.record.id)
        .bind(parent.record.last_sequence)
        .fetch_all(&mut **tx)
        .await
        .map_err(storage_error)?;
        let mut canceled = 0;
        for payload in rows {
            let event: RunEvent = parse(&payload)?;
            if event.kind == "run_canceled" {
                if event.payload != json!({"state":"canceled"}) {
                    return Err(Error::Forbidden);
                }
                canceled += 1;
            }
        }
        if canceled != 1 {
            return Err(Error::Forbidden);
        }
        return Ok(Some("canceled".into()));
    }
    if registry::read_entry(tx, &workflow.project_id, &workflow.binding.workflow_id)
        .await?
        .revoked
    {
        return Ok(Some("workflow_revoked".into()));
    }
    if crate::workflow_input_drafts::known_cancellation(tx, &parent.record).await? {
        return Ok(Some("canceled".into()));
    }
    if highwater.is_none() {
        return Ok(Some("workflow_clock_unverified".into()));
    }
    if highwater.is_some_and(|h| at < h) {
        return Ok(Some("workflow_clock_rollback".into()));
    }
    let c = workflow
        .progress
        .human_continuations()
        .last()
        .ok_or(Error::Forbidden)?;
    if time_budget_exhausted(&parent.record, at) || c.cursors.iter().any(|c| at >= c.expires_at) {
        return Ok(Some("budget_exceeded".into()));
    }
    Ok(None)
}
impl Store {
    pub(crate) async fn close_unactivated_human_resume(&self, run: &str) -> Result<bool> {
        let parent = self.stored_run(run).await?;
        let workflow = self
            .stored_workflow(&parent.record.spec.project_id, run)
            .await?;
        let Some(c) = workflow.progress.human_continuations().last() else {
            return Ok(false);
        };
        if c.blocked_reason.is_some() {
            return Ok(true);
        }
        if !c.execution_claims.is_empty()
            || !matches!(
                parent.record.state,
                RunState::Queued
                    | RunState::WaitingForExecutor
                    | RunState::Canceled
                    | RunState::Expired
            )
        {
            return Ok(false);
        }
        // Conservative intent never extends Cancellation or Recovery authority.
        let result = self
            .mutate_with_clock_intent(
                run,
                None,
                crate::workflow_clocks::WorkflowClockIntent::HumanResumeNonActivation,
                |r| {
                    if !matches!(
                        r.record.state,
                        RunState::Queued
                            | RunState::WaitingForExecutor
                            | RunState::Canceled
                            | RunState::Expired
                    ) || r.lease.is_some()
                    {
                        return Err(Error::Conflict);
                    }
                    Ok(Change {
                        workflow_resume_block: Some("derive".into()),
                        ..Default::default()
                    })
                },
            )
            .await;
        match result {
            Ok(_) => Ok(true),
            Err(Error::Conflict | Error::Forbidden) => {
                let current = self
                    .stored_workflow(&parent.record.spec.project_id, run)
                    .await?;
                if current
                    .progress
                    .human_continuations()
                    .last()
                    .is_some_and(|after| {
                        after.id == c.id
                            && after.generation == c.generation
                            && after.blocked_reason.is_some()
                    })
                {
                    Ok(true)
                } else {
                    Err(Error::Conflict)
                }
            }
            Err(e) => Err(e),
        }
    }
}
fn scope_hash(run: &RunRecord, workflow: &StoredWorkflow) -> String {
    canonical_hash(&json!({"spec":run.spec,"binding":workflow.binding,"plan":workflow.plan.sha256}))
}
pub(crate) fn derive(
    parent: &StoredRun,
    workflow: &StoredWorkflow,
    at: i64,
) -> Result<Vec<WorkflowHumanCursor>> {
    if parent.lease.is_some()
        || parent.record.started_at.is_none()
        || !matches!(
            parent.record.state,
            RunState::Interrupted | RunState::Queued | RunState::WaitingForExecutor
        )
        || parent.checkpoint.in_flight.is_some()
        || parent.checkpoint.pending_file_change.is_some()
        || parent.checkpoint.pending_file_operation.is_some()
        || parent.checkpoint.completion_pending
        || !parent.checkpoint.pending_tools.is_empty()
        || parent.checkpoint.input_request.is_some()
        || parent
            .checkpoint
            .approvals
            .iter()
            .any(|a| a.decision.is_none())
    {
        return Err(Error::UncertainEffect);
    }
    tools::reject_stopped(workflow)?;
    if matches!(
        workflow.plan.definition.mode,
        WorkflowMode::Hierarchical {
            manager_step_id: None,
            ..
        }
    ) {
        return Err(Error::Forbidden);
    }
    // An approval for an unexecuted ordinary call cannot become a grant for a
    // new execution generation. Existing opened questions keep their identity.
    if parent.checkpoint.approvals.iter().any(|a| {
        !workflow.inputs.contains_key(&a.tool.id)
            && !workflow
                .agents
                .values()
                .flat_map(|a| &a.turns)
                .flat_map(|t| &t.tool_results)
                .any(|r| r.id == a.tool.id)
    }) {
        return Err(Error::UncertainEffect);
    }
    if time_budget_exhausted(&parent.record, at) {
        return Err(Error::BudgetExceeded);
    }
    for a in workflow.progress.attempts().values() {
        match &a.operation {
            WorkflowOperation::Model { .. } => {
                let turn = agents::find_turn(workflow, &a.id).ok_or(Error::Forbidden)?;
                if turn
                    .response
                    .as_ref()
                    .is_none_or(|r| r.rejection_code.is_some())
                {
                    return Err(Error::UncertainEffect);
                }
            }
            _ if a.outcome == Some(WorkflowOutcome::Uncertain)
                || (a.outcome.is_none()
                    && !workflow.progress.input_checkpoints().contains_key(&a.id)) =>
            {
                return Err(Error::UncertainEffect);
            }
            _ => {}
        }
    }
    let mut cursors = vec![];
    for (node_id, node) in workflow.progress.nodes() {
        if node.state != WorkflowNodeState::Interrupted {
            continue;
        }
        let turn = workflow
            .agents
            .get(node_id)
            .and_then(|a| a.turns.last())
            .ok_or(Error::UncertainEffect)?;
        let response = turn.response.as_ref().ok_or(Error::UncertainEffect)?;
        if response.rejection_code.is_some()
            || response
                .tools
                .iter()
                .skip(turn.tool_results.len())
                .any(|t| {
                    !matches!(
                        t.name.as_str(),
                        "request_input" | "create_artifact" | "list_artifacts" | "read_artifact"
                    )
                })
        {
            return Err(Error::UncertainEffect);
        }
        let q = response
            .tools
            .iter()
            .enumerate()
            .filter_map(|(ordinal, t)| {
                workflow
                    .inputs
                    .get(&tools::broker_id(&workflow.run_id, turn, &t.id))
                    .map(|q| (ordinal, q))
            })
            .rfind(|(ordinal, q)| q.receipt.is_some() || *ordinal == turn.tool_results.len())
            .ok_or(Error::UncertainEffect)?
            .1;
        let input = workflow
            .progress
            .input_checkpoints()
            .get(&q.request_id)
            .ok_or(Error::Forbidden)?;
        if input.closed_reason.is_some()
            || q.turn_id != turn.id
            || (q.receipt.is_none()
                && response
                    .tools
                    .get(turn.tool_results.len())
                    .is_none_or(|t| t.id != q.raw_tool_call_id))
        {
            return Err(Error::UncertainEffect);
        }
        if at >= q.expires_at {
            return Err(Error::BudgetExceeded);
        }
        cursors.push(WorkflowHumanCursor {
            node_id: node_id.clone(),
            model_claim: turn.claim.clone(),
            input_claim: input.claim.clone(),
            model_turn_id: turn.id.clone(),
            request_id: q.request_id.clone(),
            fingerprint: q.fingerprint.clone(),
            context_sha256: canonical_hash(&turn.context),
            response_sha256: canonical_hash(response),
            prefix_sha256: canonical_hash(&turn.tool_results),
            next_call_ordinal: turn.tool_results.len() as u32,
            expires_at: q.expires_at,
            scope_sha256: scope_hash(&parent.record, workflow),
        });
    }
    if cursors.is_empty() {
        return Err(Error::UncertainEffect);
    }
    Ok(cursors)
}
pub(crate) async fn verify_live(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    parent: &StoredRun,
    workflow: &StoredWorkflow,
    at: i64,
) -> Result<Vec<WorkflowHumanCursor>> {
    if registry::read_entry(tx, &workflow.project_id, &workflow.binding.workflow_id)
        .await?
        .revoked
    {
        return Err(Error::WorkflowRevoked);
    }
    if crate::workflow_input_drafts::known_cancellation(tx, &parent.record).await? {
        return Err(Error::Forbidden);
    }
    let cursors = derive(parent, workflow, at)?;
    for q in workflow.inputs.values() {
        verify_question_events(tx, &parent.record, workflow, &q.request_id).await?;
    }
    verify_history(tx, parent, workflow).await?;
    Ok(cursors)
}
async fn verify_question_events(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    run: &RunRecord,
    workflow: &StoredWorkflow,
    request: &str,
) -> Result<()> {
    let q = workflow.inputs.get(request).ok_or(Error::Forbidden)?;
    let rows = sqlx::query_scalar::<_, String>(
        "SELECT payload FROM events WHERE run_id=$1 AND sequence<=$2 ORDER BY sequence",
    )
    .bind(&run.id)
    .bind(run.last_sequence)
    .fetch_all(&mut **tx)
    .await
    .map_err(storage_error)?;
    let mut requested = 0;
    let mut answered = 0;
    for payload in rows {
        let e: RunEvent = parse(&payload)?;
        if e.payload["request_id"].as_str() != Some(request) {
            continue;
        }
        if e.kind == "workflow_input_requested" {
            if e.payload["fingerprint"] != q.fingerprint
                || e.payload["node_id"] != q.node_id
                || e.payload["model_turn_id"] != q.turn_id
                || !matches!(e.schema_version, 7 | 8)
            {
                return Err(Error::Forbidden);
            }
            requested += 1;
        }
        if e.kind == "workflow_input_answered" {
            let receipt = q.receipt.as_ref().ok_or(Error::Forbidden)?;
            if !matches!(e.schema_version, 7 | 8)
                || e.sequence != receipt.confirmed_sequence
                || e.payload["receipt_id"] != receipt.receipt_id
                || e.payload["answer_sha256"] != receipt.answer_sha256
                || e.payload["node_id"] != q.node_id
                || e.payload["model_turn_id"] != q.turn_id
            {
                return Err(Error::Forbidden);
            }
            answered += 1;
        }
    }
    if requested != 1 || answered != usize::from(q.receipt.is_some()) {
        return Err(Error::Forbidden);
    }
    Ok(())
}
pub(crate) async fn verify_history(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    parent: &StoredRun,
    workflow: &StoredWorkflow,
) -> Result<()> {
    if !workflow.progress.human_continuations().is_empty() {
        // Historical envelope integrity is required at direct execution gates
        // too. The boolean is evidence only; existing cancellation gates retain
        // responsibility for deciding which transition is permitted.
        crate::workflow_input_drafts::known_cancellation(tx, &parent.record).await?;
    }
    for c in workflow.progress.human_continuations() {
        crate::workflow_operations::verify_resume_accepted(tx, &parent.record, workflow, c).await?;
        let events = sqlx::query(
            "SELECT id,sequence,payload FROM events WHERE run_id=$1 AND sequence<=$2 ORDER BY sequence",
        )
        .bind(&parent.record.id)
        .bind(parent.record.last_sequence)
        .fetch_all(&mut **tx)
        .await
        .map_err(storage_error)?;
        let mut activations = 0;
        let mut blocks = 0;
        for row in events {
            let event: RunEvent =
                parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
            if event.payload["resume_id"].as_str() != Some(&c.id) {
                continue;
            }
            if event.id != row.try_get::<String, _>("id").map_err(storage_error)?
                || event.run_id != parent.record.id
                || event.sequence != row.try_get::<i64, _>("sequence").map_err(storage_error)?
                || event.created_at < parent.record.created_at
                || event.created_at > parent.record.updated_at
            {
                return Err(Error::Forbidden);
            }
            if event.kind == "workflow_input_resume_activated" {
                if event.payload
                    != json!({"resume_id":c.id,"generation":c.generation,"lease_sha256":c.lease_sha256,"claims_sha256":canonical_hash(&c.execution_claims)})
                    || event.schema_version != 8
                {
                    return Err(Error::Forbidden);
                }
                activations += 1;
            }
            if event.kind == "workflow_input_resume_blocked" {
                if event.payload
                    != json!({"resume_id":c.id,"generation":c.generation,"reason":c.blocked_reason})
                    || event.schema_version != 8
                {
                    return Err(Error::Forbidden);
                }
                blocks += 1;
            }
        }
        if activations != usize::from(!c.execution_claims.is_empty())
            || blocks != usize::from(c.blocked_reason.is_some())
        {
            return Err(Error::Forbidden);
        }
        if c.execution_claims
            .iter()
            .any(|claim| agents::active(workflow, claim).is_ok())
            && parent.lease.as_ref().is_some_and(|lease| {
                c.lease_sha256.as_deref() != Some(hash(lease.token.as_bytes()).as_str())
            })
        {
            return Err(Error::StaleLease);
        }
    }
    Ok(())
}
pub(crate) async fn preview(
    store: &Store,
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    parent: &StoredRun,
    workflow: &StoredWorkflow,
) -> Result<WorkflowInputResumePreview> {
    let result = match store.workflow_clock.execution_now(tx, &parent.record).await {
        Ok(at) if parent.record.state == RunState::Interrupted => {
            verify_live(tx, parent, workflow, at).await
        }
        Ok(_) => Err(Error::Conflict),
        Err(e) => Err(e),
    };
    Ok(WorkflowInputResumePreview {
        available: result.is_ok(),
        blocked_reason: result.as_ref().err().map(|e| e.code().to_owned()),
        base_run_revision: parent.record.revision,
        base_workflow_revision: workflow.progress.revision(),
        expected_checkpoint_sha256: canonical_hash(&workflow.progress),
        expected_cursor_sha256: result.as_ref().ok().map(|c| cursor_hash(c)),
    })
}
pub(super) fn validate(workflow: &StoredWorkflow, run: &RunRecord) -> Result<()> {
    if !workflow.progress.human_continuations().is_empty()
        && !matches!(workflow.format_version, 4 | 5)
    {
        return Err(Error::Forbidden);
    }
    for c in workflow.progress.human_continuations() {
        for cursor in &c.cursors {
            let turn =
                agents::find_turn(workflow, &cursor.model_turn_id).ok_or(Error::Forbidden)?;
            let q = workflow
                .inputs
                .get(&cursor.request_id)
                .ok_or(Error::Forbidden)?;
            let ordinal = cursor.next_call_ordinal as usize;
            if turn.claim != cursor.model_claim
                || cursor.scope_sha256 != scope_hash(run, workflow)
                || cursor.context_sha256 != canonical_hash(&turn.context)
                || turn.response_sha256.as_ref() != Some(&cursor.response_sha256)
                || ordinal > turn.tool_results.len()
                || canonical_hash(&&turn.tool_results[..ordinal]) != cursor.prefix_sha256
                || q.fingerprint != cursor.fingerprint
                || q.expires_at != cursor.expires_at
                || q.turn_id != cursor.model_turn_id
                || workflow
                    .progress
                    .input_checkpoints()
                    .get(&q.request_id)
                    .is_none_or(|q| q.claim != cursor.input_claim)
            {
                return Err(Error::Forbidden);
            }
        }
    }
    Ok(())
}
