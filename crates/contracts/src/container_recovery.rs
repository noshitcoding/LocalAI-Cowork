use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TeamContainerRecoveryPlan {
    pub nonce: Uuid,
    pub team_id: Uuid,
    pub order_id: Uuid,
    pub run_id: Uuid,
    pub revision: i64,
    pub record_format: u32,
    pub record_sha256: String,
    pub container_id: String,
    pub host_id: Uuid,
    pub engine_id: String,
    pub node_name: Option<String>,
    pub node_uid: Option<Uuid>,
    /// An inspected current location, never a claim about an unknown old host.
    pub legacy_location_adoption: bool,
    pub container_present: bool,
    pub running: bool,
    pub paused: bool,
    pub plan_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RetireTeamContainer {
    pub idempotency_key: Uuid,
    pub expected_revision: i64,
    pub nonce: Uuid,
    pub plan_sha256: String,
    pub allow_legacy_location_adoption: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamContainerRecoveryReceipt {
    pub operation_id: Uuid,
    pub run_id: Uuid,
    pub nonce: Uuid,
    pub revision: i64,
    pub request_sha256: String,
    pub plan_sha256: String,
    pub record_sha256: String,
    pub container_id: String,
    pub host_id: Uuid,
    pub engine_id: String,
    pub legacy_location_adoption: bool,
    pub container_removed: bool,
    /// Retirement does not prove what the old tool did before it stopped.
    pub effects_unknown: bool,
}
