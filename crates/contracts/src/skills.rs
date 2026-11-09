//! Project skills are contextual instructions and never execution permissions.
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentExecutionMode {
    #[default]
    Execute,
    Plan,
    Hybrid,
}
pub fn execute_mode(value: &AgentExecutionMode) -> bool {
    *value == AgentExecutionMode::Execute
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeSkillRevision {
    pub sequence: i64,
    pub recorded_at: i64,
    pub deleted: bool,
    pub skill: Option<Skill>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeSkillHistory {
    pub items: Vec<NativeSkillRevision>,
    pub next_cursor: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Skill {
    #[serde(default, skip_serializing_if = "execute_mode")]
    pub run_mode: AgentExecutionMode,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_learn: bool,
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub description: String,
    pub instructions: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_template: Option<String>,
    pub enabled: bool,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteSkill {
    #[serde(default, skip_serializing_if = "execute_mode")]
    pub run_mode: AgentExecutionMode,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_learn: bool,
    pub id: Option<String>,
    pub expected_revision: i64,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub instructions: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_template: Option<String>,
    #[serde(default = "enabled")]
    pub enabled: bool,
}
fn enabled() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillLearningOutcome {
    pub run_id: String,
    pub skill_id: String,
    pub skill_revision: i64,
    pub recorded_at: i64,
    pub state: RunState,
    pub reason: String,
    pub observed_tools: Vec<String>,
    pub proposed_instructions: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillLearningHistory {
    pub items: Vec<SkillLearningOutcome>,
}
