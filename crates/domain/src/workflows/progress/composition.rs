//! Subflow guards use the one parent attempt ledger. No child budget is minted.
use super::*;

impl WorkflowProgress {
    pub(super) fn composition_entry(plan: &WorkflowExecutionGraph<'_>, node: &str) -> bool {
        plan.composition_scopes
            .values()
            .any(|scope| scope.entry_node == node)
    }
    pub(super) fn composition_capacity(
        &self,
        plan: &WorkflowExecutionGraph<'_>,
        node: &str,
    ) -> bool {
        plan.composition_scopes
            .iter()
            .filter(|(scope, _)| self.descendant(node, scope))
            .all(|(scope, limit)| {
                self.nodes
                    .values()
                    .filter(|candidate| {
                        matches!(candidate.state, WorkflowNodeState::Running { .. })
                            && self.descendant(&candidate.plan.id, scope)
                    })
                    .count()
                    < usize::from(limit.max_parallel)
            })
    }
    pub(super) fn composition_stopped(
        &self,
        plan: &WorkflowExecutionGraph<'_>,
        node: &str,
    ) -> bool {
        plan.composition_scopes.iter().any(|(scope, limit)| {
            limit.failure == WorkflowFailure::Stop
                && self.descendant(node, scope)
                && self.nodes.values().any(|candidate| {
                    matches!(
                        candidate.state,
                        WorkflowNodeState::Failed { .. } | WorkflowNodeState::Blocked
                    ) && self.descendant(&candidate.plan.id, scope)
                })
        })
    }
    pub(super) fn start_composition(
        &mut self,
        plan: &WorkflowExecutionGraph<'_>,
        node: &str,
        at: i64,
    ) {
        for (exit, scope) in plan.composition_scopes.iter() {
            if scope.entry_node == node {
                self.nodes
                    .get_mut(exit)
                    .expect("compiled call exit")
                    .started_at
                    .get_or_insert(at);
            }
        }
        if plan.composition.is_some() {
            self.format_version = self.format_version.max(10);
        }
        if plan.iteration_program.is_some() {
            self.format_version = self.format_version.max(11);
        }
    }
}
