//! Event input is data, never an execution authority or a tool configuration.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationEventSource {
    Manual,
    Webhook,
    File,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileWatchChange {
    Created,
    Modified,
    Removed,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AutomationFileWatch {
    /// An exact registered external folder capability, narrowed to read-only.
    pub folder: FolderGrant,
    pub path: String,
    pub recursive: bool,
    pub changes: Vec<FileWatchChange>,
    pub extensions: Vec<String>,
    pub exclude_paths: Vec<String>,
    pub include_existing: bool,
    pub debounce_ms: u32,
    pub poll_interval_ms: u32,
    pub max_files: u32,
    pub max_file_bytes: u64,
    pub max_scan_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileWatchObservation {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AutomationFileWatchStatus {
    pub trigger_id: String,
    pub trigger_revision: i64,
    pub initialized: bool,
    pub tracked_files: u32,
    pub pending_changes: u32,
    pub ignored_entries: u32,
    pub last_scan_at: Option<i64>,
    pub next_scan_at: i64,
    pub scanning: bool,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "operator", rename_all = "snake_case", deny_unknown_fields)]
pub enum EventPredicate {
    Exists { pointer: String },
    Equals { pointer: String, value: Value },
    Contains { pointer: String, text: String },
    GreaterThan { pointer: String, value: f64 },
    LessThan { pointer: String, value: f64 },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubmitAutomationEvent {
    /// Stable sender identifier. Reuse with different data is a conflict.
    pub event_id: String,
    pub data: Value,
}

/// Explicit data-only mapping. Targets are typed workflow input pointers;
/// sources are pointers into the untrusted event object. No authority fields.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AutomationWorkflowInputs {
    pub format_version: u32,
    pub bindings: std::collections::BTreeMap<String, String>,
}

/// Frozen values, not an immutable file snapshot or an execution permission.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AutomationWorkflowInputSnapshot {
    pub workflow_id: String,
    pub version_id: String,
    pub definition_sha256: String,
    pub mapping_sha256: String,
    pub values: Box<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationEventState {
    Queued,
    Filtered,
    Paused,
    Dispatching,
    Started,
    Blocked,
    Discarded,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AutomationEvent {
    pub id: String,
    pub project_id: String,
    pub trigger_id: String,
    pub trigger_revision: i64,
    pub task_id: String,
    pub task_revision: i64,
    pub event_id: String,
    pub source: AutomationEventSource,
    pub data: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_input: Option<Box<AutomationWorkflowInputSnapshot>>,
    pub state: AutomationEventState,
    pub run_id: Option<String>,
    pub reason: Option<String>,
    pub received_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AutomationEventPage {
    pub items: Vec<AutomationEvent>,
    pub next_cursor: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AutomationTrigger {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub revision: i64,
    pub task_id: String,
    pub task_revision: i64,
    pub source: AutomationEventSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_watch: Option<AutomationFileWatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_inputs: Option<Box<AutomationWorkflowInputs>>,
    /// All predicates must match. An empty list accepts every object.
    pub predicates: Vec<EventPredicate>,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteAutomationTrigger {
    pub id: Option<String>,
    pub expected_revision: i64,
    pub idempotency_key: String,
    pub name: String,
    pub task_id: String,
    pub task_revision: i64,
    pub source: AutomationEventSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_watch: Option<AutomationFileWatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_inputs: Option<Box<AutomationWorkflowInputs>>,
    pub predicates: Vec<EventPredicate>,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AutomationEventPreview {
    pub matches: bool,
    pub would_queue: bool,
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_revision: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_revision: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_input: Option<Box<AutomationWorkflowInputSnapshot>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RetireAutomationEvent {
    pub idempotency_key: String,
    pub expected_updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AutomationWebhookGrant {
    pub trigger_id: String,
    pub revision: i64,
    pub enabled: bool,
    pub expires_at: i64,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigureAutomationWebhook {
    pub expected_revision: i64,
    pub idempotency_key: String,
    /// Write-only random 256-bit hexadecimal bearer. Only its digest is retained.
    pub token: Option<String>,
    pub enabled: bool,
    pub expires_at: i64,
}

#[derive(JsonSchema)]
pub struct AutomationContract {
    pub file_watch_status: AutomationFileWatchStatus,
    pub trigger: AutomationTrigger,
    pub write_trigger: WriteAutomationTrigger,
    pub submit_event: SubmitAutomationEvent,
    pub event: AutomationEvent,
    pub events: AutomationEventPage,
    pub preview: AutomationEventPreview,
    pub retire_event: RetireAutomationEvent,
    pub webhook_grant: AutomationWebhookGrant,
    pub configure_webhook: ConfigureAutomationWebhook,
}
