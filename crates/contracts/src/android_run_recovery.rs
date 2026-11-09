//! Public V1 checkpoint parity and explicit current server-agent continuation.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidRunCheckpoint {
    pub schema_version: u16,
    pub id: Uuid,
    pub run_id: Uuid,
    pub sequence: i64,
    pub safe_to_resume: bool,
    pub executor_state: Value,
    pub created_at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidRunCheckpointDescriptor {
    pub id: Uuid,
    pub run_id: Uuid,
    pub sequence: i64,
    pub origin: String,
    pub safe_to_resume: bool,
    pub state_sha256: String,
    pub state_size: u64,
    pub state_pages: u32,
    pub state_page_sha256: Vec<String>,
    pub created_at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidRunCheckpointPage {
    pub run_id: Uuid,
    pub entries: Vec<AndroidRunCheckpointDescriptor>,
    pub next_sequence: Option<i64>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AndroidRunWaitingReason {
    PlatformCapacity,
    OrganisationCapacity,
    UserCapacity,
    InstanceCapacity,
    ChatWorkspaceBusy,
    AdmissionUnavailable,
    RunnerCapacity,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidRunWaiting {
    pub run_id: Uuid,
    pub reason: Option<AndroidRunWaitingReason>,
    pub checked_at: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidResumeRun {
    pub request_id: Uuid,
    pub expected_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidRunResumeReceipt {
    pub request_id: Uuid,
    pub run_id: Uuid,
    pub checkpoint_id: Uuid,
    pub queued_revision: i64,
}
