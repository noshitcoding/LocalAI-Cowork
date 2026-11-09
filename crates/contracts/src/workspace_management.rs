//! Durable native workspace configuration. Source metadata never grants file access.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectSource {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub reference: String,
    pub enabled: bool,
    pub access: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectSettings {
    pub project_id: String,
    pub revision: i64,
    pub name: String,
    pub instructions: String,
    pub sources: Vec<ProjectSource>,
    pub working_folder: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteProjectSettings {
    pub expected_revision: i64,
    pub idempotency_key: String,
    pub name: String,
    pub instructions: String,
    pub sources: Vec<ProjectSource>,
    pub working_folder: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManageChat {
    pub expected_revision: i64,
    pub idempotency_key: String,
    pub title: String,
    pub project_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArchiveChat {
    pub expected_revision: i64,
    pub idempotency_key: String,
    pub archived: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatManagement {
    pub order: Order,
    pub archived: bool,
}
/// A copied composer configuration, before runtime workspace/project instructions.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatWorkflowSelection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<Box<serde_json::Value>>,
    pub workflow_id: String,
    pub version_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatRunConfiguration {
    pub agent_config: EffectiveAgentConfig,
    pub provider_id: Option<String>,
    pub provider_revision: Option<i64>,
    pub access_mode: AccessMode,
    pub budgets: Budgets,
    #[serde(default)]
    pub folders: Vec<FolderGrant>,
    #[serde(default)]
    pub native_tools: Vec<String>,
    #[serde(default)]
    pub mcp_tools: Vec<McpToolSelection>,
    pub environment: Option<ExecutionEnvironmentRef>,
    pub model_binding: Option<ModelRegistryRef>,
    pub output: Option<StructuredOutput>,
    pub workflow: Option<ChatWorkflowSelection>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatConfiguration {
    pub order_id: String,
    pub revision: i64,
    pub updated_at: i64,
    pub value: Option<ChatRunConfiguration>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteChatConfiguration {
    pub expected_revision: i64,
    pub idempotency_key: String,
    pub value: ChatRunConfiguration,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkspaceTaskExecution {
    Model {
        provider_id: String,
        provider_revision: i64,
    },
    Workflow {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        inputs: Option<Box<serde_json::Value>>,
        workflow_id: String,
        version_id: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceTaskDefinition {
    pub name: String,
    pub description: String,
    pub order_id: String,
    pub prompt: String,
    pub expected_output: String,
    pub execution: WorkspaceTaskExecution,
    pub access_mode: AccessMode,
    pub budgets: Budgets,
    #[serde(default)]
    pub folders: Vec<FolderGrant>,
    #[serde(default)]
    pub native_tools: Vec<String>,
    #[serde(default)]
    pub mcp_tools: Vec<McpToolSelection>,
    #[serde(default)]
    pub agent_config: Option<EffectiveAgentConfig>,
    #[serde(default)]
    pub environment: Option<ExecutionEnvironmentRef>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceTask {
    pub id: String,
    pub project_id: String,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub archived: bool,
    pub last_run_id: Option<String>,
    pub value: WorkspaceTaskDefinition,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteWorkspaceTask {
    pub expected_revision: i64,
    pub idempotency_key: String,
    pub value: WorkspaceTaskDefinition,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArchiveWorkspaceTask {
    pub expected_revision: i64,
    pub idempotency_key: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StartWorkspaceTask {
    pub expected_revision: i64,
    pub order_revision: i64,
    pub idempotency_key: String,
}
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct WorkspaceManagementContract {
    pub chat_configuration: ChatConfiguration,
    pub write_chat_configuration: WriteChatConfiguration,
    pub project_settings: ProjectSettings,
    pub write_project_settings: WriteProjectSettings,
    pub manage_chat: ManageChat,
    pub archive_chat: ArchiveChat,
    pub chat_management: ChatManagement,
    pub workspace_task: WorkspaceTask,
    pub write_workspace_task: WriteWorkspaceTask,
    pub archive_workspace_task: ArchiveWorkspaceTask,
    pub start_workspace_task: StartWorkspaceTask,
}
