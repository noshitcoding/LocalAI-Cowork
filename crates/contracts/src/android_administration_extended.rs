//! Explicit current administration requests; historic/cache bytes are never execution authority.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidMobileCacheBegin {
    pub request_id: Uuid,
    pub archive_size: u64,
    pub archive_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidAdministrationRevision {
    pub expected_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidMobileCacheActivate {
    pub request_id: Uuid,
    pub expected_revision: i64,
    pub thread_id: Uuid,
    pub ordinals: Vec<i64>,
    pub remainder_history_only_reason: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidSupportGrantWrite {
    pub project_id: Uuid,
    pub user_id: Uuid,
    pub expected_revision: i64,
    pub expires_at: i64,
    pub reason: String,
    pub request_id: Uuid,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidSupportGrantRevoke {
    pub expected_revision: i64,
    pub request_id: Uuid,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidPersonalStorageQuotaWrite {
    pub expected_revision: i64,
    pub limit_bytes: i64,
    pub request_id: Uuid,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidExecutorPoolWrite {
    pub organization_id: Uuid,
    pub id: Uuid,
    pub expected_revision: i64,
    pub name: String,
    pub active: bool,
    pub policy: serde_json::Value,
    pub request_id: Uuid,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidExecutorPoolProjectWrite {
    pub organization_id: Uuid,
    pub project_id: Uuid,
    pub expected_revision: i64,
    pub active: bool,
    pub request_id: Uuid,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidExecutorSyncGrantWrite {
    pub project_id: Uuid,
    pub expected_revision: i64,
    pub write: bool,
    pub active: bool,
    pub expires_at: i64,
    pub request_id: Uuid,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidWireSyncChange {
    pub schema_version: i64,
    pub operation_id: Uuid,
    pub device_id: Uuid,
    pub entity_type: String,
    pub entity_id: Uuid,
    pub base_revision: i64,
    pub operation: String,
    pub payload: Option<serde_json::Value>,
    pub client_timestamp: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidWireSyncBatch {
    pub changes: Vec<AndroidWireSyncChange>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidExecutorSyncBatch {
    pub project_id: Uuid,
    pub changes: Vec<AndroidWireSyncChange>,
}
