//! Actor-owned composer copies. Library references are provenance, never authority.
use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActorRunConfiguration {
    pub agent_config: EffectiveAgentConfig,
    pub access_mode: AccessMode,
    pub budgets: Budgets,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActorChatRunConfiguration {
    pub model_profile_id: String,
    pub provider_revision: i64,
    pub run_configuration: ActorRunConfiguration,
    #[serde(default)]
    pub skill_ids: Vec<String>,
    #[serde(default)]
    pub memory_ids: Vec<String>,
    #[serde(default)]
    pub mcp_metadata_ids: Vec<String>,
    #[serde(default)]
    pub expected_mcp_revisions: BTreeMap<String, i64>,
    pub model_binding: Option<ModelRegistryRef>,
    pub output: Option<StructuredOutput>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActorChatConfiguration {
    pub thread_id: String,
    pub project_id: String,
    pub revision: i64,
    pub updated_at: i64,
    pub value: Option<ActorChatRunConfiguration>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteActorChatConfiguration {
    pub expected_revision: i64,
    pub idempotency_key: String,
    pub value: ActorChatRunConfiguration,
}
