use super::*;
impl GenerationSettings {
    pub fn is_empty(&self) -> bool {
        self.temperature.is_none() && self.timeout_ms.is_none()
    }
}

/// What the configured model accepts besides text. Frozen into each run with its profile.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ModelCapabilities {
    /// The model accepts images (vision); document previews and edited images are shown to it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub image_input: bool,
}
impl ModelCapabilities {
    pub fn is_empty(&self) -> bool {
        !self.image_input
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderSettings {
    pub profile: ProviderProfile,
    #[serde(default)]
    pub generation: GenerationSettings,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateProviderSettings {
    pub idempotency_key: String,
    pub expected_revision: i64,
    pub profile: CreateProvider,
    #[serde(default)]
    pub generation: GenerationSettings,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub remove_key: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteProviderSettings {
    pub idempotency_key: String,
    pub expected_revision: i64,
}

/// Read-only catalogue lookup before a provider or model has been saved.
/// Credentials are used for this request only, never persisted or returned.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiscoverProviderModels {
    pub kind: ProviderKind,
    pub endpoint: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub explicitly_allow_no_key: bool,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ProviderSettingsContract {
    pub settings: ProviderSettings,
    pub update: UpdateProviderSettings,
    pub delete: DeleteProviderSettings,
    pub discover: DiscoverProviderModels,
}
