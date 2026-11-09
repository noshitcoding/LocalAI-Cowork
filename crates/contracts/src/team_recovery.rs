//! Team execution recovery preserves immutable original requests and receipts.
use crate::TeamRunStatus;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TeamEffectView {
    pub id: Uuid,
    pub run_id: Uuid,
    pub slot: String,
    pub attempt: i64,
    pub sequence: i64,
    pub kind: String,
    pub state: String,
    pub request_sha256: String,
    pub request_id: String,
    pub scope_sha256: String,
    pub provider_revision: i64,
    pub created_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TeamEffectDetail {
    pub effect: TeamEffectView,
    pub request: Value,
    pub outcome: Option<String>,
    pub source: Option<String>,
    pub result: Option<Value>,
    pub receipt: Option<Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TeamRecoveryView {
    pub run_id: Uuid,
    pub revision: i64,
    pub scope_sha256: String,
    pub status: TeamRunStatus,
    pub resume_allowed: bool,
    pub blocked_reason: Option<String>,
    pub effects: Vec<TeamEffectView>,
    pub next_cursor: i64,
    pub has_more: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResolveTeamEffect {
    pub idempotency_key: Uuid,
    pub expected_revision: i64,
    pub effect_id: Uuid,
    pub request_sha256: String,
    pub source: String,
    pub owner_verified: bool,
    pub outcome: String,
    pub receipt: Value,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResumeTeamRun {
    pub idempotency_key: Uuid,
    pub expected_revision: i64,
    pub acknowledge_cancelled: bool,
}
