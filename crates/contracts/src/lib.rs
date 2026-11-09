use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
mod browser;
pub use browser::*;
mod managed_results;
pub use managed_results::*;
mod managed_processes;
pub use managed_processes::*;
mod workspace_transfer;
pub use workspace_transfer::*;
mod execution_inputs;
pub use execution_inputs::*;
mod execution_controllers;
pub use execution_controllers::*;
mod execution_actor;
pub use execution_actor::*;
mod execution_targets;
pub use execution_targets::*;
mod documents;
pub use documents::*;
mod schedule_recurrence;
pub use schedule_recurrence::*;
mod provider_settings;
pub use provider_settings::*;
mod connectors;
pub use connectors::*;
mod automation;
pub use automation::*;
mod desktop_ops;
pub use desktop_ops::*;

pub const SCHEMA_VERSION: u32 = 8;
pub const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
mod processes;
pub use processes::*;
mod mcp;
pub use mcp::*;
mod mcp_oauth_tasks;
pub use mcp_oauth_tasks::*;
mod native_secrets;
pub use native_secrets::*;
mod personal_money;
pub use personal_money::*;
mod team;
pub use team::*;
mod team_recovery;
pub use team_recovery::*;
mod structured_output;
pub use structured_output::*;
mod knowledge_provenance;
pub use knowledge_provenance::*;
mod knowledge_import;
pub use knowledge_import::*;
mod knowledge_topics;
pub use knowledge_topics::*;
mod skills;
pub use skills::*;
mod mcp_owner;
pub use mcp_owner::*;
mod audit;
pub use audit::*;
mod personal_provider;
pub use personal_provider::*;
mod external_memory;
pub use external_memory::*;
mod actor_context;
pub use actor_context::*;
mod actor_chat_configuration;
pub use actor_chat_configuration::*;
pub mod library_config;
pub use library_config::*;
mod codex_accounts;
pub use codex_accounts::*;
pub mod workspace_management;
pub use workspace_management::*;
mod model_context;
pub use model_context::*;
mod actor_model_registry;
pub use actor_model_registry::*;
mod android_auth_security;
pub use android_auth_security::*;
mod android_administration;
pub use android_administration::*;
pub mod android_private_sharing;
pub use android_private_sharing::*;
mod container_recovery;
pub use container_recovery::*;
mod team_lost_host_recovery;
pub use team_lost_host_recovery::*;
mod android_oidc;
pub use android_oidc::*;
pub mod android_administration_extended;
pub use android_administration_extended::*;
mod android_auth_transport;
pub use android_auth_transport::*;
pub mod historical_import;
pub use historical_import::*;
mod android_terminal;
pub use android_terminal::*;
mod android_schedules;
pub use android_schedules::*;
mod android_run_recovery;
pub use android_run_recovery::*;
mod android_project_versions;
pub use android_project_versions::*;
mod android_workflows;
pub use android_workflows::*;
mod device_interop;
pub use device_interop::*;
mod offline;
pub use offline::*;
pub mod offline_mcp;
pub use offline_mcp::*;
mod android_devices;
pub use android_devices::*;

