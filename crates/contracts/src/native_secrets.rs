use super::*;
/// Contains no native key locator or secret material.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeSecretsStatus {
    pub revision: i64,
    pub configured: bool,
    pub key_available: bool,
    pub pending_operation_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeSecretsSetup {
    pub idempotency_key: String,
    pub base_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeSecretsResolution {
    pub operation_id: String,
    pub base_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NativeSecretsSetupState {
    Pending,
    Activated,
    Abandoned,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeSecretsReceipt {
    pub operation_id: String,
    pub operation_state: NativeSecretsSetupState,
    pub store: NativeSecretsStatus,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeSecretsRequestLookup {
    pub receipt: Option<NativeSecretsReceipt>,
}
