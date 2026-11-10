//! Reviewed execution intent. Callers submit a request, never a resolved RunSpec.
use super::*;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionProcessAccess {
    BrokerOnly,
    UserRights,
    QualifiedEnvironment,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutionNetworkAccess {
    NoProcess,
    UserRights,
    Denied,
    WebProxy { policy_id: String },
}

/// Process networking does not describe model or connector data transmission.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionBoundary {
    pub executor: String,
    pub process_access: ExecutionProcessAccess,
    pub process_network: ExecutionNetworkAccess,
    pub environment: Option<ExecutionEnvironmentRef>,
    pub folders: Vec<FolderGrant>,
    pub connector_aliases: Vec<String>,
    /// False until an immutable input snapshot, rather than live folder grants,
    /// is actually bound by the selected adapter. Never inferred from a cwd.
    pub input_snapshot_bound: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionPlan {
    pub format_version: u32,
    pub id: String,
    /// This owner API is authenticated by the installation's owner authority.
    /// Team/device adapters must supply their own authenticated actor binding.
    pub actor: String,
    pub project_id: String,
    pub order_id: String,
    pub idempotency_key: String,
    pub request_sha256: String,
    pub spec_sha256: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub preflight: RunPreflight,
    pub boundary: ExecutionBoundary,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreparedExecutionPlan {
    pub plan: ExecutionPlan,
    pub sha256: String,
    /// Receipt metadata is outside the immutable plan hash.
    pub committed_run_id: Option<String>,
    pub retired: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommitExecutionPlan {
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RetiredExecutionRequest {
    pub order_id: String,
    pub idempotency_key: String,
    pub request_sha256: String,
    pub retired: bool,
}
