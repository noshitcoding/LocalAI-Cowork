//! Public frozen list sources, lowered into the same immutable parent plan.
use super::iteration_graph::{generated, original_id};
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowEmbeddedIteration {
    pub name: String,
    pub dependencies: Vec<String>,
    pub definition: Box<WorkflowDefinition>,
    pub collection_input: String,
    pub arguments: BTreeMap<String, WorkflowIterationArgument>,
    pub result: WorkflowOutputRef,
    pub rights: WorkflowRights,
    pub budgets: Budgets,
    pub max_parallel: u16,
}

impl WorkflowDefinition {
    pub fn iteration_source(&self) -> Result<WorkflowIterationProgram> {
        if self.format_version != 5 || self.iterations.is_empty() || self.iterations.len() > 16 {
            return invalid("workflow_iteration_invalid");
        }
        let mut workflow = self.clone();
        workflow.iterations.clear();
        workflow.format_version = if workflow.subflows.is_empty() { 3 } else { 4 };
        let mut iterations = BTreeMap::new();
        for (id, call) in &self.iterations {
            let data = workflow
                .dataflow
                .as_mut()
                .and_then(|data| data.steps.remove(id))
                .ok_or(Error::Forbidden)?;
            let input = WorkflowDataType::Object {
                fields: data
                    .inputs
                    .iter()
                    .map(|(key, value)| (key.clone(), value.value_type.clone()))
                    .collect(),
            };
            let source = WorkflowIterationSource {
                format_version: 1,
                name: call.name.clone(),
                input,
                collection_input: call.collection_input.clone(),
                arguments: call.arguments.clone(),
                body: call.definition.clone(),
                result: call.result.clone(),
                rights: call.rights.clone(),
                budgets: call.budgets.clone(),
                max_parallel: call.max_parallel,
            };
            source.validate()?;
            if source.output_type()? != data.output {
                return invalid("workflow_data_type_mismatch");
            }
            iterations.insert(
                id.clone(),
                WorkflowIterationNode {
                    source,
                    dependencies: call.dependencies.clone(),
                    inputs: data.inputs,
                },
            );
        }
        Ok(WorkflowIterationProgram {
            format_version: 1,
            workflow,
            iterations,
        })
    }
}

impl WorkflowIterationProgram {
    /// No provider lookup or run creation. This is the static capture/return
    /// skeleton; dynamic body instances remain derived from accepted data.
    pub(super) fn scaffold(&self) -> Result<WorkflowDefinition> {
        if self.format_version != 1
            || self.iterations.is_empty()
            || self.iterations.len() > 16
            || !matches!(self.workflow.format_version, 3 | 4)
            || (self.workflow.format_version == 4) == self.workflow.subflows.is_empty()
            || !self.workflow.iterations.is_empty()
            || !matches!(self.workflow.mode, WorkflowMode::Parallel { .. })
        {
            return invalid("workflow_iteration_invalid");
        }
        if serde_json::to_vec(self)
            .map_err(|_| Error::Forbidden)?
            .len()
            > 2 * 1024 * 1024
        {
            return invalid("workflow_iteration_source_limit");
        }
        let mut definition = self.workflow.clone();
        for id in definition
            .steps
            .iter()
            .map(|step| &step.id)
            .chain(definition.controls.keys())
            .chain(definition.documents.keys())
            .chain(definition.subflows.keys())
            .chain(definition.roles.iter().map(|role| &role.id))
        {
            original_id(id)?;
        }
        for (id, iteration) in &self.iterations {
            original_id(id)?;
            iteration.source.validate()?;
            if definition.steps.iter().any(|step| step.id == *id)
                || definition.controls.contains_key(id)
                || definition.documents.contains_key(id)
                || definition.subflows.contains_key(id)
            {
                return Err(Error::Conflict);
            }
            let input = WorkflowDataType::Object {
                fields: iteration
                    .inputs
                    .iter()
                    .map(|(key, value)| (key.clone(), value.value_type.clone()))
                    .collect(),
            };
            if input != iteration.source.input {
                return invalid("workflow_data_type_mismatch");
            }
            let capture = generated("capture", id, "");
            definition.controls.insert(
                capture.clone(),
                WorkflowControlStep {
                    name: iteration.source.name.clone(),
                    dependencies: iteration.dependencies.clone(),
                    operation: WorkflowControlOperation::Compose,
                },
            );
            definition.controls.insert(
                id.clone(),
                WorkflowControlStep {
                    name: iteration.source.name.clone(),
                    dependencies: vec![capture.clone()],
                    operation: WorkflowControlOperation::Collect { candidates: vec![] },
                },
            );
            let data = definition.dataflow.as_mut().ok_or(Error::Forbidden)?;
            if data.steps.contains_key(id) || data.steps.contains_key(&capture) {
                return Err(Error::Conflict);
            }
            data.steps.insert(
                capture.clone(),
                WorkflowStepData {
                    context_policy: WorkflowDataContext::Inherit,
                    inputs: iteration.inputs.clone(),
                    output: iteration.source.input.clone(),
                },
            );
            data.steps.insert(
                id.clone(),
                WorkflowStepData {
                    context_policy: WorkflowDataContext::Inherit,
                    inputs: BTreeMap::new(),
                    output: iteration.source.output_type()?,
                },
            );
            if let Some(gate) = definition.gates.remove(id) {
                definition.gates.insert(capture, gate);
            }
        }
        Ok(definition)
    }

