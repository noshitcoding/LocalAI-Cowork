use super::*;
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduledRunTemplate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_config: Option<EffectiveAgentConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<WorkflowRunRequest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_tools: Vec<McpToolSelection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<FolderGrant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<ExecutionEnvironmentRef>,
    #[serde(default = "schedule_read_only")]
    pub access_mode: AccessMode,
    pub order_id: String,
    pub prompt: String,
    pub provider_id: String,
    pub provider_revision: i64,
    #[serde(default)]
    pub budgets: Budgets,
}
fn schedule_read_only() -> AccessMode {
    AccessMode::ReadOnly
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProductSchedule {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub timezone: String,
    pub recurrence: ScheduleRecurrence,
    pub template: ScheduledRunTemplate,
    pub paused: bool,
    pub next_run: Option<i64>,
    pub active_occurrence: Option<ScheduleOccurrence>,
    pub last_run_id: Option<String>,
    pub last_error: Option<String>,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteSchedule {
    pub id: Option<String>,
    pub expected_revision: i64,
    pub name: String,
    pub timezone: String,
    pub recurrence: ScheduleRecurrence,
    pub template: ScheduledRunTemplate,
    #[serde(default)]
    pub paused: bool,
}
/// Original occurrence content is evidence; returning it never dispatches it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleOccurrence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_request: Option<WorkflowRunRequest>,
    pub id: String,
    pub due_at: i64,
    pub claimed_at: i64,
    pub request: CreateRun,
    pub run_id: Option<String>,
    pub uncertain: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeScheduleHistoryEntry {
    pub sequence: i64,
    pub recorded_at: i64,
    pub schedule_revision: Option<i64>,
    pub deleted: bool,
    pub paused: Option<bool>,
    pub next_run: Option<i64>,
    pub occurrence: Option<ScheduleOccurrence>,
    pub run: Option<RunRecord>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeScheduleHistory {
    pub items: Vec<NativeScheduleHistoryEntry>,
    pub next_cursor: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeDeletedSchedule {
    pub id: String,
    pub name: String,
}
