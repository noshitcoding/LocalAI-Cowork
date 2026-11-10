//! OIDC content and original-operation DTOs contain no caller-asserted subject claims.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AndroidOidcConfig {
    pub issuer: String,
    pub client_id: String,
    pub redirect_uris: Vec<String>,
    pub allow_signup: bool,
    #[serde(default)]
    pub endpoint_origins: Vec<String>,
    #[serde(default = "oidc_groups_claim")]
    pub groups_claim: String,
}
fn oidc_groups_claim() -> String {
    "groups".into()
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AndroidOidcGroupMapping {
    pub group: String,
    pub role: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidOidcSsoProvider {
    pub id: String,
    pub label: String,
    pub issuer: String,
    pub groups_claim: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidOidcSsoSettings {
    pub organisation_id: Uuid,
    pub revision: i64,
    pub enabled: bool,
    pub available: bool,
    pub provider_id: Option<String>,
    pub auto_provision: bool,
    pub group_mappings: Vec<AndroidOidcGroupMapping>,
    pub providers: Vec<AndroidOidcSsoProvider>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AndroidOidcSsoWrite {
    pub expected_revision: i64,
    pub enabled: bool,
    pub provider_id: Option<String>,
    pub auto_provision: bool,
    pub group_mappings: Vec<AndroidOidcGroupMapping>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidOidcMembershipOrigin {
    pub membership_id: Uuid,
    pub organisation_id: Uuid,
    pub identity_id: Uuid,
    pub profile_id: Uuid,
    pub external_identity_id: Uuid,
    pub flow_id: Uuid,
    pub claims_sha256: String,
    pub groups_sha256: String,
    pub settings_revision: i64,
    pub membership_revision: i64,
    pub role: String,
    pub active: bool,
    pub expires_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidOidcProfileWrite {
    pub request_id: Uuid,
    pub expected_revision: i64,
    pub organization_id: Option<Uuid>,
    pub name: String,
    pub config: AndroidOidcConfig,
    pub client_secret: Option<String>,
    pub active: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidOidcProfile {
    pub id: Uuid,
    pub organization_id: Option<Uuid>,
    pub name: String,
    pub revision: i64,
    pub epoch: Uuid,
    pub config_sha256: String,
    pub config: AndroidOidcConfig,
    pub active: bool,
    pub has_secret: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidOidcBegin {
    pub request_id: Uuid,
    pub profile_id: Uuid,
    pub device_id: String,
    pub redirect_uri: String,
    pub purpose: String,
    pub resume_secret: String,
    #[serde(default)]
    pub native_code_challenge: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidOidcFlow {
    pub id: Uuid,
    pub profile_id: Uuid,
    pub profile_revision: i64,
    pub purpose: String,
    pub authorization_url: String,
    pub expires_at: i64,
    pub phase: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidOidcComplete {
    pub flow_id: Uuid,
    pub resume_secret: String,
    pub state: String,
    pub code: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidOidcRecover {
    pub flow_id: Uuid,
    pub resume_secret: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidOidcFinish {
    pub request_id: Uuid,
    pub flow: AndroidOidcComplete,
    pub device_id: String,
    #[serde(default)]
    pub native_code_challenge: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidOidcRetire {
    pub request_id: Uuid,
    pub flow_id: Uuid,
    pub resume_secret: String,
}
/// Existing V1 Android/Web start request; outer PKCE is separate from SDK PKCE.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidOidcCompatStart {
    pub device_id: String,
    pub code_challenge: String,
    pub client_state: String,
    pub redirect_uri: String,
    pub code_challenge_method: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidOidcLink {
    pub id: Uuid,
    pub profile_id: Uuid,
    pub issuer: String,
    pub subject: String,
    pub revision: i64,
    pub active: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidOidcUnlink {
    pub request_id: Uuid,
    pub expected_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidOidcOrigin {
    pub method: String,
    pub profile_id: Uuid,
    pub profile_revision: i64,
    pub organization_id: Option<Uuid>,
    pub external_identity_id: Uuid,
    pub issuer: String,
    pub verified_at: i64,
    pub expires_at: i64,
    pub claims_sha256: String,
    pub flow_id: Uuid,
}
