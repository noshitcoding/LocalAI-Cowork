//! V1-compatible private schedules and additive original occurrence history.
use super::*;
use uuid::Uuid;

fn empty_schedule_input() -> Value {
    serde_json::json!({})
}
fn schedule_enabled() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidScheduleRecord {
    pub schema_version: u16,
    pub id: Uuid,
    pub revision: i64,
    pub etag: String,
    pub task_id: Uuid,
    pub project_id: Uuid,
    pub thread_id: Uuid,
    pub cron: String,
    pub timezone: String,
    pub executor_target: Value,
    pub input: Value,
    pub model_profile_id: Option<Uuid>,
    pub enabled: bool,
    pub next_run_at: Option<String>,
    pub last_triggered_at: Option<String>,
    pub blocked_reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidCreateSchedule {
    pub task_id: Uuid,
    pub project_id: Uuid,
    pub thread_id: Uuid,
    pub cron: String,
    pub timezone: String,
    pub executor_target: Value,
    #[serde(default = "empty_schedule_input")]
    pub input: Value,
    pub model_profile_id: Option<Uuid>,
    #[serde(default = "schedule_enabled")]
    pub enabled: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidCreateThreadSchedule {
    pub task_id: Uuid,
    pub project_id: Uuid,
    pub title: String,
    pub cron: String,
    pub timezone: String,
    pub executor_target: Value,
    #[serde(default = "empty_schedule_input")]
    pub input: Value,
    pub model_profile_id: Option<Uuid>,
    #[serde(default = "schedule_enabled")]
    pub enabled: bool,
    pub idempotency_key: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidUpdateSchedule {
    pub expected_revision: i64,
    pub cron: String,
    pub timezone: String,
    pub executor_target: Value,
    #[serde(default = "empty_schedule_input")]
    pub input: Value,
    pub model_profile_id: Option<Uuid>,
    pub enabled: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidThreadSchedule {
    pub schema_version: u16,
    pub thread: Value,
    pub schedule: AndroidScheduleRecord,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidScheduleOccurrence {
    pub id: Uuid,
    pub schedule_id: Uuid,
    pub run_id: Uuid,
    pub sequence: i64,
    pub executor_target: Value,
    pub due_at: String,
    pub triggered_at: String,
    pub status: String,
    pub error: Option<String>,
    pub completed_at: Option<String>,
    pub missed_occurrences: u32,
    pub catch_up_truncated: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidScheduleHistory {
    pub occurrences: Vec<AndroidScheduleOccurrence>,
    pub next_cursor: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidSchedulePreview {
    pub timezone: String,
    pub cron: String,
    pub next_occurrences: Vec<String>,
    pub ambiguous_time_policy: String,
    pub nonexistent_time_policy: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidScheduleOriginal {
    pub purpose: String,
    pub request_sha256: String,
    pub receipt: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidCreateScheduleTask {
    pub project_id: Uuid,
    pub name: String,
    pub instructions: String,
    #[serde(default)]
    pub required_capabilities: Vec<String>,
    pub default_target: Option<Value>,
    #[serde(default = "empty_schedule_input")]
    pub config: Value,
    #[serde(default)]
    pub release: bool,
}

/// Finalization uses an observed server evidence digest, never caller-supplied stop evidence.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidScheduleResolutionRequest {
    pub expected_revision: i64,
    pub evidence_sha256: String,
    pub action: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidScheduleResolutionView {
    pub schema_version: u16,
    pub schedule_id: Uuid,
    pub occurrence_id: Uuid,
    pub run_id: Uuid,
    pub grant_id: Uuid,
    pub original_creation_key: String,
    pub original_request_sha256: String,
    pub original_spec_sha256: String,
    pub family: String,
    pub occurrence_status: String,
    pub run_state: String,
    pub schedule_revision: i64,
    pub evidence_sha256: String,
    pub dispatch_sha256: String,
    pub terminal_receipt_sha256: Option<String>,
    pub physical_stop_verified: bool,
    pub physical_quiescence_verified: bool,
    pub never_created_guest: bool,
    pub never_dispatched: bool,
    pub unresolved_effects: bool,
    pub can_resolve: bool,
    pub can_retire: bool,
    pub block_reason: Option<String>,
    pub schedule_retired: bool,
    pub original_resolution: Option<AndroidScheduleResolutionReceipt>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidScheduleResolutionReceipt {
    pub schema_version: u16,
    pub id: Uuid,
    pub schedule_id: Uuid,
    pub occurrence_id: Uuid,
    pub run_id: Uuid,
    pub grant_id: Uuid,
    pub original_creation_key: String,
    pub original_request_sha256: String,
    pub original_spec_sha256: String,
    pub evidence_sha256: String,
    pub dispatch_sha256: String,
    pub terminal_receipt_sha256: Option<String>,
    pub action: String,
    pub outcome: String,
    pub unresolved_effects: bool,
    pub schedule_retired: bool,
    pub finalized_at: String,
    pub schedule: AndroidScheduleRecord,
}
