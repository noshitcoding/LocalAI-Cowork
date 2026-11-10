//! Owner-scoped durable human answers. Saved drafts never authorize execution.
use cowork_contracts::SubmitInput;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Read-only expectations for an explicit continuation. This view grants no authority.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInputResumePreview {
    pub available: bool,
    pub blocked_reason: Option<String>,
    pub base_run_revision: i64,
    pub base_workflow_revision: u64,
    pub expected_checkpoint_sha256: String,
    pub expected_cursor_sha256: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResumeWorkflowInput {
    pub run_id: String,
    pub base_run_revision: i64,
    pub base_workflow_revision: u64,
    pub expected_checkpoint_sha256: String,
    pub expected_cursor_sha256: String,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowInputState {
    WaitingAnswer,
    Answered,
    Closed,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInputReceipt {
    pub request_id: String,
    pub fingerprint: String,
    pub receipt_id: String,
    pub answer_sha256: String,
    pub confirmed_sequence: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInputView {
    pub request_id: String,
    pub fingerprint: String,
    pub node_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<cowork_domain::workflows::WorkflowReviewRequest>,
    pub question: String,
    pub state: WorkflowInputState,
    pub expires_at: i64,
    pub actionable: bool,
    pub receipt: Option<WorkflowInputReceipt>,
    pub closed_reason: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubmitWorkflowInput {
    pub input: SubmitInput,
    pub fingerprint: String,
    pub idempotency_key: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInputDraft {
    pub run_id: String,
    pub project_id: String,
    pub request_id: String,
    pub fingerprint: String,
    pub owner: String,
    pub revision: i64,
    pub answer: String,
    pub answer_sha256: String,
    pub snapshot_id: Option<String>,
    pub updated_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInputDraftSnapshot {
    pub id: String,
    pub run_id: String,
    pub project_id: String,
    pub request_id: String,
    pub fingerprint: String,
    pub owner: String,
    pub base_revision: i64,
    pub revision: i64,
    pub answer: String,
    pub answer_sha256: String,
    pub conflict: bool,
    pub created_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInputDraftSnapshotSummary {
    pub id: String,
    pub base_revision: i64,
    pub revision: i64,
    pub answer_sha256: String,
    pub conflict: bool,
    pub created_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveWorkflowInputDraft {
    pub fingerprint: String,
    pub base_revision: i64,
    pub idempotency_key: String,
    pub answer: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowInputDraftSave {
    Saved {
        draft: WorkflowInputDraft,
        snapshot: WorkflowInputDraftSnapshot,
    },
    Conflict {
        draft: WorkflowInputDraft,
        snapshot: WorkflowInputDraftSnapshot,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrepareWorkflowInputAnswer {
    pub fingerprint: String,
    pub base_draft_revision: i64,
    pub idempotency_key: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInputDraftSnapshotRef {
    pub snapshot_id: String,
    pub revision: i64,
    pub answer_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowInputAnswerIntentState {
    Pending,
    Accepted,
    Rejected,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInputAnswerRejection {
    pub code: String,
    pub message: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInputAnswerIntent {
    pub id: String,
    pub run_id: String,
    pub project_id: String,
    pub request_id: String,
    pub fingerprint: String,
    pub owner: String,
    pub request_sha256: String,
    pub submission: SubmitWorkflowInput,
    pub draft_snapshot: WorkflowInputDraftSnapshotRef,
    pub state: WorkflowInputAnswerIntentState,
    pub receipt: Option<WorkflowInputReceipt>,
    pub rejection: Option<WorkflowInputAnswerRejection>,
    pub created_at: i64,
    pub updated_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInputAnswerIntentSummary {
    pub id: String,
    pub state: WorkflowInputAnswerIntentState,
    pub answer_sha256: String,
    pub draft_revision: i64,
    pub created_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInputDraftLookup {
    pub draft: WorkflowInputDraft,
    pub snapshots: Vec<WorkflowInputDraftSnapshotSummary>,
    pub intents: Vec<WorkflowInputAnswerIntentSummary>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInputAnswerExport {
    pub owner: String,
    pub drafts: Vec<WorkflowInputDraft>,
    pub snapshots: Vec<WorkflowInputDraftSnapshot>,
    pub intents: Vec<WorkflowInputAnswerIntent>,
}
