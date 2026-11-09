//! Actual account-owned skill and memory content; no executor or credential grants.
use super::*;
fn enabled() -> bool {
    true
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActorSkillDefinition {
    pub project_id: Option<String>,
    pub name: String,
    pub description: String,
    pub prompt_template: String,
    pub trigger_pattern: Option<String>,
    #[serde(default)]
    pub run_mode: AgentExecutionMode,
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub auto_learn: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActorMemoryDefinition {
    pub project_id: Option<String>,
    pub key: String,
    pub content: String,
    pub category: String,
    pub confidence: Option<f64>,
    #[serde(default = "enabled")]
    pub enabled: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteActorSkill {
    pub request_id: uuid::Uuid,
    pub expected_revision: i64,
    pub value: ActorSkillDefinition,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteActorMemory {
    pub request_id: uuid::Uuid,
    pub expected_revision: i64,
    pub value: ActorMemoryDefinition,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteActorContext {
    pub request_id: uuid::Uuid,
    pub expected_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActorContextRecord {
    pub id: uuid::Uuid,
    pub revision: i64,
    pub etag: String,
    pub deleted: bool,
    pub project_id: Option<uuid::Uuid>,
    pub value: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActorSkillLearningOutcome {
    pub run_id: uuid::Uuid,
    pub project_id: uuid::Uuid,
    pub skill_id: uuid::Uuid,
    pub skill_revision: i64,
    pub node_ids: Vec<String>,
    pub recorded_at: i64,
    pub state: String,
    pub reason: String,
    pub observed_tools: Vec<String>,
    pub source_definition: Value,
    pub proposed_prompt_template: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActorSkillLearningHistory {
    pub items: Vec<ActorSkillLearningOutcome>,
}
