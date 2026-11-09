//! Owner-visible local result metadata. Native custody and execution grants stay private.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OwnerManagedResultList {
    /// Complete for this run, bounded to 128 attempts. Larger runs return an error.
    pub entries: Vec<OwnerManagedResultEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OwnerManagedResultEntry {
    pub order_id: String,
    pub run_id: String,
    pub process_attempt_id: String,
    /// None means no local reception intent, not a promise of remote availability.
    pub version: Option<OwnerManagedResultVersion>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OwnerManagedResultVersionState {
    Receiving,
    Ready,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OwnerManagedResultVersion {
    pub id: String,
    pub revision: i64,
    pub state: OwnerManagedResultVersionState,
    pub created_at: i64,
    pub manifest_sha256: String,
    pub file_count: u64,
    pub total_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OwnerManagedResultManifest {
    pub entry: OwnerManagedResultEntry,
    pub manifest: WorkspaceTransferManifest,
}

/// A private native acknowledgement is projected only as scoped display data.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OwnerManagedResultDeliveryView {
    pub order_id: String,
    pub run_id: String,
    pub process_attempt_id: String,
    pub delivery: Option<OwnerManagedResultDelivery>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OwnerManagedResultDeliveryState {
    Prepared,
    Committed,
    Expired,
    Accepted,
    Retired,
    Unconfirmed,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OwnerManagedResultDelivery {
    pub id: String,
    pub result_version_id: String,
    pub request_sha256: String,
    pub manifest_sha256: String,
    pub state: OwnerManagedResultDeliveryState,
    pub created_at: i64,
    pub accept_before: i64,
    pub revision: i64,
    pub accepted_at: Option<i64>,
    pub delivery_receipt_sha256: Option<String>,
    pub retirement_revision: Option<u64>,
    pub retirement_recorded_at: Option<i64>,
    pub retirement_receipt_sha256: Option<String>,
}

/// Confirms exactly the intent previously displayed by the controller.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConfirmOwnerManagedResultDelivery {
    pub id: String,
    pub request_sha256: String,
}
