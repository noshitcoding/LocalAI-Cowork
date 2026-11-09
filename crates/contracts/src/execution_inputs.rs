//! Immutable local preparation. These records grant neither transfer nor execution.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PrepareExecutionInput {
    pub idempotency_key: String,
    pub base_revision: i64,
    /// Existing local folder capabilities, resolved by the broker. No host path.
    pub folder_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionInputSource {
    pub alias: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionInputSnapshot {
    pub format_version: u32,
    pub id: String,
    pub order_id: String,
    pub project_id: String,
    pub order_revision: i64,
    pub request_sha256: String,
    pub created_at: i64,
    pub sources: Vec<ExecutionInputSource>,
    pub manifest: WorkspaceTransferManifest,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PreparedExecutionInput {
    pub snapshot: ExecutionInputSnapshot,
    pub sha256: String,
    pub retired: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RetireExecutionInput {
    pub sha256: String,
}

/// An immutable content reference, never a transfer or execution credential.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionInputRef {
    pub snapshot_id: String,
    pub sha256: String,
    pub manifest_sha256: String,
}

/// Logical namespace of a run's private working copies. Native directory IDs
/// and materialization receipts remain in local private custody.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RunWorkspaceBinding {
    pub id: String,
    pub input: ExecutionInputRef,
}
