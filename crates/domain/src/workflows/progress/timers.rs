//! Durable timer transitions. Scheduling and completion never grant IO permits.
use super::*;
impl WorkflowProgress {
    pub fn complete_timer(
        &mut self,
        plan: &WorkflowPlan,
        expected: u64,
        claim: &WorkflowClaim,
        id: &str,
        input: Option<&serde_json::Value>,
        at_ms: i64,
    ) -> Result<()> {
        let graph = self.execution_graph(plan)?;
        if !matches!(
            graph
                .definition
                .controls
                .get(&claim.node_id)
                .map(|s| &s.operation),
            Some(WorkflowControlOperation::Wait { .. })
        ) {
            return Err(Error::Forbidden);
        }
        plan.definition.validate_data_input(input)?;
        if self
            .control_input
            .as_deref()
            .is_some_and(|saved| Some(saved) != input)
        {
            return Err(Error::Forbidden);
        }
        self.edit(
            plan,
            expected,
            at_ms.div_euclid(1000),
            Decision::Native {
                id: id.into(),
                claim: claim.clone(),
                input: if self.control_input.is_none() {
                    input.cloned().map(Box::new)
                } else {
                    None
                },
                clock_ms: Some(at_ms),
            },
        )
    }
    pub fn next_wake_at(&self) -> Option<i64> {
        self.nodes
            .values()
            .filter_map(|n| {
                if let WorkflowNodeState::WaitingTimer { wake_at, .. } = n.state {
                    Some(wake_at)
                } else {
                    None
                }
            })
            .min()
    }
    pub fn can_park_timer(&self, plan: &WorkflowPlan) -> Result<bool> {
        self.validate(plan)?;
        Ok(self.timer_quiescent(&self.raw_execution_graph(plan)?))
    }
    // Used by deterministic replay too: never call validate from a reducer.
    pub(super) fn timer_quiescent(&self, plan: &WorkflowExecutionGraph<'_>) -> bool {
        self.state() == WorkflowState::Running
            && self.next_wake_at().is_some()
            && !self
                .nodes
                .values()
                .any(|node| self.ready(node) && self.iteration_capacity(plan, &node.plan.id))
            && !self.nodes.values().any(|n| {
                matches!(
                    n.state,
                    WorkflowNodeState::Running { .. }
                        | WorkflowNodeState::WaitingChildren
                        | WorkflowNodeState::Interrupted
                )
            })
            && !self
                .attempts
                .values()
                .any(|a| a.outcome.is_none() || a.outcome == Some(WorkflowOutcome::Uncertain))
    }
    pub fn advance_timers(&mut self, plan: &WorkflowPlan, expected: u64, at: i64) -> Result<bool> {
        let nodes = self.due_timers(at);
        if nodes.is_empty() {
            self.validate(plan)?;
            return Ok(false);
        }
        self.edit(plan, expected, at, Decision::TimersAdvanced { nodes })?;
        Ok(true)
    }
    fn due_timers(&self, at: i64) -> Vec<String> {
        self.nodes.iter().filter_map(|(id,n)| if matches!(n.state, WorkflowNodeState::WaitingTimer { wake_at, .. } if wake_at <= at) { Some(id.clone()) } else { None }).collect()
    }
    pub(super) fn apply_timers_advanced(
        &mut self,
        plan: &WorkflowExecutionGraph<'_>,
        nodes: &[String],
        at: i64,
    ) -> Result<()> {
        if self.state() == WorkflowState::Interrupted
            || nodes.is_empty()
            || self.due_timers(at) != nodes
        {
            return Err(Error::Conflict);
        }
        for id in nodes {
            self.no_pending(id)?;
            let node = &self.nodes[id];
            let WorkflowNodeState::WaitingTimer { output, sha256, .. } = &node.state else {
                return Err(Error::Forbidden);
            };
            let state = if self.within_time(plan, id, at).is_err() {
                WorkflowNodeState::Failed {
                    code: "budget_exceeded".into(),
                }
            } else {
                WorkflowNodeState::Completed {
                    output: output.clone(),
                    sha256: sha256.clone(),
                }
            };
            self.nodes.get_mut(id).ok_or(Error::NotFound)?.state = state;
        }
        self.format_version = self.format_version.max(8);
        Ok(())
    }
    pub fn close_timers(
        &mut self,
        plan: &WorkflowPlan,
        expected: u64,
        reason: &str,
        at: i64,
    ) -> Result<()> {
        if self.next_wake_at().is_none() {
            self.validate(plan)?;
            return Ok(());
        }
        self.edit(
            plan,
            expected,
            at,
            Decision::TimersClosed {
                reason: reason.into(),
            },
        )
    }
    pub(super) fn apply_timers_closed(&mut self, reason: &str) -> Result<()> {
        if !matches!(reason, "cancelled" | "budget_exceeded" | "workflow_revoked") {
            return Err(Error::Forbidden);
        }
        if self.next_wake_at().is_none() {
            return Err(Error::Conflict);
        }
        for node in self.nodes.values_mut() {
            if matches!(node.state, WorkflowNodeState::WaitingTimer { .. }) {
                node.state = WorkflowNodeState::Failed {
                    code: reason.into(),
                };
            }
        }
        self.format_version = self.format_version.max(8);
        Ok(())
    }
}
