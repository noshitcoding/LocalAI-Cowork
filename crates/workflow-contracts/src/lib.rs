//! Public workflow API. Private plans, leases and permits stay inside the application.
mod execution_plans;
mod execution_plans_v2;
mod input;
pub use execution_plans_v2::*;
mod workflow_input_source;
pub use workflow_input_source::*;
mod restore_execution_plans;
pub use restore_execution_plans::*;
mod import_execution_plans;
pub use execution_plans::*;
pub use import_execution_plans::*;
mod team_execution;
use cowork_contracts::*;
pub use cowork_domain::workflows::{
    WorkflowDataContext, WorkflowDataSource, WorkflowDataType, WorkflowDataflow,
    WorkflowDefinition, WorkflowFailure, WorkflowMode, WorkflowRights, WorkflowRole, WorkflowStep,
    WorkflowStepData, WorkflowStepExecution, WorkflowValueBinding,
};
pub use input::*;
pub use team_execution::*;
mod journal;
pub use journal::*;
mod productivity_history;
pub use productivity_history::*;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowVersionRef {
    pub id: String,
    pub revision: i64,
    pub name: String,
    pub sha256: String,
    pub created_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowLibraryEntry {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub revision: i64,
    pub revoked: bool,
    pub versions: Vec<WorkflowVersionRef>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowPublication {
    pub workflow_id: String,
    pub project_id: String,
    pub revision: i64,
    pub version_id: Option<String>,
    pub revoked: bool,
    pub created_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowVersion {
    pub workflow_id: String,
    pub project_id: String,
    pub reference: WorkflowVersionRef,
    pub definition: WorkflowDefinition,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowPage {
    pub items: Vec<WorkflowLibraryEntry>,
    pub total: i64,
    pub next_offset: Option<u32>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateWorkflow {
    pub idempotency_key: String,
    pub definition: WorkflowDefinition,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PublishWorkflow {
    pub base_revision: i64,
    pub idempotency_key: String,
    pub definition: WorkflowDefinition,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RevokeWorkflow {
    pub base_revision: i64,
    pub idempotency_key: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRunKind {
    Workflow,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRunRequest {
    /// Immutable original input, resolved only by the confirmed PlanV2 path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_source: Option<WorkflowInputSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<Box<serde_json::Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_config: Option<EffectiveAgentConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<FolderGrant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_tools: Vec<McpToolSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<ExecutionEnvironmentRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native_tools: Vec<String>,
    pub kind: WorkflowRunKind,
    pub order_id: String,
    pub base_revision: i64,
    pub idempotency_key: String,
    pub prompt: String,
    pub workflow_id: String,
    pub version_id: String,
    #[serde(default)]
    pub access_mode: AccessMode,
    #[serde(default)]
    pub budgets: Budgets,
    /// An expectation to compare, never authority to grant rights.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_binding: Option<WorkflowBinding>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum RunRequest {
    Single(CreateRun),
    Workflow(WorkflowRunRequest),
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum RunPreflight {
    Single(Preflight),
    Workflow(WorkflowPreflight),
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowModelView {
    pub role_id: String,
    pub provider: ProviderProfile,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowPreflight {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_source: Option<WorkflowInputSource>,
    pub ready: bool,
    pub workflow: WorkflowBinding,
    pub name: String,
    pub version_revision: i64,
    pub executor: String,
    pub policy_revision: i64,
    pub access_mode: AccessMode,
    pub budgets: Budgets,
    pub allowed_tools: Vec<String>,
    pub roles: Vec<WorkflowModelView>,
    pub max_parallel: u16,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowPublicState {
    WaitingTimer,
    Queued,
    Running,
    Interrupted,
    Completed,
    Failed,
    Canceled,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowNodeStatus {
    WaitingTimer,
    Skipped,
    Pending,
    Ready,
    Running,
    WaitingApproval,
    WaitingInput,
    Completed,
    Failed,
    Blocked,
    Interrupted,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowNodeKind {
    Subflow,
    Arguments,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowNodeView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<WorkflowNodeKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_id: Option<String>,
    pub goal: String,
    pub dependencies: Vec<String>,
    pub state: WorkflowNodeStatus,
    pub error_code: Option<String>,
    pub steps: u32,
    pub charged_tokens: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRunApproval {
    pub approval: Approval,
    pub node_id: String,
    /// Native document actions have no model role. Existing roles serialize unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRunView {
    pub run: RunRecord,
    pub binding: WorkflowBinding,
    pub name: String,
    pub version_revision: i64,
    pub mode: WorkflowMode,
    pub max_parallel: u16,
    pub state: WorkflowPublicState,
    pub roles: Vec<WorkflowModelView>,
    pub nodes: Vec<WorkflowNodeView>,
    pub approvals: Vec<WorkflowRunApproval>,
    #[serde(default)]
    pub inputs: Vec<WorkflowInputView>,
    #[serde(default)]
    pub input_resume_available: bool,
    #[serde(default)]
    pub input_resume_preview: WorkflowInputResumePreview,
    #[serde(default)]
    pub input_resume_blocked_reason: Option<String>,
    pub completion_resume_available: bool,
    #[serde(default)]
    pub settled_resume_available: bool,
    #[serde(default)]
    pub settled_resume_blocked_reason: Option<String>,
    pub charged_tokens: u64,
    pub steps_used: u32,
}
/// Aggregate schema root keeps the legacy definitions and adds public workflow contracts.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowUnresolvedEffect {
    pub id: String,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowEffectResolutionView {
    pub run: RunRecord,
    pub settled_calls: Vec<String>,
    pub unresolved_calls: Vec<WorkflowUnresolvedEffect>,
    pub automatic_retry: bool,
}
#[derive(JsonSchema)]
pub struct ApiContractV2 {
    pub workflow_input_snapshot: WorkflowInputSnapshot,
    pub prepare_restore_execution_plan_v2: PrepareRestoreExecutionPlanV2,
    pub prepare_import_execution_plan_v2: PrepareImportExecutionPlanV2,
    pub prepare_owner_execution_plan_v2: PrepareOwnerExecutionPlanV2,
    pub prepared_execution_plan_v2: PreparedExecutionPlanV2,
    pub prepare_execution_input: PrepareExecutionInput,
    pub prepared_execution_input: PreparedExecutionInput,
    pub retire_execution_input: RetireExecutionInput,
    pub workspace_transfer_manifest: WorkspaceTransferManifest,
    pub read_workspace_transfer_chunk: ReadWorkspaceTransferChunk,
    pub prepared_execution_plan: PreparedExecutionPlan,
    pub commit_execution_plan: CommitExecutionPlan,
    pub retired_execution_request: RetiredExecutionRequest,
    pub manage_sandbox_session: ManageSandboxSession,
    pub sandbox_session: SandboxSession,
    pub create_sandbox_preview: CreateSandboxPreview,
    pub sandbox_preview: SandboxPreview,
    pub sandbox_session_operation: SandboxSessionOperation,
    pub scheduled_run_template: ScheduledRunTemplate,
    pub product_schedule: ProductSchedule,
    pub write_schedule: WriteSchedule,
    pub native_schedule_history: NativeScheduleHistory,
    pub native_deleted_schedule: NativeDeletedSchedule,
    #[serde(flatten)]
    pub core: cowork_contracts::ApiContract,
    pub run_request: RunRequest,
    pub run_preflight: RunPreflight,
    pub create_workflow: CreateWorkflow,
    pub publish_workflow: PublishWorkflow,
    pub revoke_workflow: RevokeWorkflow,
    pub workflow_page: WorkflowPage,
    pub workflow_version: WorkflowVersion,
    pub workflow_publication: WorkflowPublication,
    pub workflow_run: WorkflowRunView,
    pub workflow_effect_resolution: WorkflowEffectResolutionView,
    pub save_workflow_draft: SaveWorkflowDraft,
    pub workflow_draft_save: WorkflowDraftSave,
    pub workflow_draft_page: WorkflowDraftPage,
    pub workflow_draft_snapshot: WorkflowDraftSnapshot,
    pub workflow_draft_snapshot_page: WorkflowDraftSnapshotPage,
    pub prepare_workflow_operation: PrepareWorkflowOperation,
    pub workflow_operation_page: WorkflowOperationPage,
    pub submit_workflow_input: SubmitWorkflowInput,
    pub workflow_input_draft_save: WorkflowInputDraftSave,
    pub workflow_input_draft_lookup: WorkflowInputDraftLookup,
    pub save_workflow_input_draft: SaveWorkflowInputDraft,
    pub prepare_workflow_input_answer: PrepareWorkflowInputAnswer,
    pub workflow_input_answer_intent: WorkflowInputAnswerIntent,
    pub workflow_input_answer_export: WorkflowInputAnswerExport,
}
