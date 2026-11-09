//! Durable editor data and immutable commands. Draft content grants no authority.
use super::*;
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDraftOrigin {
    pub workflow_id: String,
    pub version_id: String,
    pub base_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDraftContent {
    pub editor_format: String,
    /// Incomplete editor values are durable; publication validates a separate definition.
    pub data: Value,
    #[serde(default)]
    pub origin: Option<WorkflowDraftOrigin>,
    pub archived: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveWorkflowDraft {
    pub id: String,
    pub base_revision: i64,
    pub idempotency_key: String,
    pub content: WorkflowDraftContent,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDraft {
    pub id: String,
    pub project_id: String,
    pub revision: i64,
    pub snapshot_id: String,
    pub sha256: String,
    pub content: WorkflowDraftContent,
    pub updated_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDraftSnapshot {
    pub id: String,
    pub draft_id: String,
    pub project_id: String,
    pub base_revision: i64,
    pub revision: i64,
    pub content: WorkflowDraftContent,
    pub sha256: String,
    pub conflict: bool,
    pub created_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDraftSnapshotSummary {
    pub id: String,
    pub draft_id: String,
    pub project_id: String,
    pub base_revision: i64,
    pub revision: i64,
    pub sha256: String,
    pub conflict: bool,
    pub created_at: i64,
}
impl From<&WorkflowDraftSnapshot> for WorkflowDraftSnapshotSummary {
    fn from(snapshot: &WorkflowDraftSnapshot) -> Self {
        Self {
            id: snapshot.id.clone(),
            draft_id: snapshot.draft_id.clone(),
            project_id: snapshot.project_id.clone(),
            base_revision: snapshot.base_revision,
            revision: snapshot.revision,
            sha256: snapshot.sha256.clone(),
            conflict: snapshot.conflict,
            created_at: snapshot.created_at,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDraftSnapshotRef {
    pub draft_id: String,
    pub snapshot_id: String,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowDraftSave {
    Saved {
        draft: WorkflowDraft,
        snapshot: WorkflowDraftSnapshot,
    },
    Conflict {
        draft: WorkflowDraft,
        snapshot: WorkflowDraftSnapshot,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDraftPage {
    pub items: Vec<WorkflowDraft>,
    pub total: i64,
    pub next_offset: Option<u32>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDraftSnapshotPage {
    pub items: Vec<WorkflowDraftSnapshotSummary>,
    pub total: i64,
    pub next_offset: Option<u32>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowOperationRequest {
    Create {
        request: CreateWorkflow,
    },
    Publish {
        workflow_id: String,
        request: PublishWorkflow,
    },
    Revoke {
        workflow_id: String,
        request: RevokeWorkflow,
    },
    Start {
        request: Box<WorkflowRunRequest>,
    },
    ResumeInput {
        request: ResumeWorkflowInput,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrepareWorkflowOperation {
    pub intent_id: String,
    pub request: WorkflowOperationRequest,
    #[serde(default)]
    pub draft_snapshot: Option<WorkflowDraftSnapshotRef>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowOperationState {
    Pending,
    Accepted,
    Rejected,
    Discarded,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowOperationReceipt {
    Created {
        publication: WorkflowPublication,
    },
    Published {
        publication: WorkflowPublication,
    },
    Revoked {
        publication: WorkflowPublication,
    },
    /// The accepted configuration is a frozen receipt, not the mutable current state.
    Started {
        run_id: String,
        spec: Box<RunSpec>,
        created_at: i64,
    },
    /// Acceptance queues one generation; it does not confirm a human answer.
    InputResumeQueued {
        run_id: String,
        resume_id: String,
        generation: u64,
        accepted_run_revision: i64,
        accepted_workflow_revision: u64,
        cursor_sha256: String,
        confirmed_sequence: i64,
        accepted_at: i64,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowOperationRejection {
    pub code: String,
    pub message: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowOperation {
    pub id: String,
    pub project_id: String,
    pub intent_id: String,
    pub request_sha256: String,
    pub request: WorkflowOperationRequest,
    pub draft_snapshot: Option<WorkflowDraftSnapshotRef>,
    pub state: WorkflowOperationState,
    pub receipt: Option<WorkflowOperationReceipt>,
    pub rejection: Option<WorkflowOperationRejection>,
    pub created_at: i64,
    pub updated_at: i64,
    pub acknowledged_at: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowOperationPage {
    pub items: Vec<WorkflowOperation>,
    pub total: i64,
    pub next_offset: Option<u32>,
}
