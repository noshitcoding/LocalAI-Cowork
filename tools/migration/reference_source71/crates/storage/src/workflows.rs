//! Trusted application-service primitives, not an HTTP authority or IO executor.
use super::*;
use cowork_domain::workflows::*;
use serde::{Deserialize, Serialize};
pub(super) mod agents;
pub(super) mod completion;
pub(super) mod evidence;
pub(crate) mod inputs;
pub(crate) mod product;
pub(super) mod registry;
pub(super) mod resume;
pub use agents::{
    WorkflowModelAdmission, WorkflowModelContext, WorkflowModelPermit, WorkflowModelResponse,
    WorkflowModelTurn,
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
    pub fn validate(&self, run: &RunRecord) -> Result<()> {
        self.plan.validate()?;
        self.progress.validate(&self.plan)?;
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
        {
            return Err(Error::Forbidden);
        }
        agents::validate(self, run)?;
        mcp_sampling::validate(self, run)?;
        tools::validate_receipts(self)?;
        inputs::validate(self, run)?;
        continuations::validate(self, run)?;
        resume::validate(self)?;
        Ok(())
    }
}
pub enum WorkflowCommand {
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
