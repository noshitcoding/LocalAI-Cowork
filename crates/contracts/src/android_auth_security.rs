//! Additive original V1 wire contracts. They authorize no identity or factor.
use super::*;
use uuid::Uuid;
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidPasswordCredentials {
    pub email: String,
    pub password: String,
    pub device_id: String,
    #[serde(default)]
    pub second_factor: Option<String>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidAuthTokens {
    pub schema_version: u16,
    pub access_token: String,
    pub access_expires_at: String,
    pub refresh_token: String,
    pub refresh_expires_at: String,
    pub user_id: Uuid,
    pub session_id: Uuid,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidAuthenticationAssurance {
    pub required: bool,
    pub totp_enabled: bool,
    pub passkey_registered: bool,
    pub session_verified: bool,
    pub enrollment_allowed: bool,
    pub reauthentication_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_verified: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_verification_expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oidc_organisation_id: Option<Uuid>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidAuthSession {
    pub schema_version: u16,
    pub id: Uuid,
    pub device_id: Uuid,
    pub current: bool,
    pub active: bool,
    pub created_at: String,
    pub last_used_at: String,
    pub expires_at: String,
    pub revoked_at: Option<String>,
    pub revoke_reason: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidTotpStatus {
    pub schema_version: u16,
    pub enabled: bool,
    pub unused_recovery_codes: u16,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidTotpSetup {
    pub schema_version: u16,
    pub secret: String,
    pub otpauth_uri: String,
    pub expires_at: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidTotpCode {
    pub code: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidDisableTotp {
    pub password: String,
    pub second_factor: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidRecoveryCodes {
    pub schema_version: u16,
    pub recovery_codes: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidCreateInvitation {
    pub email: String,
    #[serde(default)]
    pub expires_at: Option<String>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidInvitationSecret {
    pub schema_version: u16,
    pub invitation_id: Uuid,
    pub email: String,
    pub token: String,
    pub expires_at: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidAcceptInvitation {
    pub token: String,
    pub display_name: String,
    pub password: String,
    pub device_id: Uuid,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidCreateTeam {
    pub name: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidSetTeamMember {
    pub user_id: Uuid,
    pub role: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidTeamMember {
    pub schema_version: u16,
    pub team_id: Uuid,
    pub user_id: Uuid,
    pub role: String,
    pub email: String,
    pub display_name: String,
    pub created_at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidTeam {
    pub schema_version: u16,
    pub id: Uuid,
    pub revision: i64,
    pub etag: String,
    pub name: String,
    pub owner_user_id: Uuid,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}
