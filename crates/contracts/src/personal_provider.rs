//! New owner-authorized configuration; source imports contain no credential.
use super::*;

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigurePersonalProvider {
    pub profile: CreateProvider,
    pub idempotency_key: String,
    pub api_key: Option<String>,
    pub explicitly_allow_no_key: bool,
}