/// Native-only request. Never accepted using the browser's daemon credential alone.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeFolderSelection {
    pub idempotency_key: String,
    pub project_id: String,
    pub name: String,
    pub path: std::path::PathBuf,
    pub expected_resource_id: String,
    #[serde(default)]
    pub writable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RegisteredFolder {
    pub grant: FolderGrant,
    /// Human native grant manager only; not sent to models as folder context.
    pub canonical_path: std::path::PathBuf,
    pub revoked: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeChooseFolderRequest {
    pub idempotency_key: String,
    pub project_id: String,
    pub writable: bool,
    pub language: UiLanguage,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeFolderOutcome {
    Canceled,
    Registered { folder: RegisteredFolder },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FolderRequestLookup {
    pub folder: Option<RegisteredFolder>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RevokeFolder {
    pub project_id: String,
    pub folder_id: String,
    pub expected_revision: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AccessMode {
    ReadOnly,
    #[default]
    ConfirmMutations,
    ConfirmAll,
    Autonomous,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    WaitingForTimer,
    Queued,
    WaitingForExecutor,
    WaitingForSnapshot,
    Running,
    WaitingApproval,
    WaitingInput,
    CancelRequested,
    Interrupted,
    Completed,
    Failed,
    Canceled,
    Expired,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub revision: i64,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateProject {
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Order {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub draft: String,
    pub revision: i64,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateOrder {
    pub project_id: String,
    pub title: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveDraft {
    pub base_revision: i64,
    pub draft: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BranchOrder {
    pub title: String,
    pub through_message_id: String,
    pub idempotency_key: String,
}

/// Original IDs and ownership are retained in this immutable historical context.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct HistorySnapshot {
    pub source_order_id: String,
    pub through_message_id: String,
    pub created_at: i64,
    pub sha256: String,
    pub messages: Vec<Message>,
    pub artifacts: Vec<Artifact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspace_snapshots: Vec<WorkspaceSnapshot>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct OrderHistory {
    pub inherited: Option<HistorySnapshot>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct OrderArchive {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_input_resume_operations: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_input_answer_records: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflows: Vec<WorkflowSnapshot>,
    pub project: Project,
    pub order: Order,
    pub history: OrderHistory,
    pub messages: Vec<Message>,
    pub artifacts: Vec<Artifact>,
    pub runs: Vec<RunDetail>,
    pub events: Vec<RunEvent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uploads: Vec<FileUpload>,
}

/// Read-only archival data. Never accepted as workflow grants or IO authority.
/// The Rust domain validates these private checkpoint bodies on export/import.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowSnapshot {
    pub run_id: String,
    pub project_id: String,
    pub binding: WorkflowBinding,
    pub progress_sha256: String,
    pub plan: Value,
    pub progress: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_contexts: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_inputs: Option<Value>,
}

/// A reviewable conversation export, not an executable migration package.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct OrderExport {
    pub format_version: u32,
    pub exported_at: i64,
    pub sha256: String,
    pub data: OrderArchive,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ActivityItem {
    pub run: RunRecord,
    pub project_name: String,
    pub order_title: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ActivityPage {
    pub items: Vec<ActivityItem>,
    pub total: i64,
    pub attention: i64,
    pub next_offset: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchKind {
    Order,
    Message,
    Artifact,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SearchHit {
    pub id: String,
    pub project_id: String,
    pub project_name: String,
    pub order_id: String,
    pub order_title: String,
    pub kind: SearchKind,
    pub excerpt: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SearchPage {
    pub items: Vec<SearchHit>,
    pub next_offset: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Ollama,
    OpenaiCompatible,
    Codex,
    Test,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ProviderProfile {
    #[serde(default, skip_serializing_if = "GenerationSettings::is_empty")]
    pub generation: GenerationSettings,
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub endpoint: String,
    pub model: String,
    pub revision: i64,
    pub context_tokens: u32,
    pub credential_required: bool,
    pub credential_ready: bool,
    /// Omitted when empty so existing profile hashes stay unchanged.
    #[serde(default, skip_serializing_if = "ModelCapabilities::is_empty")]
    pub capabilities: ModelCapabilities,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateProvider {
    pub name: String,
    pub kind: ProviderKind,
    pub endpoint: String,
    pub model: String,
    pub context_tokens: u32,
    #[serde(default, skip_serializing_if = "ModelCapabilities::is_empty")]
    pub capabilities: ModelCapabilities,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Budgets {
    pub max_steps: u32,
    pub max_seconds: u64,
    pub max_output_tokens: u32,
    #[serde(
        default = "default_total_tokens",
        skip_serializing_if = "is_default_total_tokens"
    )]
    pub max_total_tokens: u64,
}
pub fn default_total_tokens() -> u64 {
    262_144
}
fn is_default_total_tokens(value: &u64) -> bool {
    *value == default_total_tokens()
}
fn is_default_budgets(value: &Budgets) -> bool {
    *value == Budgets::default()
}
impl Default for Budgets {
    fn default() -> Self {
        Self {
            max_steps: 64,
            max_seconds: 1800,
            max_output_tokens: 4096,
            max_total_tokens: default_total_tokens(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateRun {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_config: Option<EffectiveAgentConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_binding: Option<ModelRegistryRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<StructuredOutput>,
    /// Explicit owner-native adapters. Empty preserves existing capability scope.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<ExecutionEnvironmentRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_tools: Vec<McpToolSelection>,
    #[serde(default, skip_serializing_if = "is_default_budgets")]
    pub budgets: Budgets,
    pub order_id: String,
    pub base_revision: i64,
    pub idempotency_key: String,
    pub prompt: String,
    pub provider_id: String,
    pub provider_revision: i64,
    #[serde(default)]
    pub access_mode: AccessMode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<FolderGrant>,
}

/// Frozen binding to a private native workflow plan stored with this run.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkflowBinding {
    /// Explicit run input, frozen with this binding; never tool authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<Box<Value>>,
    pub workflow_id: String,
    pub version_id: String,
    pub definition_sha256: String,
    pub plan_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RunSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_workspace: Option<RunWorkspaceBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_config: Option<EffectiveAgentConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_binding: Option<ModelRegistryRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<StructuredOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<WorkflowBinding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_tools: Vec<McpToolBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<ExecutionEnvironmentRef>,
    pub order_id: String,
    pub project_id: String,
    pub prompt: String,
    pub provider: Option<ProviderProfile>,
    /// A native file action runs through the same durable policy/tool path, without a model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_action: Option<ToolCall>,
    pub access_mode: AccessMode,
    pub executor: String,
    pub policy_revision: i64,
    pub allowed_tools: Vec<String>,
    pub budgets: Budgets,
    #[serde(default)]
    pub folders: Vec<FolderGrant>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FolderGrant {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub writable: bool,
    pub revision: String,
    pub resource_id: String,
    /// Present only for a managed working directory, never an external grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct Workspace {
    pub order_id: String,
    pub folder: FolderGrant,
    pub base_snapshot_sha256: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct WorkspaceEntry {
    pub path: String,
    /// None denotes a directory; file entries include their content hash.
    pub sha256: Option<String>,
    pub size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct WorkspaceSnapshot {
    pub run_id: String,
    pub order_id: String,
    pub entries: Vec<WorkspaceEntry>,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileLocation {
    pub folder_id: String,
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteTextFile {
    pub folder_id: String,
    pub path: String,
    pub content: String,
    /// Null means create-only. Existing files always require their observed SHA-256.
    pub expected_sha256: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct DirectoryListing {
    pub folder_id: String,
    pub path: String,
    pub entries: Vec<FileEntry>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TextFile {
    pub folder_id: String,
    pub path: String,
    pub content: String,
    pub sha256: String,
    pub size: u64,
}

pub const MAX_DOWNLOAD_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DownloadFile {
    pub path: String,
    #[serde(default)]
    pub expected_sha256: Option<String>,
}

/// Metadata sent with an authenticated binary response, validated again by the client.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct DownloadMetadata {
    #[schemars(length(min = 1, max = 1024))]
    pub file_name: String,
    #[schemars(range(min = 0, max = 67108864))]
    pub size: u64,
    #[schemars(regex(pattern = "^[0-9a-f]{64}$"))]
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UiLanguage {
    De,
    En,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeSaveRequest {
    pub file: DownloadMetadata,
    pub language: UiLanguage,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum NativeSaveOutcome {
    Canceled,
    Saved { file: DownloadMetadata },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct FileChange {
    pub run_id: String,
    pub tool_call_id: String,
    pub folder_id: String,
    pub path: String,
    pub before_sha256: Option<String>,
    pub after_sha256: String,
    pub size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileNodeKind {
    File,
    Directory,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct FileNode {
    pub kind: FileNodeKind,
    pub sha256: String,
    pub size: u64,
    pub resource_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct FileStat {
    pub folder_id: String,
    pub path: String,
    pub node: Option<FileNode>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveFile {
    pub folder_id: String,
    pub path: String,
    pub expected_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameFile {
    pub folder_id: String,
    pub path: String,
    pub destination: String,
    pub expected_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileOperationKind {
    CreateDirectory,
    Rename,
    Remove,
    Restore,
    Import,
    /// A new file or directory published by a built-in document tool; never a replacement.
    Generate,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RetainedVersion {
    TextBefore,
    TextAfter,
    OperationBefore,
    OperationAfter,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileRestoreSource {
    pub run_id: String,
    pub tool_call_id: String,
    pub version: RetainedVersion,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RestoreFile {
    /// Optional for legacy receipts. Confirmed copy restoration binds this
    /// server-resolved hash and checks it again before reading retained bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_source_sha256: Option<String>,
    pub folder_id: String,
    pub path: String,
    pub source: FileRestoreSource,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateRestoreRun {
    pub order_id: String,
    pub base_revision: i64,
    pub idempotency_key: String,
    pub prompt: String,
    #[serde(default)]
    pub access_mode: AccessMode,
    pub restore: RestoreFile,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UploadState {
    Reserved,
    Ready,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReserveUpload {
    pub idempotency_key: String,
    pub file: DownloadMetadata,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct FileUpload {
    pub id: String,
    pub order_id: String,
    pub project_id: String,
    pub file: DownloadMetadata,
    pub state: UploadState,
    pub revision: i64,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct UploadPage {
    pub items: Vec<FileUpload>,
    pub next_offset: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportFile {
    pub folder_id: String,
    pub path: String,
    pub upload_id: String,
    pub file: DownloadMetadata,
    /// Null permits creation only; replacement requires the observed target hash.
    pub expected_sha256: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateImportRun {
    pub order_id: String,
    pub base_revision: i64,
    pub idempotency_key: String,
    pub prompt: String,
    #[serde(default)]
    pub access_mode: AccessMode,
    pub import: ImportFile,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct FileOperation {
    pub run_id: String,
    pub tool_call_id: String,
    pub folder_id: String,
    pub kind: FileOperationKind,
    pub path: String,
    pub destination: Option<String>,
    pub before: Option<FileNode>,
    pub after: Option<FileNode>,
    /// Retained manifest for a directory; regular-file contents use before.sha256.
    pub entries: Vec<WorkspaceEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored_from: Option<FileRestoreSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_from: Option<FileUpload>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_from: Option<DocumentProvenance>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReconcileFileDecision {
    AcceptCompleted,
    ConfirmNotApplied,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReconcileFile {
    pub fingerprint: String,
    pub decision: ReconcileFileDecision,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RunRecord {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_attempts: Vec<ModelAttempt>,
    pub id: String,
    pub spec: RunSpec,
    pub state: RunState,
    pub revision: i64,
    pub last_sequence: i64,
    pub steps: u32,
    pub created_at: i64,
    pub updated_at: i64,
    pub started_at: Option<i64>,
    pub error_code: Option<String>,
    pub usage: Usage,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct Usage {
    /// Reported totals are partial when any request has no complete measurement.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub incomplete: bool,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cost: Option<f64>,
    pub price_source_version: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelAttemptState {
    InFlight,
    Measured,
    Unmeasured,
    Interrupted,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ModelAttempt {
    pub id: String,
    pub step: u32,
    /// Absent for imported preview history whose exact request was not recorded.
    pub request_sha256: Option<String>,
    pub reserved_tokens: u64,
    pub state: ModelAttemptState,
    pub usage: Usage,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub limit_exceeded: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct TokenBudgetStatus {
    pub limit: u64,
    pub charged_tokens: u64,
    pub unconfirmed_tokens: u64,
    pub remaining_tokens: u64,
    pub limit_exceeded: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RunEvent {
    pub schema_version: u32,
    pub id: String,
    pub run_id: String,
    pub sequence: i64,
    pub kind: String,
    pub payload: Value,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Message {
    pub id: String,
    pub order_id: String,
    pub run_id: String,
    pub role: String,
    pub content: String,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Approval {
    pub id: String,
    pub run_id: String,
    pub tool: ToolCall,
    pub target: String,
    pub policy_revision: i64,
    pub fingerprint: String,
    pub decision: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResolveApproval {
    pub fingerprint: String,
    pub allow: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubmitInput {
    pub request_id: String,
    pub answer: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Artifact {
    pub id: String,
    pub run_id: String,
    pub order_id: String,
    pub name: String,
    pub media_type: String,
    pub size: u64,
    pub sha256: String,
    pub content: String,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RunDetail {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_attempts: Vec<McpAttempt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub process_attempts: Vec<ProcessAttempt>,
    pub token_budget: TokenBudgetStatus,
    #[serde(default)]
    pub file_operations: Vec<FileOperation>,
    #[serde(default)]
    pub uncertain_file_operation: Option<FileOperation>,
    pub run: RunRecord,
    pub approvals: Vec<Approval>,
    pub input_request: Option<ToolCall>,
    pub artifacts: Vec<Artifact>,
    #[serde(default)]
    pub file_changes: Vec<FileChange>,
    #[serde(default)]
    pub uncertain_file_change: Option<FileChange>,
    #[serde(default)]
    pub uncertain_file_fingerprint: Option<String>,
    #[serde(default)]
    pub workspace_snapshot: Option<WorkspaceSnapshot>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Preflight {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_binding: Option<ModelRegistryRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<ExecutionEnvironmentRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_tools: Vec<McpToolBinding>,
    pub ready: bool,
    pub provider: ProviderProfile,
    pub executor: String,
    pub policy_revision: i64,
    pub access_mode: AccessMode,
    pub allowed_tools: Vec<String>,
    pub budgets: Budgets,
    #[serde(default)]
    pub folders: Vec<FolderGrant>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub correlation_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Health {
    pub product: String,
    pub version: String,
    pub schema_version: u32,
    pub deployment: String,
    pub capabilities: Vec<String>,
}

/// Contract-generation root. Every public request/response is reachable from this type.
#[derive(JsonSchema)]
pub struct ApiContract {
    pub reconcile_owner_managed_process: ReconcileOwnerManagedProcess,
    pub owner_managed_process_reconciliation: OwnerManagedProcessReconciliation,
    pub owner_managed_result_list: OwnerManagedResultList,
    pub owner_managed_result_entry: OwnerManagedResultEntry,
    pub owner_managed_result_manifest: OwnerManagedResultManifest,
    pub owner_managed_result_delivery_view: OwnerManagedResultDeliveryView,
    pub confirm_owner_managed_result_delivery: ConfirmOwnerManagedResultDelivery,
    pub automation: AutomationContract,
    pub create_browser_session: CreateBrowserSession,
    pub browser_session_view: BrowserSessionView,
    pub browser_command: BrowserCommand,
    pub browser_input: BrowserInput,
    pub browser_takeover: BrowserTakeover,
    pub delete_browser_profile: DeleteBrowserProfile,
    pub browser_extension_connection: BrowserExtensionConnection,
    pub native_save_database_backup: NativeSaveDatabaseBackup,
    pub database_backup_save_outcome: DatabaseBackupSaveOutcome,
    pub owner_terminal_launch: OwnerTerminalLaunch,
    pub owner_terminal_preview: OwnerTerminalPreview,
    pub owner_terminal_session: OwnerTerminalSession,
    pub owner_connector: OwnerConnector,
    pub save_owner_connector: SaveOwnerConnector,
    pub delete_owner_connector: DeleteOwnerConnector,
    pub owner_connector_probe: OwnerConnectorProbe,
    pub owner_terminal_status: OwnerTerminalStatus,
    pub owner_terminal_output: OwnerTerminalOutput,
    pub owner_terminal_input: OwnerTerminalInput,
    pub owner_terminal_resize: OwnerTerminalResize,
    pub maintenance_status: MaintenanceStatus,
    pub owner_database_backup_request: OwnerDatabaseBackupRequest,
    pub schedule_recurrence: ScheduleRecurrence,
    pub workspace_management: WorkspaceManagementContract,
    pub configure_personal_provider: ConfigurePersonalProvider,
    pub offline_grant_view: OfflineGrantView,
    pub offline_local_grant_view: OfflineLocalGrantView,
    pub offline_grant_status_request: OfflineGrantStatusRequest,
    pub offline_grant_status: OfflineGrantStatus,
    pub signed_offline_grant_status: SignedOfflineGrantStatus,
    pub offline_run_summary: OfflineRunSummary,
    pub offline_run_page: OfflineRunPage,
    pub offline_device: OfflineDevice,
    pub offline_device_proof_request: OfflineDeviceProofRequest,
    pub offline_device_proof: OfflineDeviceProof,
    pub offline_source: OfflineSource,
    pub offline_scope_revision: OfflineScopeRevision,
    pub offline_grant: OfflineGrant,
    pub signed_offline_grant: SignedOfflineGrant,
    pub issue_offline_grant: IssueOfflineGrant,
    pub install_offline_grant: InstallOfflineGrant,
    pub configure_offline_credential: ConfigureOfflineCredential,
    pub configure_offline_tools: ConfigureOfflineTools,
    pub offline_mcp_connection_input: OfflineMcpConnectionInput,
    pub offline_mcp_connection_view: OfflineMcpConnectionView,
    pub offline_tool_configuration_view: OfflineToolConfigurationView,
    pub offline_workspace_upload: OfflineWorkspaceUpload,
    pub start_offline_run: StartOfflineRun,
    pub offline_journal_entry: OfflineJournalEntry,
    pub offline_pending_tool: OfflinePendingTool,
    pub offline_artifact: OfflineArtifact,
    pub offline_run: OfflineRun,
    pub approve_offline_tools: ApproveOfflineTools,
    pub offline_run_export: OfflineRunExport,
    pub offline_reconciliation_receipt: OfflineReconciliationReceipt,
    pub device_capability: DeviceCapability,
    pub device_executor_registration: DeviceExecutorRegistration,
    pub pair_device_executor: PairDeviceExecutor,
    pub update_device_executor: UpdateDeviceExecutor,
    pub device_passkey_record: DevicePasskeyRecord,
    pub device_passkey_challenge: DevicePasskeyChallenge,
    pub finish_device_passkey_registration: FinishDevicePasskeyRegistration,
    pub start_device_passkey_authentication: StartDevicePasskeyAuthentication,
    pub finish_device_passkey_authentication: FinishDevicePasskeyAuthentication,
    pub device_push_subscription: DevicePushSubscription,
    pub register_device_push: RegisterDevicePush,
    pub device_feature_configuration: DeviceFeatureConfiguration,
    pub device_credential_metadata: DeviceCredentialMetadata,
    pub device_pairing_record: DevicePairingRecord,
    pub team: Team,
    pub create_team: CreateTeam,
    pub update_team: UpdateTeam,
    pub team_member: TeamMember,
    pub add_team_member: AddTeamMember,
    pub update_team_member: UpdateTeamMember,
    pub configure_team_provider: ConfigureTeamProvider,
    pub team_provider: TeamProvider,
    pub team_mcp_server: TeamMcpServer,
    pub configure_team_mcp_server: ConfigureTeamMcpServer,
    pub team_mcp_tool: TeamMcpTool,
    pub team_github_configuration: TeamGithubConfiguration,
    pub configure_team_github: ConfigureTeamGithub,
    pub team_project: TeamProject,
    pub create_team_project: CreateTeamProject,
    pub update_team_project: UpdateTeamProject,
    pub team_project_grant: TeamProjectGrant,
    pub team_project_grant_view: TeamProjectGrantView,
    pub team_order: TeamOrder,
    pub create_team_order: CreateTeamOrder,
    pub update_team_order: UpdateTeamOrder,
    pub team_document: TeamDocument,
    pub save_team_document: SaveTeamDocument,
    pub team_run: TeamRun,
    pub create_team_run: CreateTeamRun,
    pub team_run_step: TeamRunStep,
    pub team_effect: TeamEffectView,
    pub team_effect_detail: TeamEffectDetail,
    pub team_recovery: TeamRecoveryView,
    pub structured_output: StructuredOutput,
    pub knowledge_provenance: KnowledgeProvenance,
    pub knowledge_revision: KnowledgeRevision,
    pub knowledge_import_request: KnowledgeImportRequest,
    pub knowledge_import_result: KnowledgeImportResult,
    pub knowledge_folders: KnowledgeFolders,
    pub write_knowledge_folders: WriteKnowledgeFolders,
    pub build_knowledge_graph: BuildKnowledgeGraph,
    pub knowledge_topic_graph: KnowledgeTopicGraph,
    pub search_knowledge_graph: SearchKnowledgeGraph,
    pub knowledge_graph_results: KnowledgeGraphResults,
    pub skill: Skill,
    pub write_skill: WriteSkill,
    pub native_skill_history: NativeSkillHistory,
    pub skill_learning_history: SkillLearningHistory,
    pub configure_owner_mcp_stdio: ConfigureOwnerMcpStdio,
    pub native_mcp_factory: NativeMcpFactory,
    pub configure_owner_mcp_credential: ConfigureOwnerMcpCredential,
    pub owner_mcp_credential: OwnerMcpCredential,
    pub owner_mcp_discovery: OwnerMcpDiscovery,
    pub owner_mcp_catalog: OwnerMcpCatalog,
    pub owner_mcp_capabilities: OwnerMcpCapabilities,
    pub owner_mcp_revision: OwnerMcpRevision,
    pub owner_mcp_expected_revision: OwnerMcpExpectedRevision,
    pub model_registry_ref: ModelRegistryRef,
    pub import_model_qualification: ImportModelQualification,
    pub model_registry_view: ModelRegistryView,
    pub actor_model_context: ActorModelContext,
    pub android_password_credentials: AndroidPasswordCredentials,
    pub android_auth_tokens: AndroidAuthTokens,
    pub android_authentication_assurance: AndroidAuthenticationAssurance,
    pub android_auth_session: AndroidAuthSession,
    pub android_totp_status: AndroidTotpStatus,
    pub android_totp_setup: AndroidTotpSetup,
    pub android_totp_code: AndroidTotpCode,
    pub android_disable_totp: AndroidDisableTotp,
    pub android_recovery_codes: AndroidRecoveryCodes,
    pub android_create_invitation: AndroidCreateInvitation,
    pub android_invitation_secret: AndroidInvitationSecret,
    pub android_accept_invitation: AndroidAcceptInvitation,
    pub android_create_team: AndroidCreateTeam,
    pub android_set_team_member: AndroidSetTeamMember,
    pub android_team_member: AndroidTeamMember,
    pub android_team: AndroidTeam,
    pub android_auth_management_policy: AndroidAuthManagementPolicy,
    pub set_android_auth_management_policy: SetAndroidAuthManagementPolicy,
    pub android_native_authorization: AndroidNativeAuthorization,
    pub android_native_authorization_code: AndroidNativeAuthorizationCode,
    pub android_native_token_exchange: AndroidNativeTokenExchange,
    pub android_refresh_session: AndroidRefreshSession,
    pub android_browser_auth_tokens: AndroidBrowserAuthTokens,
    pub android_auth_security_context: AndroidAuthSecurityContext,
    pub native_auth_transport_request: NativeAuthTransportRequest,
    pub native_auth_transport_view: NativeAuthTransportView,
    pub native_auth_transport_replay: NativeAuthTransportReplay,
    pub native_auth_transport_capture: NativeAuthTransportCapture,
    pub native_auth_transport_resolution: NativeAuthTransportResolution,
    pub historical_owner: HistoricalOwner,
    pub historical_stream: HistoricalStream,
    pub historical_pin_set: HistoricalPinSet,
    pub historical_file_declaration: HistoricalFileDeclaration,
    pub begin_historical_import: BeginHistoricalImport,
    pub historical_import_limit: HistoricalImportLimit,
    pub historical_import_record: HistoricalImportRecord,
    pub historical_chunk_receipt: HistoricalChunkReceipt,
    pub historical_content_project: HistoricalContentProject,
    pub historical_content_page: HistoricalContentPage,
    pub historical_content_catalog_entry: HistoricalContentCatalogEntry,
    pub historical_content_catalog_manifest: HistoricalContentCatalogManifest,
    pub historical_content_catalog_item: HistoricalContentCatalogItem,
    pub historical_content_catalog_page: HistoricalContentCatalogPage,
    pub historical_preview_project_summary: HistoricalPreviewProjectSummary,
    pub historical_preview_summary: HistoricalPreviewSummary,
    pub historical_content_order: HistoricalContentOrder,
    pub historical_content_document: HistoricalContentDocument,
    pub historical_content_file: HistoricalContentFile,
    pub historical_project_target: HistoricalProjectTarget,
    pub historical_project_mapping: HistoricalProjectMapping,
    pub preview_historical_import: PreviewHistoricalImport,
    pub historical_activated_object: HistoricalActivatedObject,
    pub historical_prepared_project: HistoricalPreparedProject,
    pub historical_import_preview: HistoricalImportPreview,
    pub activate_historical_import: ActivateHistoricalImport,
    pub historical_activation_receipt: HistoricalActivationReceipt,
    pub historical_file_stage_request: HistoricalFileStageRequest,
    pub historical_file_operation: HistoricalFileOperation,
    pub close_historical_unsent_file: CloseHistoricalUnsentFile,
    pub historical_unsent_file_receipt: HistoricalUnsentFileReceipt,
    pub historical_content_message: HistoricalContentMessage,
    pub historical_content_artifact: HistoricalContentArtifact,
    pub configure_historical_import_limit: ConfigureHistoricalImportLimit,
    pub historical_import_target: HistoricalImportTarget,
    pub build_historical_history_index: BuildHistoricalHistoryIndex,
    pub historical_history_page_pin: HistoricalHistoryPagePin,
    pub historical_history_index: HistoricalHistoryIndex,
    pub historical_history_table: HistoricalHistoryTable,
    pub historical_history_row: HistoricalHistoryRow,
    pub historical_history_cell: HistoricalHistoryCell,
    pub historical_history_column: HistoricalHistoryColumn,
    pub historical_history_cells_page: HistoricalHistoryCellsPage,
    pub historical_history_tables_page: HistoricalHistoryTablesPage,
    pub historical_history_rows_page: HistoricalHistoryRowsPage,
    pub android_create_terminal_session: AndroidCreateTerminalSession,
    pub android_terminal_session_ticket: AndroidTerminalSessionTicket,
    pub android_terminal_session: AndroidTerminalSession,
    pub android_terminal_session_page: AndroidTerminalSessionPage,
    pub android_terminal_resize: AndroidTerminalResize,
    pub android_schedule_record: AndroidScheduleRecord,
    pub android_create_schedule: AndroidCreateSchedule,
    pub android_create_thread_schedule: AndroidCreateThreadSchedule,
    pub android_update_schedule: AndroidUpdateSchedule,
    pub android_thread_schedule: AndroidThreadSchedule,
    pub android_schedule_occurrence: AndroidScheduleOccurrence,
    pub android_schedule_history: AndroidScheduleHistory,
    pub android_schedule_preview: AndroidSchedulePreview,
    pub android_schedule_original: AndroidScheduleOriginal,
    pub android_create_schedule_task: AndroidCreateScheduleTask,
    pub android_schedule_resolution_request: AndroidScheduleResolutionRequest,
    pub android_schedule_resolution_view: AndroidScheduleResolutionView,
    pub android_schedule_resolution_receipt: AndroidScheduleResolutionReceipt,
    pub android_run_checkpoint: AndroidRunCheckpoint,
    pub android_run_checkpoint_descriptor: AndroidRunCheckpointDescriptor,
    pub android_run_checkpoint_page: AndroidRunCheckpointPage,
    pub android_run_waiting_reason: AndroidRunWaitingReason,
    pub android_run_waiting: AndroidRunWaiting,
    pub android_resume_run: AndroidResumeRun,
    pub android_run_resume_receipt: AndroidRunResumeReceipt,
    pub android_project_version: AndroidProjectVersion,
    pub android_create_project_version: AndroidCreateProjectVersion,
    pub android_apply_project_version: AndroidApplyProjectVersion,
    pub android_merge_file_status: AndroidMergeFileStatus,
    pub android_merge_file_review: AndroidMergeFileReview,
    pub android_project_merge_review: AndroidProjectMergeReview,
    pub android_merge_resolution_choice: AndroidMergeResolutionChoice,
    pub android_merge_file_resolution: AndroidMergeFileResolution,
    pub android_apply_project_merge: AndroidApplyProjectMerge,
    pub android_fork_destination: AndroidForkDestination,
    pub android_fork_project: AndroidForkProject,
    pub android_fork_object_mapping: AndroidForkObjectMapping,
    pub android_project_fork_receipt: AndroidProjectForkReceipt,
    pub android_fork_thread: AndroidForkThread,
    pub android_project_version_content: AndroidProjectVersionContent,
    pub android_workflow_engine: AndroidWorkflowEngine,
    pub android_workflow_version_ref: AndroidWorkflowVersionRef,
    pub android_workflow_entry: AndroidWorkflowEntry,
    pub android_workflow_version: AndroidWorkflowVersion,
    pub android_workflow_page: AndroidWorkflowPage,
    pub android_workflow_version_page: AndroidWorkflowVersionPage,
    pub android_workflow_write: AndroidWorkflowWrite,
    pub android_workflow_change: AndroidWorkflowChange,
    pub android_workflow_selection: AndroidWorkflowSelection,
    pub android_workflow_preflight: AndroidWorkflowPreflight,
    pub android_workflow_operation_receipt: AndroidWorkflowOperationReceipt,
    pub android_workflow_node_view: AndroidWorkflowNodeView,
    pub android_workflow_run_view: AndroidWorkflowRunView,
    pub mcp_task_detail: McpTaskDetail,
    pub mcp_task_interaction: McpTaskInteraction,
    pub answer_mcp_task_interaction: AnswerMcpTaskInteraction,
    pub offline_mcp_interaction_page: OfflineMcpInteractionPage,
    pub offline_mcp_result_bytes: OfflineMcpResultBytes,
    pub android_project_member_write: AndroidProjectMemberWrite,
    pub android_project_member_view: AndroidProjectMemberView,
    pub android_project_member_page: AndroidProjectMemberPage,
    pub android_private_project_member_view: AndroidPrivateProjectMemberView,
    pub android_private_project_member_page: AndroidPrivateProjectMemberPage,
    pub android_private_project_member_receipt: AndroidPrivateProjectMemberReceipt,
    pub team_container_recovery_plan: TeamContainerRecoveryPlan,
    pub retire_team_container: RetireTeamContainer,
    pub team_container_recovery_receipt: TeamContainerRecoveryReceipt,
    pub team_original_host_binding: TeamOriginalHostBinding,
    pub team_host_trust_profile_view: TeamHostTrustProfileView,
    pub team_lost_host_plan: TeamLostHostPlan,
    pub prepare_team_lost_host_retirement: PrepareTeamLostHostRetirement,
    pub receive_team_lost_host_retirement: ReceiveTeamLostHostRetirement,
    pub team_lost_host_retirement_receipt: TeamLostHostRetirementReceipt,
    pub android_oidc_config: AndroidOidcConfig,
    pub android_oidc_profile_write: AndroidOidcProfileWrite,
    pub android_oidc_profile: AndroidOidcProfile,
    pub android_oidc_begin: AndroidOidcBegin,
    pub android_oidc_flow: AndroidOidcFlow,
    pub android_oidc_complete: AndroidOidcComplete,
    pub android_oidc_finish: AndroidOidcFinish,
    pub android_oidc_retire: AndroidOidcRetire,
    pub android_oidc_compat_start: AndroidOidcCompatStart,
    pub android_oidc_recover: AndroidOidcRecover,
    pub android_oidc_link: AndroidOidcLink,
    pub android_oidc_unlink: AndroidOidcUnlink,
    pub android_oidc_origin: AndroidOidcOrigin,
    pub android_oidc_group_mapping: AndroidOidcGroupMapping,
    pub android_oidc_sso_provider: AndroidOidcSsoProvider,
    pub android_oidc_sso_settings: AndroidOidcSsoSettings,
    pub android_oidc_sso_write: AndroidOidcSsoWrite,
    pub android_oidc_membership_origin: AndroidOidcMembershipOrigin,
    pub android_mobile_cache_begin: AndroidMobileCacheBegin,
    pub android_administration_revision: AndroidAdministrationRevision,
    pub android_mobile_cache_activate: AndroidMobileCacheActivate,
    pub android_support_grant_write: AndroidSupportGrantWrite,
    pub android_support_grant_revoke: AndroidSupportGrantRevoke,
    pub android_personal_storage_quota_write: AndroidPersonalStorageQuotaWrite,
    pub android_executor_pool_write: AndroidExecutorPoolWrite,
    pub android_executor_pool_project_write: AndroidExecutorPoolProjectWrite,
    pub android_executor_sync_grant_write: AndroidExecutorSyncGrantWrite,
    pub android_wire_sync_change: AndroidWireSyncChange,
    pub android_wire_sync_batch: AndroidWireSyncBatch,
    pub android_executor_sync_batch: AndroidExecutorSyncBatch,
    pub android_content_access: AndroidContentAccess,
    pub device_office_request: DeviceOfficeRequest,
    pub device_crew_agent: DeviceCrewAgent,
    pub device_crew_task: DeviceCrewTask,
    pub device_crew_definition: DeviceCrewDefinition,
    pub device_interop_adapter: DeviceInteropAdapter,
    pub device_interop_inventory: DeviceInteropInventory,
    pub actor_model_registry_view: ActorModelRegistryView,
    pub import_actor_model_qualification: ImportActorModelQualification,
    pub revoke_actor_model_qualification: RevokeActorModelQualification,
    pub preview_model_context: PreviewModelContext,
    pub compact_model_context: CompactModelContext,
    pub model_context_view: ModelContextView,
    pub context_compaction_view: ContextCompactionView,
    pub context_archive_view: ContextArchiveView,
    pub audit_record: AuditRecord,
    pub audit_page: AuditPage,
    pub signed_audit_archive: SignedAuditArchive,
    pub audit_archive_bundle: AuditArchiveBundle,
    pub capture_audit_archive: CaptureAuditArchive,
    pub preserve_audit_history: PreserveAuditHistory,
    pub preserved_audit_history: PreservedAuditHistory,
    pub audit_verification: AuditVerification,
    pub audit_insights: AuditInsights,
    pub audit_diagnostics: AuditDiagnostics,
    pub audit_insights_query: AuditInsightsQuery,
    pub audit_skill_use: AuditSkillUse,
    pub audit_cost: AuditCost,
    pub audit_metric_fact: AuditMetricFact,
    pub audit_execution_totals: AuditExecutionTotals,
    pub audit_execution_insights: AuditExecutionInsights,
    pub audit_original_reference: AuditOriginalReference,
    pub verify_audit_v1: VerifyAuditV1,
    pub audit_v1_assessment: AuditV1Assessment,
    pub audit_v1_verification: AuditV1Verification,
    pub audit_recovery_plan: AuditRecoveryPlan,
    pub confirm_audit_recovery: ConfirmAuditRecovery,
    pub audit_recovery_receipt: AuditRecoveryReceipt,
    pub configure_audit_witness: ConfigureAuditWitness,
    pub audit_witness_binding: AuditWitnessBinding,
    pub capture_audit_checkpoint: CaptureAuditCheckpoint,
    pub audit_checkpoint_request: AuditCheckpointRequest,
    pub signed_audit_checkpoint: SignedAuditCheckpoint,
    pub import_audit_checkpoint: ImportAuditCheckpoint,
    pub audit_checkpoint_status: AuditCheckpointStatus,
    pub prepare_audit_trust_transition: PrepareAuditTrustTransition,
    pub audit_trust_transition_proposal: AuditTrustTransitionProposal,
    pub signed_audit_trust_transition: SignedAuditTrustTransition,
    pub import_audit_trust_transition: ImportAuditTrustTransition,
    pub audit_origin_rotation_plan: AuditOriginRotationPlan,
    pub resolve_team_effect: ResolveTeamEffect,
    pub resume_team_run: ResumeTeamRun,
    pub team_tool_approval: TeamToolApproval,
    pub approve_team_tool: ApproveTeamTool,
    pub personal_money: PersonalMoneyStatus,
    pub configure_mcp_server: ConfigureMcpServer,
    pub mcp_server: McpServer,
    pub mcp_attempt: McpAttempt,
    pub mcp_oauth_configuration: McpOAuthConfiguration,
    pub configure_mcp_oauth: ConfigureMcpOAuth,
    pub mcp_oauth_profile: McpOAuthProfile,
    pub mcp_oauth_flow: McpOAuthFlow,
    pub mcp_oauth_request: McpOAuthRequest,
    pub mcp_oauth_discovery: McpOAuthDiscovery,
    pub mcp_oauth_metadata: McpOAuthMetadata,
    pub mcp_async_task: McpAsyncTask,
    pub mcp_async_task_request: McpAsyncTaskRequest,
    pub native_secrets_status: NativeSecretsStatus,
    pub native_secrets_setup: NativeSecretsSetup,
    pub native_secrets_resolution: NativeSecretsResolution,
    pub native_secrets_receipt: NativeSecretsReceipt,
    pub native_secrets_request_lookup: NativeSecretsRequestLookup,
    pub process_invocation: ProcessInvocation,
    pub native_folder_selection: NativeFolderSelection,
    pub registered_folder: RegisteredFolder,
    pub native_choose_folder_request: NativeChooseFolderRequest,
    pub native_folder_outcome: NativeFolderOutcome,
    pub folder_request_lookup: FolderRequestLookup,
    pub revoke_folder: RevokeFolder,
    pub reserve_upload: ReserveUpload,
    pub file_upload: FileUpload,
    pub upload_page: UploadPage,
    pub create_import_run: CreateImportRun,
    pub document_action_request: DocumentActionRequest,
    pub document_preparation: DocumentPreparation,
    pub save_document_draft: SaveDocumentDraft,
    pub document_draft: DocumentDraft,
    pub document_draft_snapshot: DocumentDraftSnapshot,
    pub document_draft_save: DocumentDraftSave,
    pub native_save_request: NativeSaveRequest,
    pub native_save_outcome: NativeSaveOutcome,
    pub download_file: DownloadFile,
    pub download_metadata: DownloadMetadata,
    pub create_restore_run: CreateRestoreRun,
    pub file_stat: FileStat,
    pub remove_file: RemoveFile,
    pub rename_file: RenameFile,
    pub workspace: Workspace,
    pub folder_grant: FolderGrant,
    pub file_location: FileLocation,
    pub write_text_file: WriteTextFile,
    pub directory_listing: DirectoryListing,
    pub text_file: TextFile,
    pub file_change: FileChange,
    pub reconcile_file: ReconcileFile,
    pub branch_order: BranchOrder,
    pub order_history: OrderHistory,
    pub order_export: OrderExport,
    pub activity_page: ActivityPage,
    pub search_page: SearchPage,
    pub create_provider: CreateProvider,
    pub provider_settings: ProviderSettingsContract,
    pub library_config: LibraryConfigContract,
    pub codex_accounts: CodexAccountsContract,
    pub project: Project,
    pub create_project: CreateProject,
    pub order: Order,
    pub create_order: CreateOrder,
    pub save_draft: SaveDraft,
    pub provider: ProviderProfile,
    pub create_run: CreateRun,
    pub run: RunRecord,
    pub event: RunEvent,
    pub message: Message,
    pub detail: RunDetail,
    pub resolve_approval: ResolveApproval,
    pub submit_input: SubmitInput,
    pub preflight: Preflight,
    pub error: ApiError,
    pub health: Health,
}
