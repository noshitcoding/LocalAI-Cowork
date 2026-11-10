//! Schema-two Android and explicitly paired device metadata. Secrets are input-only.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeviceCapability {
    pub schema_version: u16,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub attributes: std::collections::BTreeMap<String, Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeviceExecutorRegistration {
    pub schema_version: u16,
    pub executor_id: Uuid,
    pub kind: String,
    pub pool_id: Option<Uuid>,
    pub owner_user_id: Option<Uuid>,
    pub display_name: String,
    pub protocol_version: u16,
    pub capabilities: Vec<DeviceCapability>,
    #[serde(default)]
    pub labels: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub personal_device_remote_control: Option<String>,
    pub max_concurrent_runs: u16,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PairDeviceExecutor {
    pub registration: DeviceExecutorRegistration,
    pub team_id: Option<Uuid>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateDeviceExecutor {
    pub expected_revision: i64,
    pub draining: bool,
    pub revoked: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DevicePasskeyRecord {
    pub schema_version: u16,
    pub id: Uuid,
    pub label: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DevicePasskeyChallenge {
    pub schema_version: u16,
    pub challenge_id: Uuid,
    pub public_key: Value,
    pub expires_at: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FinishDevicePasskeyRegistration {
    pub challenge_id: Uuid,
    pub label: String,
    pub credential: Value,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StartDevicePasskeyAuthentication {
    pub email: String,
    pub device_id: Uuid,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FinishDevicePasskeyAuthentication {
    pub challenge_id: Uuid,
    pub credential: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DevicePushSubscription {
    pub schema_version: u16,
    pub id: Uuid,
    pub device_id: Uuid,
    pub provider: String,
    pub created_at: String,
    pub last_success_at: Option<String>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RegisterDevicePush {
    pub device_id: Uuid,
    pub provider: String,
    pub token: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeviceFeatureConfiguration {
    pub schema_version: u16,
    pub passkeys_enabled: bool,
    pub rp_id: Option<String>,
    pub origin: Option<String>,
    pub fcm_enabled: bool,
    pub fleet_enabled: bool,
    pub push_delivery: String,
    pub minimum_compatible_schema_version: u16,
    pub agent_protocol_version: u16,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeviceCredentialMetadata {
    pub id: Uuid,
    pub executor_id: Uuid,
    pub label: String,
    pub expires_at: String,
    pub revoked: bool,
    pub revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DevicePairingRecord {
    pub schema_version: u16,
    pub registration: Value,
    pub team_id: Option<Uuid>,
    pub revoked: bool,
    pub online: bool,
    pub draining: bool,
    pub active_runs: u16,
    pub last_seen_at: String,
    pub revision: i64,
}
