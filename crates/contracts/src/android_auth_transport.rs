//! Additive authentication transport and installation policy contracts.
use super::*;
use uuid::Uuid;
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidAuthManagementPolicy {
    pub schema_version: u16,
    pub revision: i64,
    pub admin_required: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetAndroidAuthManagementPolicy {
    pub operation_id: Uuid,
    pub expected_revision: i64,
    pub admin_required: bool,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidNativeAuthorization {
    pub email: String,
    pub password: String,
    pub device_id: String,
    #[serde(default)]
    pub second_factor: Option<String>,
    pub code_challenge: String,
    pub code_challenge_method: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidNativeAuthorizationCode {
    pub schema_version: u16,
    pub code: String,
    pub expires_at: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidNativeTokenExchange {
    pub code: String,
    pub code_verifier: String,
    pub device_id: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidRefreshSession {
    pub refresh_token: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidBrowserAuthTokens {
    pub schema_version: u16,
    pub access_token: String,
    pub access_expires_at: String,
    pub refresh_expires_at: String,
    pub user_id: Uuid,
    pub session_id: Uuid,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidAuthSecurityContext {
    pub schema_version: u16,
    pub user_id: Uuid,
    pub session_id: Uuid,
}
/// Only the paired local native bridge transports this credential-bearing request.
#[derive(Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeAuthTransportRequest {
    pub operation_id: Uuid,
    pub endpoint: String,
    pub purpose: String,
    pub body_json: String,
    pub actor_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeAuthTransportView {
    pub operation_id: Uuid,
    pub endpoint: String,
    pub purpose: String,
    pub state: String,
    pub actor_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeAuthTransportReplay {
    pub request: NativeAuthTransportRequest,
    pub response_json: Option<String>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeAuthTransportCapture {
    pub operation_id: Uuid,
    pub response_json: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeAuthTransportResolution {
    pub operation_id: Uuid,
    pub acknowledged: bool,
}
