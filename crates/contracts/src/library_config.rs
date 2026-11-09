//! Reusable private content. Saved templates never grant execution authority.
use super::*;

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct GenerationSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentModelSelection {
    #[default]
    Inherit,
    Provider {
        provider_id: String,
        provider_revision: i64,
    },
    Codex {
        account_id: String,
        model: Option<String>,
        reasoning_effort: Option<String>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LibrarySource {
    pub id: String,
    pub revision: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PersonalityDefinition {
    pub name: String,
    pub description: String,
    pub icon: Option<String>,
    pub role: String,
    pub goal: String,
    pub system_prompt: String,
    pub skills_markdown: String,
    pub generation: GenerationSettings,
    pub model: AgentModelSelection,
    pub is_default: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Personality {
    pub id: String,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub value: PersonalityDefinition,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AgentConfiguration {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_filter: Option<Vec<String>>,
    pub role: String,
    pub goal: String,
    pub backstory: String,
    pub instructions: String,
    pub skills_markdown: String,
    pub model: AgentModelSelection,
    pub generation: GenerationSettings,
    pub access_mode: AccessMode,
    pub budgets: Budgets,
    pub native_tools: Vec<String>,
    pub mcp_tools: Vec<McpToolSelection>,
    pub skill_ids: Vec<String>,
    pub memory_ids: Vec<String>,
    pub allow_delegation: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub personality_source: Option<LibrarySource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_source: Option<LibrarySource>,
}
/// Copied prompt/generation values. Existing run fields hold execution authority.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct EffectiveAgentConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_mode: Option<AgentExecutionMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub behavior: Option<AgentBehaviorSettings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_mode: Option<AccessMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_filter: Option<Vec<String>>,
    pub instructions: String,
    pub generation: GenerationSettings,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub personality_source: Option<LibrarySource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_source: Option<LibrarySource>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AgentBehaviorSettings {
    pub command_allow_groups: Vec<Vec<String>>,
    pub command_blacklist: Vec<String>,
    pub max_tool_calls_per_loop: u32,
    pub fallback_to_human_on_repeated_failure: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AgentTemplateDefinition {
    pub name: String,
    pub description: String,
    pub icon: Option<String>,
    pub personality: Option<Personality>,
    pub configuration: AgentConfiguration,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentTemplate {
    pub id: String,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub value: AgentTemplateDefinition,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WritePersonality {
    pub idempotency_key: String,
    pub expected_revision: i64,
    pub value: PersonalityDefinition,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteAgentTemplate {
    pub idempotency_key: String,
    pub expected_revision: i64,
    pub value: AgentTemplateDefinition,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteLibraryEntry {
    pub idempotency_key: String,
    pub expected_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LibraryDeleted {
    pub id: String,
    pub revision: i64,
    pub deleted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ApplicationPreferences {
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub provider_defaults: std::collections::BTreeMap<String, String>,
    pub language: String,
    pub theme: String,
    pub default_provider_id: Option<String>,
    pub default_agent_id: Option<String>,
    pub default_personality_id: Option<String>,
    pub system_prompt: String,
    pub default_budgets: Budgets,
    pub default_access_mode: AccessMode,
    pub auto_approve_safe_tools: bool,
    pub auto_pilot_all_tools: bool,
    pub read_only_fs_mode: bool,
    pub command_whitelist: String,
    pub command_blacklist: String,
    pub max_tool_calls_per_loop: u32,
    pub fallback_to_human_on_repeated_failure: bool,
    pub confirm_on_close_with_running_tasks: bool,
    pub telemetry_enabled: bool,
    pub notifications_enabled: bool,
    pub sounds_enabled: bool,
    pub launch_at_startup: bool,
    pub show_timestamps: bool,
    pub default_start_view: String,
    pub focus_mode: bool,
    pub compact_mode: bool,
    pub verbose_mode: bool,
    pub limit_thinking_window: bool,
    pub super_verbose_audit_logging: bool,
    pub font_scale: u32,
    pub shortcut_overlay_enabled: bool,
    pub sync_theme_with_system: bool,
    pub chat_retention_days: u32,
    pub auto_backup_db: bool,
    pub db_backup_interval_hours: u32,
    pub workspace_default_path: String,
    pub mcp_auto_reconnect: bool,
    pub mcp_verbose_logging: bool,
    pub mcp_env_editor_enabled: bool,
    pub mcp_allow_manual_import: bool,
    pub ollama_stream_autosave: bool,
    pub db_cleanup_on_start: bool,
    pub task_batch_multi_select_enabled: bool,
    pub terminal_persistence_mode: String,
}
impl Default for ApplicationPreferences {
    fn default() -> Self {
        Self {
            provider_defaults: Default::default(),
            language: "de".into(),
            theme: "system".into(),
            default_provider_id: None,
            default_agent_id: None,
            default_personality_id: None,
            system_prompt: String::new(),
            default_budgets: Budgets::default(),
            default_access_mode: AccessMode::default(),
            auto_approve_safe_tools: false,
            auto_pilot_all_tools: false,
            read_only_fs_mode: false,
            command_whitelist: String::new(),
            command_blacklist: String::new(),
            max_tool_calls_per_loop: 64,
            fallback_to_human_on_repeated_failure: true,
            confirm_on_close_with_running_tasks: true,
            telemetry_enabled: false,
            notifications_enabled: true,
            sounds_enabled: false,
            launch_at_startup: false,
            show_timestamps: true,
            default_start_view: "last".into(),
            focus_mode: false,
            compact_mode: false,
            verbose_mode: false,
            limit_thinking_window: true,
            super_verbose_audit_logging: false,
            font_scale: 100,
            shortcut_overlay_enabled: true,
            sync_theme_with_system: true,
            chat_retention_days: 0,
            auto_backup_db: false,
            db_backup_interval_hours: 24,
            workspace_default_path: String::new(),
            mcp_auto_reconnect: true,
            mcp_verbose_logging: false,
            mcp_env_editor_enabled: true,
            mcp_allow_manual_import: true,
            ollama_stream_autosave: true,
            db_cleanup_on_start: false,
            task_batch_multi_select_enabled: true,
            terminal_persistence_mode: "runtime".into(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreferencesRecord {
    pub revision: i64,
    pub updated_at: i64,
    pub value: ApplicationPreferences,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WritePreferences {
    pub idempotency_key: String,
    pub expected_revision: i64,
    pub value: ApplicationPreferences,
}
/// Registers all library wire types with the shared generated contract.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct LibraryConfigContract {
    pub external_memory_provider: ExternalMemoryProvider,
    pub write_external_memory_provider: WriteExternalMemoryProvider,
    pub retrieve_external_memory: RetrieveExternalMemory,
    pub delete_external_memory_provider: DeleteExternalMemoryProvider,
    pub external_memory_retrieval: ExternalMemoryRetrieval,
    pub actor_context_record: ActorContextRecord,
    pub actor_skill_learning_history: ActorSkillLearningHistory,
    pub write_actor_skill: WriteActorSkill,
    pub write_actor_memory: WriteActorMemory,
    pub delete_actor_context: DeleteActorContext,
    pub actor_chat_configuration: ActorChatConfiguration,
    pub write_actor_chat_configuration: WriteActorChatConfiguration,
    pub personality: Personality,
    pub agent_template: AgentTemplate,
    pub write_personality: WritePersonality,
    pub write_agent_template: WriteAgentTemplate,
    pub delete_library_entry: DeleteLibraryEntry,
    pub library_deleted: LibraryDeleted,
    pub preferences_record: PreferencesRecord,
    pub write_preferences: WritePreferences,
    pub agent_configuration: AgentConfiguration,
    pub effective_agent_config: EffectiveAgentConfig,
}
