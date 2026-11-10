//! Public runner identity and registry provenance; never a private key or a
//! platform qualification. Only the trusted operator path writes registrations.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRunnerIdentity {
    pub runner_id: String,
    pub generation: u64,
    pub adapter_build_sha256: String,
    pub control_public_key: Vec<u8>,
    pub runner_public_key: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionTargetState {
    Registered,
    Active,
    Revoked,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionTargetRef {
    pub target_id: String,
    pub revision: i64,
    pub identity_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionTarget {
    pub format_version: u32,
    pub target_id: String,
    pub revision: i64,
    pub state: ExecutionTargetState,
    pub identity: ExecutionRunnerIdentity,
    pub identity_sha256: String,
    pub created_at: i64,
    pub updated_at: i64,
}