    /// A library's executable shape includes every frozen body role once, so
    /// publication and profile collection do not depend on an example list.
    pub(super) fn validation_definition(&self) -> Result<WorkflowDefinition> {
        let mut definition = self.scaffold()?;
        for (id, iteration) in &self.iterations {
            let body = iteration.source.body.execution_definition()?;
            for mut role in body.roles {
                role.id = generated("role", id, &role.id);
                role.rights = role
                    .rights
                    .intersect(&body.rights)
                    .intersect(&iteration.source.rights);
                role.budgets = minimum_budgets(
                    &role.budgets,
                    &minimum_budgets(&body.budgets, &iteration.source.budgets),
                );
                definition.roles.push(role);
            }
        }
        Ok(definition)
    }

    pub fn source_definition(&self) -> Result<WorkflowDefinition> {
        self.scaffold()?;
        let mut definition = self.workflow.clone();
        definition.format_version = 5;
        for (id, iteration) in &self.iterations {
            let source = &iteration.source;
            definition.iterations.insert(
                id.clone(),
                WorkflowEmbeddedIteration {
                    name: source.name.clone(),
                    dependencies: iteration.dependencies.clone(),
                    definition: source.body.clone(),
                    collection_input: source.collection_input.clone(),
                    arguments: source.arguments.clone(),
                    result: source.result.clone(),
                    rights: source.rights.clone(),
                    budgets: source.budgets.clone(),
                    max_parallel: source.max_parallel,
                },
            );
            definition
                .dataflow
                .as_mut()
                .ok_or(Error::Forbidden)?
                .steps
                .insert(
                    id.clone(),
                    WorkflowStepData {
                        context_policy: WorkflowDataContext::Inherit,
                        inputs: iteration.inputs.clone(),
                        output: source.output_type()?,
                    },
                );
        }
        Ok(definition)
    }
}

impl WorkflowPlan {
    /// Check all pinned bodies before provider probes, even when their list is
    /// still unknown. Compilation already intersected every effective grant.
    pub fn validate_native_step_rights(&self) -> Result<()> {
        if self.definition.steps.iter().any(|step| {
            step.execution.is_some() && !self.nodes[&step.id].rights.tools.contains("browser")
        }) {
            return invalid(
                "Pipeline web steps require the Browser capability at workflow, role, step and run scope",
            );
        }
        for iteration in self.iterations.values() {
            iteration.body.validate_native_step_rights()?;
        }
        Ok(())
    }
}
