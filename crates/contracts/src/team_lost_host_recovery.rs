//! Lost host closure requires a pre-enrolled controller and original allocation; it never settles old effects.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TeamOriginalHostBinding {
    pub format: u32,
    pub host_id: Uuid,
    pub engine_id: String,
    pub node_name: Option<String>,
    pub node_uid: Option<Uuid>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamHostTrustProfileView {
    pub id: Uuid,
    pub team_id: Uuid,
    pub binding: TeamOriginalHostBinding,
    pub profile_sha256: String,
    pub public_key_sha256: String,
    pub revision: i64,
    pub active: bool,
    pub inspected_at: i64,
    pub activated_at: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamLostHostPlan {
    pub run_id: Uuid,
    pub order_id: Uuid,
    pub team_id: Uuid,
    pub nonce: Uuid,
    pub container_id: Option<String>,
    pub container_name: String,
    pub original_record_sha256: String,
    pub original_record_source: String,
    pub profile_id: Uuid,
    pub original_profile_sha256: String,
    pub intent: Value,
    pub intent_sha256: String,
    pub current_revision: i64,
    pub plan_sha256: String,
    pub affected_runs: Vec<Uuid>,
    pub original_runtime_count: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrepareTeamLostHostRetirement {
    pub operation_id: Uuid,
    pub nonce: Uuid,
    pub expected_revision: i64,
    pub original_plan_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReceiveTeamLostHostRetirement {
    pub operation_id: Uuid,
    pub original_intent_sha256: String,
    pub witness: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamLostHostRetirementReceipt {
    pub operation_id: Uuid,
    pub run_id: Uuid,
    pub nonce: Uuid,
    pub original_record_sha256: String,
    pub original_profile_sha256: String,
    pub original_intent_sha256: String,
    pub witness_sha256: String,
    pub retirement_id: Uuid,
    pub host_retired: bool,
    pub container_removed: bool,
    pub effects_unknown: bool,
    pub workspace_promotion_allowed: bool,
}
