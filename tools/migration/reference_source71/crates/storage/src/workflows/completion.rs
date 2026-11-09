//! Derives parent results from the validated terminal graph in the run transaction.
use super::*;
impl Store {
    pub async fn finish_workflow_run(
        &self,
        project: &str,
        run: &str,
        lease: &str,
    ) -> Result<StoredRun> {
        let workflow = self.stored_workflow(project, run).await?;
        let parent = self.stored_run(run).await?;
        if matches!(parent.record.state, RunState::Completed | RunState::Failed) {
            let expected = if parent.record.state == RunState::Completed {
                WorkflowState::Completed
            } else {
                WorkflowState::Failed
            };
            return if workflow.progress.state() == expected
                || revoked_before_start(&parent, &workflow)
            {
                Ok(parent)
            } else {
                Err(Error::Conflict)
            };
        }
        self.mutate(run, Some(lease), |r| {
            if r.record.spec.project_id != project || r.record.state != RunState::Running {
                return Err(Error::Conflict);
            }
            Ok(Change {
                workflow_finish: true,
                ..Default::default()
            })
        })
        .await
    }
}
/// Success shows terminal root results, failure preserves completed root outputs.
/// All intermediate and child answers remain in the immutable export/checkpoint.
pub(super) fn result_content(workflow: &StoredWorkflow) -> Result<String> {
    let completed = workflow.progress.state() == WorkflowState::Completed;
    let nodes = workflow.progress.nodes();
    let outputs: Vec<_> = nodes
        .values()
        .filter_map(|node| {
            if node.plan.parent.is_some() {
                return None;
            }
            if completed
                && nodes
                    .values()
                    .any(|other| other.plan.dependencies.contains(&node.plan.id))
            {
                return None;
            }
            match &node.state {
                WorkflowNodeState::Completed { output, .. } => Some((&node.plan.goal, output)),
                _ => None,
            }
        })
        .collect();
    let content = if outputs.len() == 1 {
        outputs[0].1.clone()
    } else {
        outputs
            .into_iter()
            .map(|(goal, output)| format!("{goal}\n\n{output}"))
            .collect::<Vec<_>>()
            .join("\n\n---\n\n")
    };
    // Bounded node count, goal and output sizes fit this bound; never truncate.
    if content.len() > 12 * 1024 * 1024 {
        return Err(Error::ContextLimit);
    }
    Ok(content)
}

/// This is a parent admission result, not fabricated failed agent work.
/// A revoked graph was never started and its complete original data is retained.
pub(super) fn revoked_before_start(parent: &StoredRun, workflow: &StoredWorkflow) -> bool {
    parent.record.state == RunState::Failed
        && parent.record.error_code.as_deref() == Some("workflow_revoked")
        && parent.record.started_at.is_none()
        && !parent.checkpoint.completion_pending
        && workflow.progress.state() == WorkflowState::Ready
        && workflow.progress.revision() == 0
        && workflow.progress.attempts().is_empty()
        && workflow.agents.is_empty()
        && workflow
            .progress
            .nodes()
            .values()
            .all(|node| node.state == WorkflowNodeState::Pending && node.started_at.is_none())
}
