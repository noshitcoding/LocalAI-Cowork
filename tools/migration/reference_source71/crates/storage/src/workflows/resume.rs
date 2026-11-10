//! Resume uses the immutable model and tool journal, never another dispatch of an old attempt.
use super::*;
pub(super) fn settled_outputs(
    workflow: &StoredWorkflow,
) -> Result<std::collections::BTreeMap<String, String>> {
    Ok(settled_recovery(workflow)?.0)
}
pub(super) fn settled_recovery(
    workflow: &StoredWorkflow,
) -> Result<(
    std::collections::BTreeMap<String, String>,
    Vec<WorkflowSettledCursor>,
)> {
    if workflow.progress.state() != WorkflowState::Interrupted {
        return Err(Error::Conflict);
    }
    if workflow
        .progress
        .attempts()
        .values()
        .any(|a| a.outcome.is_none() || a.outcome == Some(WorkflowOutcome::Uncertain))
    {
        return Err(Error::UncertainEffect);
    }
    let mut completed = std::collections::BTreeMap::new();
    let mut cursors = vec![];
    for (id, node) in workflow
        .progress
        .nodes()
        .iter()
        .filter(|(_, n)| n.state == WorkflowNodeState::Interrupted)
    {
        if let Some(agent) = workflow.agents.get(id) {
            for (ordinal, turn) in agent.turns.iter().enumerate() {
                let reply = turn.response.as_ref().ok_or(Error::UncertainEffect)?;
                if reply.rejection_code.is_some()
                    || (ordinal + 1 < agent.turns.len()
                        && reply.tools.len() != turn.tool_results.len())
                {
                    return Err(Error::UncertainEffect);
                }
                if reply.tools.len() > turn.tool_results.len() {
                    for call in reply.tools.iter().skip(turn.tool_results.len()) {
                        if workflow.progress.attempts().contains_key(&tools::broker_id(
                            &workflow.run_id,
                            turn,
                            &call.id,
                        )) {
                            return Err(Error::UncertainEffect);
                        }
                    }
                    cursors.push(WorkflowSettledCursor {
                        node_id: id.clone(),
                        model_turn_id: turn.id.clone(),
                        model_claim: turn.claim.clone(),
                        context_sha256: canonical_hash(&turn.context),
                        response_sha256: turn.response_sha256.clone().ok_or(Error::Forbidden)?,
                        prefix_sha256: canonical_hash(&turn.tool_results),
                        next_call_ordinal: turn.tool_results.len() as u32,
                        execution_claim: None,
                    });
                }
            }
            if let Some(reply) = agent.turns.last().and_then(|turn| turn.response.as_ref()) {
                if reply.tools.is_empty() {
                    completed.insert(node.plan.id.clone(), reply.content.clone());
                }
            }
        }
    }
    Ok((completed, cursors))
}
pub(super) fn validate(workflow: &StoredWorkflow) -> Result<()> {
    for cursor in workflow.progress.settled_cursors() {
        let turn = agents::find_turn(workflow, &cursor.model_turn_id).ok_or(Error::Forbidden)?;
        let response = turn.response.as_ref().ok_or(Error::Forbidden)?;
        let count = cursor.next_call_ordinal as usize;
        if turn.claim != cursor.model_claim
            || turn.claim.node_id != cursor.node_id
            || canonical_hash(&turn.context) != cursor.context_sha256
            || turn.response_sha256.as_ref() != Some(&cursor.response_sha256)
            || response.rejection_code.is_some()
            || count >= response.tools.len()
            || count > turn.tool_results.len()
            || canonical_hash(&&turn.tool_results[..count]) != cursor.prefix_sha256
        {
            return Err(Error::Forbidden);
        }
    }
    Ok(())
}
impl Store {
    pub(crate) async fn resume_settled_workflow(
        &self,
        parent: &StoredRun,
        before: &StoredWorkflow,
    ) -> Result<RunRecord> {
        let (completed, cursors) = settled_recovery(before)?;
        let mut next = before.clone();
        next.progress.resume_settled_with_tools(
            &next.plan,
            next.progress.revision(),
            completed,
            cursors,
            seconds(),
        )?;
        let accepted=self.mutate(&parent.record.id,None,|r|{
            if r.record.revision!=parent.record.revision||r.record.state!=RunState::Interrupted||r.checkpoint.in_flight.is_some(){return Err(Error::Conflict);}
            if time_budget_exhausted(&r.record,now()){return Err(Error::BudgetExceeded);}
            r.transition(RunState::Queued)?;r.record.error_code=None;r.lease=None;
            Ok(Change{workflow_general_resume:true,workflow_write:Some(WorkflowWrite{new_action:true,expected_revision:before.progress.revision(),expected_sha256:canonical_hash(&before.progress),next:next.clone()}),events:vec![("workflow_settled_resume_queued".into(),json!({"before_sha256":canonical_hash(&before.progress),"after_sha256":canonical_hash(&next.progress),"automatic_retry":false,"reused_tool_receipts":true}))],..Default::default()})
        }).await?;
        Ok(accepted.record)
    }
}
