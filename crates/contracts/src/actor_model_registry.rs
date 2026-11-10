//! Qualification metadata for actual identity-owned private projects and profiles.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActorModelContext {
    pub actor_id: String,
    pub project_id: String,
    pub project_revision: i64,
    pub provider_binding: Value,
    pub provider: ProviderProfile,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActorModelRegistryView {
    pub context: ActorModelContext,
    pub qualification: ModelRegistryView,
    pub source_sha256: String,
    pub request_sha256: String,
}

/// Native-only operator import. Actor credentials are transient headers, never fields.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportActorModelQualification {
    pub idempotency_key: String,
    pub context: ActorModelContext,
    pub source_sha256: String,
    pub qualification_bundle: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RevokeActorModelQualification {
    pub actor_id: String,
    pub project_id: String,
    pub binding: ModelRegistryRef,
}
