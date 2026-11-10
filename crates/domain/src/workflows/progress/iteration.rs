//! List expansion is part of the same Native decision and revision as capture.
//! Replaying original upstream outputs reconstructs every instance; serialized
//! manifests or rehashed checkpoints cannot independently authorize expansion.
use super::*;

impl WorkflowProgress {
    pub fn execution_graph<'a>(
        &self,
        plan: &'a WorkflowPlan,
    ) -> Result<WorkflowExecutionGraph<'a>> {
        self.validate(plan)?;
        self.raw_execution_graph(plan)
    }
    pub fn iteration_manifests(&self) -> &BTreeMap<String, WorkflowIterationManifest> {
        &self.iterations
    }
    pub(super) fn raw_execution_graph<'a>(
        &self,
        plan: &'a WorkflowPlan,
    ) -> Result<WorkflowExecutionGraph<'a>> {
        WorkflowExecutionGraph::derive(plan, &self.iterations)
    }
    pub(super) fn capture_iteration(
        &mut self,
        plan: &WorkflowExecutionGraph<'_>,
        node: &str,
        completed: &BTreeMap<String, String>,
    ) -> Result<()> {
        let Some((id, iteration)) = plan
            .iterations
            .iter()
            .find(|(_, iteration)| iteration.capture_node == node)
        else {
            return Ok(());
        };
        if self.iterations.contains_key(id) {
            return Err(Error::Conflict);
        }
        let frame =
            plan.definition
                .resolve_step_data(node, self.control_input.as_deref(), completed)?;
        let manifest = WorkflowIterationManifest::freeze(&iteration.source, id, &frame)?;
        let mut manifests = self.iterations.clone();
        manifests.insert(id.clone(), manifest);
        let graph = WorkflowExecutionGraph::derive(plan, &manifests)?;
        for (node, compiled) in graph.nodes.iter() {
            if let Some(existing) = self.nodes.get_mut(node) {
                // Only an as-yet-unclaimed loop return gains new dependencies.
                if existing.plan != *compiled {
                    if !plan.iterations.contains_key(node)
                        || existing.state != WorkflowNodeState::Pending
                        || existing.started_at.is_none()
                    {
                        return Err(Error::Forbidden);
                    }
                    existing.plan = compiled.clone();
                }
            } else {
                self.nodes.insert(
                    node.clone(),
                    WorkflowNodeProgress {
                        plan: compiled.clone(),
                        state: WorkflowNodeState::Pending,
                        started_at: None,
                    },
                );
            }
        }
        self.iterations = manifests;
        self.format_version = self.format_version.max(11);
        Ok(())
    }
    pub(super) fn iteration_capacity(&self, plan: &WorkflowExecutionGraph<'_>, node: &str) -> bool {
        plan.iteration_instances.iter().all(|(id, instances)| {
            let Some(position) = instances.iter().position(|instance| instance.entry == node)
            else {
                return true;
            };
            // Public format5 starts entries in input order. Completion may still
            // overlap up to the confirmed limit. Keep old private journals valid.
            if plan.iteration_definition
                && instances[..position]
                    .iter()
                    .any(|instance| self.nodes[&instance.entry].started_at.is_none())
            {
                return false;
            }
            let active = instances
                .iter()
                .filter(|instance| {
                    self.nodes[&instance.entry].started_at.is_some()
                        && !terminal_node(&self.nodes[&instance.exit].state)
                })
                .count();
            active < usize::from(plan.iterations[id].source.max_parallel)
        })
    }
    pub(super) fn collect_iteration(
        &self,
        plan: &WorkflowExecutionGraph<'_>,
        node: &str,
        completed: &BTreeMap<String, String>,
    ) -> Result<Option<serde_json::Value>> {
        let Some(iteration) = plan.iterations.get(node) else {
            return Ok(None);
        };
        let manifest = self.iterations.get(node).ok_or(Error::Forbidden)?;
        let frames = manifest.verify(&iteration.source, node, &manifest.input_sha256)?;
        let results = manifest
            .items
            .iter()
            .filter_map(|item| {
                completed
                    .get(&item.id)
                    .map(|output| (item.id.clone(), output.clone()))
            })
            .collect();
        Ok(Some(frames.collect(&results)?))
    }
}
