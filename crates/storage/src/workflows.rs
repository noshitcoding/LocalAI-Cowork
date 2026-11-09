//! Trusted application-service primitives, not an HTTP authority or IO executor.
use super::*;
use cowork_domain::workflows::*;
use serde::{Deserialize, Serialize};
pub(super) mod agents;
pub(super) mod completion;
pub(super) mod documents;
pub(super) mod evidence;
pub(crate) mod inputs;
mod native_documents;
pub(crate) mod native_reviews;
mod presentation;
pub(crate) mod product;
mod resources;
pub(super) mod timers;
pub use documents::{WorkflowDocumentContext, document_execution_scope};
pub(super) mod registry;
pub(super) mod resume;
pub use agents::{
    WorkflowModelAdmission, WorkflowModelContext, WorkflowModelPermit, WorkflowModelResponse,
    WorkflowModelTurn, deterministic_step_output,
};
pub(crate) mod continuations;
mod mcp_sampling;
mod mcp_tasks;
pub(super) mod runs;
pub(super) mod tools;
pub(crate) use mcp_sampling::context as sampling_model_context;
pub use mcp_tasks::{WorkflowMcpContext, WorkflowMcpSource};
pub use tools::{WorkflowToolExecution, WorkflowToolReceipt};

pub(crate) fn model_admission_deadline(
    parent: &StoredRun,
    write: &WorkflowWrite,
    id: &str,
) -> Result<i64> {
    mcp_sampling::deadline(parent, &write.next, id)
}

pub use cowork_workflow_contracts::{
    WorkflowLibraryEntry, WorkflowPublication, WorkflowVersion, WorkflowVersionRef,
};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateWorkflowRun {
    pub order_id: String,
    pub base_revision: i64,
    pub idempotency_key: String,
    pub workflow_id: String,
    pub version_id: String,
    /// Trusted target admission bound. This type is not accepted by an HTTP API.
    pub max_parallel: u16,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredWorkflow {
    pub format_version: u32,
    pub run_id: String,
    pub project_id: String,
    pub binding: WorkflowBinding,
    pub plan: WorkflowPlan,
    pub progress: WorkflowProgress,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub agents: std::collections::BTreeMap<String, agents::WorkflowAgent>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub inputs: std::collections::BTreeMap<String, inputs::StoredWorkflowInput>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub samplings: std::collections::BTreeMap<String, mcp_sampling::WorkflowSamplingModel>,
}
impl StoredWorkflow {
    /// Reconstructs dynamic instances from the original plan and journal.
    /// This view is data only; the immutable plan remains the admission binding.
    pub fn execution_graph(&self) -> Result<WorkflowExecutionGraph<'_>> {
        self.progress.execution_graph(&self.plan)
    }
    /// Metadata lookup only. Static plans already contain the full definition;
    /// revalidating their entire journal for every saved turn is unnecessary.
    pub(super) fn execution_definition(&self) -> Result<std::borrow::Cow<'_, WorkflowDefinition>> {
        if self.plan.iteration_program.is_none() {
            return Ok(std::borrow::Cow::Borrowed(&self.plan.definition));
        }
        Ok(self.execution_graph()?.definition)
    }
    /// Classification only; deserializing metadata never authorizes dispatch.
    pub fn is_deterministic_step(&self, node: &str) -> Result<bool> {
        let definition = self.execution_definition()?;
        Ok(definition.documents.contains_key(node)
            || definition.review_form(node).is_some()
            || definition
                .steps
                .iter()
                .any(|step| step.id == node && step.execution.is_some()))
    }
    pub fn step_execution(&self, node: &str) -> Result<Option<WorkflowStepExecution>> {
        Ok(self
            .execution_definition()?
            .steps
            .iter()
            .find(|step| step.id == node)
            .and_then(|step| step.execution.clone()))
    }
    pub(super) fn execution_scopes(
        &self,
    ) -> Result<std::borrow::Cow<'_, std::collections::BTreeMap<String, WorkflowCompositionScope>>>
    {
        if self.plan.iteration_program.is_none() {
            return Ok(std::borrow::Cow::Borrowed(&self.plan.composition_scopes));
        }
        Ok(self.execution_graph()?.composition_scopes)
    }
    pub fn validate(&self, run: &RunRecord) -> Result<()> {
        self.plan.validate()?;
        self.plan
            .definition
            .validate_data_input(self.binding.inputs.as_deref())?;
        let graph = self.progress.execution_graph(&self.plan)?;
        if self
            .progress
            .control_input()
            .is_some_and(|input| Some(input) != self.binding.inputs.as_deref())
        {
            return Err(Error::Forbidden);
        }
        if !matches!(self.format_version, 1..=5)
            || self.run_id != run.id
            || self.project_id != run.spec.project_id
            || run.spec.workflow.as_ref() != Some(&self.binding)
            || run.spec.provider.is_some()
            || run.spec.file_action.is_some()
            || self.binding.plan_sha256 != self.plan.sha256
            || self.binding.definition_sha256 != self.plan.source_sha256
            || self.plan.budgets != run.spec.budgets
            || self.plan.scope.rights.tools != run.spec.allowed_tools.iter().cloned().collect()
            || self.plan.scope.rights.folders
                != run.spec.folders.iter().map(|f| f.id.clone()).collect()
            || self.plan.scope.rights.current_workspace
            || self.plan.scope.workspace_folder
                != resources::workspace_folder(
                    self.plan
                        .resource_definition
                        .as_deref()
                        .unwrap_or(&self.plan.definition),
                    &run.spec,
                )?
        {
            return Err(Error::Forbidden);
        }
        agents::validate(self, run, &graph)?;
        mcp_sampling::validate(self, run)?;
        tools::validate_receipts(self)?;
        inputs::validate(self, run)?;
        continuations::validate(self, run)?;
        resume::validate(self)?;
        Ok(())
    }
}
pub enum WorkflowCommand {
    Control {
        claim: WorkflowClaim,
        id: String,
    },
    Claim {
        node_id: String,
    },
    Admit {
        claim: WorkflowClaim,
        id: String,
        operation: WorkflowOperation,
    },
    Settle {
        claim: WorkflowClaim,
        id: String,
        outcome: WorkflowOutcome,
    },
    Complete {
        claim: WorkflowClaim,
        output: String,
    },
    Fail {
        claim: WorkflowClaim,
        code: String,
    },
    Delegate {
        claim: WorkflowClaim,
        id: String,
        requests: Vec<WorkflowDelegation>,
    },
}
#[derive(Debug)]
pub enum WorkflowCommandResult {
    Claimed(WorkflowClaim),
    Admission(WorkflowAdmission),
    Delegated(Vec<String>),
    Confirmed,
}
pub(super) struct WorkflowWrite {
    pub(super) new_action: bool,
    pub(super) expected_revision: u64,
    pub(super) expected_sha256: String,
    pub(super) next: StoredWorkflow,
}
#[derive(Serialize)]
pub(super) struct WorkflowStart {
    #[serde(skip_serializing_if = "Option::is_none")]
    inputs: Option<Box<Value>>,
    workflow_id: String,
    version_id: String,
    max_parallel: u16,
    product: bool,
    expected_binding: Option<WorkflowBinding>,
}
fn seconds() -> i64 {
    now().div_euclid(1000)
}
fn check_definition(definition: &WorkflowDefinition) -> Result<()> {
    definition.validate_definition()?;
    if json_string(definition)?.len() > 2 * 1024 * 1024 {
        return Err(Error::Validation(
            "Workflow definition exceeds storage limit".into(),
        ));
    }
    Ok(())
}
