//! Public immutable subflow sources. The source hash covers this original
//! tree; generated argument/return nodes remain private compilation details.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowEmbeddedSubflow {
    pub name: String,
    pub dependencies: Vec<String>,
    pub definition: Box<WorkflowDefinition>,
    pub result: WorkflowOutputRef,
    pub rights: WorkflowRights,
    pub budgets: Budgets,
}

impl WorkflowDefinition {
    pub fn execution_definition(&self) -> Result<WorkflowDefinition> {
        if self.format_version == 5 || !self.iterations.is_empty() {
            self.iteration_source()?
                .validation_definition()?
                .execution_definition()
        } else if self.format_version == 4 || !self.subflows.is_empty() {
            self.composition_source()?.execution_definition()
        } else {
            Ok(self.clone())
        }
    }
    pub fn composition_source(&self) -> Result<WorkflowComposition> {
        fn convert(
            definition: &WorkflowDefinition,
            depth: usize,
            count: &mut usize,
        ) -> Result<WorkflowComposition> {
            if depth > 4
                || !definition.iterations.is_empty()
                || definition.subflows.len() > 16
                || (definition.format_version == 4) == definition.subflows.is_empty()
            {
                return invalid("workflow_composition_invalid");
            }
            *count += definition.subflows.len();
            if *count > 16 {
                return invalid("workflow_composition_invalid");
            }
            let mut workflow = definition.clone();
            workflow.subflows.clear();
            if workflow.format_version == 4 {
                workflow.format_version = 3;
            }
            let mut calls = BTreeMap::new();
            for (id, call) in &definition.subflows {
                calls.insert(
                    id.clone(),
                    WorkflowSubflowCall {
                        name: call.name.clone(),
                        dependencies: call.dependencies.clone(),
                        body: Box::new(convert(&call.definition, depth + 1, count)?),
                        result: call.result.clone(),
                        rights: call.rights.clone(),
                        budgets: call.budgets.clone(),
                    },
                );
            }
            Ok(WorkflowComposition {
                format_version: 1,
                workflow,
                calls,
            })
        }
        if self.format_version != 4 || self.subflows.is_empty() {
            return invalid("workflow_composition_invalid");
        }
        convert(self, 0, &mut 0)
    }
}
impl WorkflowComposition {
    pub fn source_definition(&self) -> Result<WorkflowDefinition> {
        fn convert(
            source: &WorkflowComposition,
            depth: usize,
            count: &mut usize,
        ) -> Result<WorkflowDefinition> {
            if source.format_version != 1
                || depth > 4
                || !source.workflow.subflows.is_empty()
                || !source.workflow.iterations.is_empty()
            {
                return invalid("workflow_composition_invalid");
            }
            *count += source.calls.len();
            if *count > 16 {
                return invalid("workflow_composition_invalid");
            }
            let mut result = source.workflow.clone();
            if !source.calls.is_empty() {
                result.format_version = 4;
            }
            for (id, call) in &source.calls {
                result.subflows.insert(
                    id.clone(),
                    WorkflowEmbeddedSubflow {
                        name: call.name.clone(),
                        dependencies: call.dependencies.clone(),
                        definition: Box::new(convert(&call.body, depth + 1, count)?),
                        result: call.result.clone(),
                        rights: call.rights.clone(),
                        budgets: call.budgets.clone(),
                    },
                );
            }
            Ok(result)
        }
        convert(self, 0, &mut 0)
    }
}
