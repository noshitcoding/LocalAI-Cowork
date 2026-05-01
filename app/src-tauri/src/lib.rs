mod artifact_pipeline;
mod audit;
mod claude_code_bridge;
mod cowork_features;
mod crew_python_bridge;
mod db;
mod file_safety;
mod file_watch;
mod insights;
mod mcp;
mod memory_engine;
mod ollama;
mod process_manager;
mod scheduler;
mod skill_engine;
mod terminal_backends;
mod worker_sandbox;

use claude_code_bridge::ClaudeCodeBridge;
use crew_python_bridge::{
  crew_runtime_bootstrap,
  crew_runtime_execute_request,
  crew_runtime_status,
  crew_runtime_validate_definition,
  CrewPythonBridge,
};
use db::Database;
use futures_util::future::join_all;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use mcp::{
  call_tool,
  probe_server,
  runtime_call_tool,
  runtime_has_server,
  runtime_list_servers,
  runtime_probe_server,
  runtime_restart_server,
  runtime_start_server,
  runtime_stop_server,
  McpCallRequest,
  McpError,
  McpRuntimeServerStatus,
  McpServerRequest,
};
use reqwest::{Method, StatusCode};
use ollama::{
  chat_turn as chat_turn_internal,
  chat_turn_stream as chat_turn_stream_internal,
  check_health,
  generate_plan as generate_plan_internal,
  ChatMessage,
  ChatStreamChunkPayload,
  ChatToolDef,
  OllamaToolCall,
  OllamaConfig,
  OllamaError,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Instant;
use std::time::Duration;
use tauri::{Emitter, Manager};
use url::Url;

const LOCAL_DOCS_MCP_COMMAND: &str = "open-cowork-docs-mcp";
const LOCAL_SCREENSHOT_MCP_COMMAND: &str = "open-cowork-screenshot-mcp";
const SCREENSHOT_DATA_URL_PREFIX: &str = "data:image/png;base64,";
const SCREENSHOT_REUSE_WINDOW_MS: i64 = 20_000;
const POLICY_FLAG_STRICT: &str = "strictPolicyEnforcement";
const POLICY_FLAG_TOOL_DISPATCHER: &str = "allowToolDispatcher";
const POLICY_FLAG_MCP: &str = "allowMcpToolCalls";
const POLICY_FLAG_WEB_FETCH: &str = "allowWebFetch";
const POLICY_FLAG_FILE_READ: &str = "allowFileReadExtraction";
const POLICY_FLAG_AUTO_COMPACT: &str = "autoCompactLongContext";
const POLICY_FLAG_SHELL_EXECUTION: &str = "allowShellExecution";
const POLICY_FLAG_WEB_SEARCH: &str = "allowWebSearch";
const MAX_CREW_DELEGATION_DEPTH: usize = 4;
const DEFAULT_POLICY_ENABLED_TOOL_IDS: &[&str] = &[
  "bash",
  "read_file",
  "edit_file",
  "create_directory",
  "move_path",
  "copy_path",
  "glob",
  "grep",
  "web_fetch",
  "web_search",
  "todo",
  "ask_user",
  "mcp",
];

#[derive(Default)]
struct WatchRegistry {
  watchers: Mutex<HashMap<String, RecommendedWatcher>>,
}

#[derive(Default)]
struct CrewExecutionRegistry {
  canceled: Mutex<HashSet<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScreenshotDisplayRegion {
  x: i32,
  y: i32,
  width: i32,
  height: i32,
}

#[derive(Debug, Clone)]
struct ScreenshotCacheEntry {
  display_index: i64,
  region_key: String,
  path: String,
  mime_type: String,
  base64_image: String,
  captured_at_ms: i64,
  display_info: Value,
}

#[derive(Debug, Default)]
struct ScreenshotCacheState {
  last_entry: Option<ScreenshotCacheEntry>,
  request_counts: HashMap<String, u32>,
}

static SCREENSHOT_CACHE: OnceLock<Mutex<ScreenshotCacheState>> = OnceLock::new();

fn screenshot_cache() -> &'static Mutex<ScreenshotCacheState> {
  SCREENSHOT_CACHE.get_or_init(|| Mutex::new(ScreenshotCacheState::default()))
}

// -- Request/Response types -------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanRequest {
  prompt: String,
  config: Option<OllamaConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatTurnRequest {
  prompt: String,
  history: Vec<ChatMessage>,
  config: Option<OllamaConfig>,
  stream_id: Option<String>,
  tools: Option<Vec<ChatToolDef>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebFetchRequest {
  url: String,
  max_chars: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebSearchRequest {
  query: String,
  max_results: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WebSearchResultItem {
  title: String,
  url: String,
  snippet: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WebSearchResponse {
  query: String,
  results: Vec<WebSearchResultItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecCommandRequest {
  command: String,
  cwd: Option<String>,
  timeout_ms: Option<u64>,
  stream_id: Option<String>,
  retry_count: Option<u32>,
  retry_backoff_ms: Option<u64>,
  run_id: Option<String>,
  backend_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExecCommandResponse {
  stdout: String,
  stderr: String,
  exit_code: Option<i32>,
  current_cwd: Option<String>,
  timed_out: bool,
  duration_ms: u64,
  attempts: u32,
  normalized_status: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DesktopLaunchRequest {
  path: String,
  args: Option<Vec<String>>,
  cwd: Option<String>,
  initial_delay_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DesktopWindowRequest {
  title: Option<String>,
  process_name: Option<String>,
  process_id: Option<u32>,
  exact_match: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DesktopClickRequest {
  x: i32,
  y: i32,
  button: Option<String>,
  double_click: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DesktopMoveMouseRequest {
  x: i32,
  y: i32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DesktopTypeRequest {
  text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DesktopKeypressRequest {
  keys: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DesktopScrollRequest {
  x: Option<i32>,
  y: Option<i32>,
  scroll_y: i32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DesktopWindowInfo {
  title: String,
  process_id: u32,
  process_name: String,
  handle: String,
  x: i32,
  y: i32,
  width: i32,
  height: i32,
  is_foreground: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DesktopDisplayInfo {
  primary: bool,
  x: i32,
  y: i32,
  width: i32,
  height: i32,
  device_name: String,
  #[serde(default)]
  scale_factor: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DesktopLaunchResponse {
  pid: u32,
  path: String,
  args: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DesktopActionResponse {
  ok: bool,
  action: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DesktopScreenshotResponse {
  data_url: String,
  width: i32,
  height: i32,
  x: i32,
  y: i32,
  primary: bool,
  device_name: String,
  #[serde(default)]
  scale_factor: Option<f64>,
  #[serde(default)]
  image_width: Option<i32>,
  #[serde(default)]
  image_height: Option<i32>,
  #[serde(default)]
  coordinate_overlay: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PolicyFlagsPayload {
  #[serde(default = "default_true")]
  strict_policy_enforcement: bool,
  #[serde(default = "default_true")]
  allow_tool_dispatcher: bool,
  #[serde(default = "default_true")]
  allow_mcp_tool_calls: bool,
  #[serde(default = "default_true")]
  allow_web_fetch: bool,
  #[serde(default = "default_true")]
  allow_file_read_extraction: bool,
  #[serde(default = "default_true")]
  auto_compact_long_context: bool,
  #[serde(default = "default_true")]
  allow_shell_execution: bool,
  #[serde(default = "default_true")]
  allow_web_search: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PolicySetRequest {
  flags: PolicyFlagsPayload,
  deny_rules: Vec<String>,
  #[serde(default = "default_policy_enabled_tool_ids_vec")]
  enabled_tool_ids: Vec<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PolicyStatePayload {
  flags: PolicyFlagsPayload,
  deny_rules: Vec<String>,
  enabled_tool_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EngineRunCreateRequest {
  id: String,
  parent_run_id: Option<String>,
  thread_id: Option<String>,
  session_id: Option<String>,
  title: String,
  input_summary: Option<String>,
  status: Option<String>,
  phase: Option<String>,
  cwd: Option<String>,
  model: Option<String>,
  provider: Option<String>,
  retry_count: Option<i32>,
  resumed_from_run_id: Option<String>,
  checkpoint_json: Option<String>,
  metadata_json: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EngineRunUpdateRequest {
  id: String,
  status: Option<String>,
  phase: Option<String>,
  checkpoint_json: Option<String>,
  result_summary: Option<String>,
  error: Option<String>,
  metadata_json: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EngineRunCheckpointRequest {
  run_id: String,
  label: String,
  snapshot_json: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeInstructionUpsertRequest {
  id: String,
  scope_type: String,
  scope_ref: Option<String>,
  title: String,
  content: String,
  enabled: Option<bool>,
  priority: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerSandboxCreateRequest {
  id: String,
  run_id: String,
  parent_run_id: Option<String>,
  backend_id: Option<String>,
  source_cwd: String,
  mode: Option<String>,
  allow_file_read: Option<bool>,
  allow_file_write: Option<bool>,
  allow_shell_execution: Option<bool>,
  allow_web_fetch: Option<bool>,
  allow_web_search: Option<bool>,
  allow_mcp: Option<bool>,
  env_json: Option<String>,
  metadata_json: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerSandboxUpdateRequest {
  id: String,
  status: Option<String>,
  metadata_json: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PolicyEvaluateRequest {
  tool: String,
  target: String,
  requested_flag: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PolicyEvaluateResponse {
  allowed: bool,
  reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectorReachabilityRequest {
  key: String,
  label: Option<String>,
  api_key: Option<String>,
  webhook_url: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectorReachabilityResponse {
  reachable: bool,
  status: Option<u16>,
  message: String,
  checked_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CrewProviderHealthCheckRequest {
  provider_kind: String,
  base_url: String,
  api_key: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CrewProviderHealthCheckResponse {
  reachable: bool,
  status: Option<u16>,
  endpoint: String,
  message: String,
  checked_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CrewProviderModelsRequest {
  provider_kind: String,
  base_url: String,
  api_key: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CrewProviderModelsResponse {
  endpoint: String,
  models: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModelsResponse {
  data: Vec<OpenAiModelRow>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModelRow {
  id: Option<String>,
  #[serde(default)]
  name: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadRow {
  id: String,
  title: String,
  created_at: String,
  updated_at: String,
  provider_settings_json: Option<String>,
  permission_config_json: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageRow {
  id: String,
  role: String,
  content: String,
  timestamp: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeletedMessagesResponse {
  deleted_count: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskRow {
  id: String,
  title: String,
  prompt: String,
  status: String,
  thread_id: Option<String>,
  created_at: String,
  updated_at: String,
  error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StepRow {
  id: String,
  idx: i32,
  title: String,
  state: String,
  requires_approval: bool,
  risk_level: String,
  output: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactVersionRow {
  id: String,
  run_id: Option<String>,
  label: Option<String>,
  source_path: String,
  format: String,
  size_bytes: i64,
  summary: String,
  preview: String,
  metadata: Value,
  created_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactExportRow {
  id: String,
  artifact_version_id: String,
  export_format: String,
  target_path: String,
  size_bytes: i64,
  created_at: String,
  source_path: String,
  run_id: Option<String>,
  label: Option<String>,
  source_format: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportedAttachmentRow {
  original_path: String,
  imported_path: String,
  file_name: String,
  size_bytes: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExtractTextLimitedResponse {
  text: String,
  chars: usize,
  truncated: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FsAttachmentMetadataEntry {
  path: String,
  file_name: String,
  extension: Option<String>,
  language: Option<String>,
  size_bytes: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FsAttachmentMetadataResponse {
  root_path: String,
  root_kind: String,
  total_files: usize,
  returned_files: usize,
  truncated: bool,
  files: Vec<FsAttachmentMetadataEntry>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WebFetchResponse {
  url: String,
  status: u16,
  ok: bool,
  title: Option<String>,
  content: String,
  truncated: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScheduledTaskUpsertRequest {
  id: String,
  name: String,
  prompt: String,
  schedule_expr: String,
  task_kind: Option<String>,
  crew_id: Option<String>,
  crew_snapshot_json: Option<String>,
  model_config_json: Option<String>,
  priority: Option<i64>,
  depends_on_task_ids: Option<Vec<String>>,
  active: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScheduledTaskToggleRequest {
  id: String,
  active: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScheduledTaskRow {
  id: String,
  name: String,
  prompt: String,
  schedule_expr: String,
  task_kind: String,
  crew_id: Option<String>,
  crew_snapshot_json: Option<String>,
  model_config_json: Option<String>,
  priority: i64,
  depends_on_task_ids: Vec<String>,
  active: bool,
  last_run_at: Option<String>,
  next_run_at: Option<String>,
  created_at: String,
  updated_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScheduledRunRow {
  id: String,
  task_id: String,
  status: String,
  started_at: String,
  finished_at: Option<String>,
  result: Option<String>,
  error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PipelineExecuteRequest {
  id: String,
  config: Option<OllamaConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PipelineStepDefinition {
  tool: Option<String>,
  prompt: Option<String>,
  args: Option<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PipelineExecutionStepResult {
  step: i32,
  tool: String,
  result: String,
  success: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PipelineExecutionResponse {
  pipeline_id: String,
  status: String,
  step_results: Vec<PipelineExecutionStepResult>,
  error: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct CrewExecuteAgentRequest {
  id: String,
  name: String,
  role: String,
  goal: String,
  backstory: String,
  #[serde(default)]
  skills_markdown: String,
  personality_id: Option<String>,
  model_override: Option<String>,
  provider_kind: Option<String>,
  #[serde(default)]
  tools: Vec<String>,
  #[serde(default)]
  mcp_server_names: Vec<String>,
  #[serde(default = "default_crew_agent_enabled")]
  enabled: bool,
  allow_delegation: bool,
  verbose: bool,
  max_iterations: i32,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct CrewExecuteTaskRequest {
  id: String,
  description: String,
  expected_output: String,
  agent_id: String,
  context: Vec<String>,
  dependencies: Vec<String>,
  async_execution: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct CrewExecuteRequest {
  id: String,
  name: String,
  description: String,
  #[serde(default)]
  execution_guidelines: String,
  #[serde(default = "default_crew_output_mode")]
  output_mode: String,
  #[serde(default)]
  stop_on_failure: bool,
  #[serde(default)]
  retry_count: i32,
  #[serde(default = "default_manager_review_enabled")]
  manager_review_enabled: bool,
  #[serde(default)]
  manager_review_guidelines: String,
  #[serde(default = "default_share_all_task_outputs")]
  share_all_task_outputs: bool,
  #[serde(default)]
  shared_output_char_limit: i32,
  process: String,
  manager_agent_id: Option<String>,
  verbose: bool,
  max_rpm: i32,
  max_parallel_tasks: Option<i32>,
  agents: Vec<CrewExecuteAgentRequest>,
  tasks: Vec<CrewExecuteTaskRequest>,
  config: Option<OllamaConfig>,
  #[serde(default)]
  provider_configs: CrewProviderConfigsRequest,
  #[serde(default)]
  cwd: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct CrewProviderConfigsRequest {
  #[serde(default)]
  open_ai_compatible: Option<CrewExternalProviderConfigRequest>,
  #[serde(default)]
  open_router: Option<CrewExternalProviderConfigRequest>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct CrewExternalProviderConfigRequest {
  base_url: String,
  model: String,
  #[serde(default)]
  api_key: String,
  timeout_ms: u64,
}

#[derive(Debug, Clone)]
enum CrewTaskProviderConfig {
  Ollama(Option<OllamaConfig>),
  OpenAiCompatible(CrewExternalProviderConfigRequest),
  OpenRouter(CrewExternalProviderConfigRequest),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CrewStopRequest {
  crew_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CrewExecutionLogRow {
  id: String,
  crew_id: String,
  agent_id: String,
  task_id: String,
  action: String,
  result: String,
  timestamp: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CrewTaskExecutionRow {
  task_id: String,
  agent_id: String,
  status: String,
  output: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CrewExecutionResponse {
  crew_id: String,
  status: String,
  task_results: Vec<CrewTaskExecutionRow>,
  logs: Vec<CrewExecutionLogRow>,
  error: Option<String>,
}

#[derive(Debug, Clone)]
struct CrewToolContext {
  crew_id: String,
  agent_id: String,
  task_id: String,
  cwd: Option<String>,
  requested_tool_ids: Vec<String>,
  mcp_server_names: HashSet<String>,
  request_snapshot: CrewExecuteRequest,
  delegation_enabled: bool,
  delegation_depth: usize,
}

#[derive(Debug)]
struct CrewChatExecutionResult {
  assistant_message: String,
  logs: Vec<CrewExecutionLogRow>,
}

#[derive(Debug)]
struct CrewChatExecutionError {
  message: String,
  logs: Vec<CrewExecutionLogRow>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CrewRunHistoryRow {
  id: String,
  crew_id: String,
  crew_name: String,
  process: String,
  status: String,
  manager_agent_id: Option<String>,
  error: Option<String>,
  started_at: String,
  finished_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CrewDefinitionUpsertRequest {
  id: String,
  name: String,
  description: Option<String>,
  definition_json: String,
  flow_json: Option<String>,
  change_summary: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CrewRoleBindingUpsertRequest {
  id: String,
  scope_type: String,
  scope_ref: Option<String>,
  role: String,
  subject: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CrewApprovalCreateRequest {
  id: String,
  crew_id: Option<String>,
  run_id: Option<String>,
  approval_type: String,
  scope_ref: Option<String>,
  status: Option<String>,
  requested_by: Option<String>,
  payload_json: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CrewApprovalResolveRequest {
  id: String,
  status: String,
  resolved_by: Option<String>,
  resolution_note: Option<String>,
}

fn default_crew_agent_enabled() -> bool {
  true
}

fn default_crew_output_mode() -> String {
  "standard".to_string()
}

fn default_manager_review_enabled() -> bool {
  true
}

fn default_share_all_task_outputs() -> bool {
  true
}

fn crew_agent_can_delegate(request: &CrewExecuteRequest, agent: &CrewExecuteAgentRequest) -> bool {
  agent.allow_delegation
    || (
      request.process.eq_ignore_ascii_case("hierarchical")
        && request.manager_agent_id.as_deref() == Some(agent.id.as_str())
    )
}

fn build_effective_crew_tool_ids(request: &CrewExecuteRequest, agent: &CrewExecuteAgentRequest) -> Vec<String> {
  let mut tool_ids = normalize_policy_enabled_tool_ids(&agent.tools);
  if crew_agent_can_delegate(request, agent) && !tool_ids.iter().any(|tool_id| tool_id == "delegate_task") {
    tool_ids.push("delegate_task".to_string());
  }
  tool_ids
}

#[derive(Debug, Serialize)]
struct OpenAiLikeChatMessage {
  role: String,
  content: String,
}

#[derive(Debug, Serialize)]
struct OpenAiLikeChatRequest {
  model: String,
  messages: Vec<OpenAiLikeChatMessage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiLikeChatResponse {
  choices: Vec<OpenAiLikeChatChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAiLikeChatChoice {
  message: OpenAiLikeChatChoiceMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAiLikeChatChoiceMessage {
  content: Option<String>,
}

fn build_openai_like_messages(history: &[ChatMessage], prompt: &str) -> Vec<OpenAiLikeChatMessage> {
  let mut messages = history
    .iter()
    .map(|message| {
      let role = match message.role.as_str() {
        "system" => "system",
        "assistant" => "assistant",
        _ => "user",
      };
      let content = if role == "user" && message.role != "user" {
        format!("[{}]\n{}", message.role, message.content)
      } else {
        message.content.clone()
      };
      OpenAiLikeChatMessage {
        role: role.to_string(),
        content,
      }
    })
    .collect::<Vec<_>>();

  messages.push(OpenAiLikeChatMessage {
    role: "user".to_string(),
    content: prompt.to_string(),
  });

  messages
}

async fn chat_turn_openai_like(
  config: CrewExternalProviderConfigRequest,
  prompt: String,
  history: Vec<ChatMessage>,
  provider_label: &str,
) -> Result<String, String> {
  if config.api_key.trim().is_empty() {
    return Err(format!("{} API-Key fehlt", provider_label));
  }

  if config.model.trim().is_empty() {
    return Err(format!("{} Modell fehlt", provider_label));
  }

  let base_url = config.base_url.trim();
  if base_url.is_empty() {
    return Err(format!("{} Endpoint fehlt", provider_label));
  }

  let endpoint = if base_url.trim_end_matches('/').ends_with("/chat/completions") {
    base_url.trim_end_matches('/').to_string()
  } else {
    format!("{}/chat/completions", base_url.trim_end_matches('/'))
  };

  Url::parse(&endpoint).map_err(|error| format!("{} Endpoint ungueltig: {}", provider_label, error))?;

  let client = reqwest::Client::builder()
    .timeout(Duration::from_millis(config.timeout_ms.max(1_000)))
    .build()
    .map_err(|error| error.to_string())?;

  let payload = OpenAiLikeChatRequest {
    model: config.model.trim().to_string(),
    messages: build_openai_like_messages(&history, &prompt),
  };

  let mut request = client
    .post(&endpoint)
    .header("Authorization", format!("Bearer {}", config.api_key.trim()))
    .header("Content-Type", "application/json")
    .json(&payload);

  if provider_label.eq_ignore_ascii_case("OpenRouter") {
    request = request
      .header("HTTP-Referer", "https://open-cowork.local")
      .header("X-Title", "Open Cowork");
  }

  let response = request.send().await.map_err(|error| format!("{} Request fehlgeschlagen: {}", provider_label, error))?;
  let status = response.status();
  if !status.is_success() {
    let body = response.text().await.unwrap_or_default();
    let excerpt: String = body.chars().take(400).collect();
    return Err(format!("{} antwortete mit {}: {}", provider_label, status, excerpt));
  }

  let payload: OpenAiLikeChatResponse = response
    .json()
    .await
    .map_err(|error| format!("{} Antwort konnte nicht gelesen werden: {}", provider_label, error))?;

  let content = payload
    .choices
    .into_iter()
    .next()
    .and_then(|choice| choice.message.content)
    .unwrap_or_default();

  if content.trim().is_empty() {
    return Err(format!("{} lieferte keine Antwort", provider_label));
  }

  Ok(content)
}

async fn execute_crew_chat(
  app: &tauri::AppHandle,
  database: &Arc<Database>,
  policy: &PolicyStatePayload,
  task_config: CrewTaskProviderConfig,
  prompt: String,
  history: Vec<ChatMessage>,
  tool_context: Option<&CrewToolContext>,
  max_iterations: usize,
) -> Result<CrewChatExecutionResult, CrewChatExecutionError> {
  match task_config {
    CrewTaskProviderConfig::Ollama(config) => {
      let tool_defs = tool_context
        .map(|context| build_crew_chat_tool_defs(policy, context))
        .unwrap_or_default();

      if tool_defs.is_empty() {
        return chat_turn_internal(config, prompt, history, vec![])
          .await
          .map(|response| CrewChatExecutionResult {
            assistant_message: response.assistant_message,
            logs: Vec::new(),
          })
          .map_err(|error| CrewChatExecutionError {
            message: map_ollama_error(error),
            logs: Vec::new(),
          });
      }

      let context = tool_context.expect("tool context missing for Ollama tool loop");
      let mut current_history = history;
      let mut current_prompt = prompt;
      let mut logs = Vec::new();
      let iteration_limit = max_iterations.max(1);

      for _ in 0..iteration_limit {
        let response = match chat_turn_internal(
          config.clone(),
          current_prompt.clone(),
          current_history.clone(),
          tool_defs.clone(),
        )
        .await
        {
          Ok(response) => response,
          Err(error) => {
            return Err(CrewChatExecutionError {
              message: map_ollama_error(error),
              logs,
            });
          }
        };

        let assistant_text = response.assistant_message.trim().to_string();
        if response.tool_calls.is_empty() {
          if assistant_text.is_empty() {
            return Err(CrewChatExecutionError {
              message: "Crew-Modell lieferte keinen finalen Antworttext".to_string(),
              logs,
            });
          }

          return Ok(CrewChatExecutionResult {
            assistant_message: assistant_text,
            logs,
          });
        }

        let tool_summary = response
          .tool_calls
          .iter()
          .map(format_crew_tool_call_summary)
          .collect::<Vec<_>>()
          .join("\n");
        let assistant_history_entry = if assistant_text.is_empty() {
          format!("Tool-Aufrufe:\n{}", tool_summary)
        } else {
          format!("{}\n\nTool-Aufrufe:\n{}", assistant_text, tool_summary)
        };
        current_history.push(ChatMessage {
          role: "assistant".to_string(),
          content: assistant_history_entry,
        });

        for tool_call in &response.tool_calls {
          let (tool_result, tool_logs) = execute_crew_tool_call(app, database, policy, context, tool_call).await;
          logs.extend(tool_logs);
          current_history.push(ChatMessage {
            role: "tool".to_string(),
            content: format!(
              "Tool {} Ergebnis:\n{}",
              canonical_policy_tool_id(&tool_call.function.name),
              tool_result
            ),
          });
        }

        current_prompt = "Nutze die Tool-Ergebnisse im Verlauf. Rufe nur dann weitere Tools auf, wenn sie fuer die Task-Antwort wirklich noetig sind. Wenn genug Informationen vorliegen, liefere jetzt die finale Antwort fuer den Task.".to_string();
      }

      Err(CrewChatExecutionError {
        message: "Crew-Tool-Loop hat das Iterationslimit erreicht, ohne eine finale Antwort zu liefern".to_string(),
        logs,
      })
    }
    CrewTaskProviderConfig::OpenAiCompatible(config) => chat_turn_openai_like(
      config,
      prompt,
      history,
      "OpenAI-kompatibler Provider",
    )
    .await
    .map(|assistant_message| CrewChatExecutionResult {
      assistant_message,
      logs: Vec::new(),
    })
    .map_err(|message| CrewChatExecutionError {
      message,
      logs: Vec::new(),
    }),
    CrewTaskProviderConfig::OpenRouter(config) => chat_turn_openai_like(config, prompt, history, "OpenRouter")
      .await
      .map(|assistant_message| CrewChatExecutionResult {
        assistant_message,
        logs: Vec::new(),
      })
      .map_err(|message| CrewChatExecutionError {
        message,
        logs: Vec::new(),
      }),
  }
}

fn value_to_step_text(value: &Value) -> String {
  match value {
    Value::Null => String::new(),
    Value::String(text) => text.clone(),
    _ => serde_json::to_string(value).unwrap_or_else(|_| String::new()),
  }
}

fn find_gateway_context(tool_name: &str, gateways: &[db::ToolGatewayRow]) -> Option<String> {
  gateways
    .iter()
    .find(|entry| {
      entry.enabled
        && (entry.name.eq_ignore_ascii_case(tool_name) || entry.tool_type.eq_ignore_ascii_case(tool_name))
    })
    .map(|entry| format!(
      "Tool-Gateway: {} ({})\nKonfiguration: {}",
      entry.name,
      entry.tool_type,
      entry.config_json
    ))
}

async fn execute_pipeline_web_fetch(url: &str) -> Result<String, String> {
  let requested_url = url.trim();
  if requested_url.is_empty() {
    return Err("web_fetch benoetigt eine URL".to_string());
  }

  let client = reqwest::Client::builder()
    .timeout(Duration::from_secs(30))
    .build()
    .map_err(|err| err.to_string())?;
  let response = client
    .get(requested_url)
    .send()
    .await
    .map_err(|err| err.to_string())?;
  let status = response.status();
  let body = response.text().await.map_err(|err| err.to_string())?;
  let title = extract_html_title(&body).unwrap_or_else(|| "(ohne Titel)".to_string());
  let stripped = strip_html_like_content(&body);
  let content: String = stripped.trim().chars().take(4_000).collect();

  Ok(format!("URL: {}\nStatus: {}\nTitel: {}\n\n{}", requested_url, status.as_u16(), title, content))
}

async fn execute_pipeline_web_search(query: &str) -> Result<String, String> {
  let trimmed = query.trim();
  if trimmed.is_empty() {
    return Err("web_search benoetigt eine Suchanfrage".to_string());
  }

  let encoded_query = url::form_urlencoded::byte_serialize(trimmed.as_bytes()).collect::<String>();
  let search_url = format!("https://html.duckduckgo.com/html/?q={}", encoded_query);
  let client = reqwest::Client::builder()
    .timeout(Duration::from_secs(30))
    .build()
    .map_err(|err| err.to_string())?;
  let body = client
    .get(&search_url)
    .send()
    .await
    .map_err(|err| err.to_string())?
    .text()
    .await
    .map_err(|err| err.to_string())?;
  let results = parse_duckduckgo_results(&body, 5);

  Ok(results
    .iter()
    .enumerate()
    .map(|(index, item)| {
      let snippet = if item.snippet.is_empty() {
        String::new()
      } else {
        format!("\n{}", item.snippet)
      };
      format!("{}. {}\n{}{}", index + 1, item.title, item.url, snippet)
    })
    .collect::<Vec<_>>()
    .join("\n\n"))
}

async fn execute_pipeline_llm_step(
  config: Option<OllamaConfig>,
  tool_name: &str,
  prompt: &str,
  previous_context: &str,
  gateway_context: Option<String>,
) -> Result<String, String> {
  let full_prompt = [
    if previous_context.trim().is_empty() {
      None
    } else {
      Some(format!("Bisheriger Pipeline-Kontext:\n{}", previous_context))
    },
    gateway_context,
    Some(format!("Tool: {}\nAufgabe:\n{}", tool_name, prompt)),
  ]
  .into_iter()
  .flatten()
  .collect::<Vec<_>>()
  .join("\n\n");

  if tool_name.eq_ignore_ascii_case("plan") || tool_name.eq_ignore_ascii_case("planner") {
    return generate_plan_internal(config, full_prompt)
      .await
      .map(|response| response.raw_response)
      .map_err(map_ollama_error);
  }

  chat_turn_internal(config, full_prompt, vec![], vec![])
    .await
    .map(|response| response.assistant_message)
    .map_err(map_ollama_error)
}

fn build_crew_tool_context(
  request: &CrewExecuteRequest,
  agent: &CrewExecuteAgentRequest,
  task_id: &str,
  database: &Arc<Database>,
  delegation_depth: usize,
) -> Result<CrewToolContext, String> {
  let requested_tool_ids = build_effective_crew_tool_ids(request, agent);
  let requested_cwd = request.cwd.as_deref().map(str::trim).filter(|value| !value.is_empty());

  if let Some(cwd) = requested_cwd {
    if !Path::new(cwd).is_absolute() {
      return Err("Crew-Arbeitsverzeichnis muss absolut sein".to_string());
    }
  }

  let cwd = ensure_run_cwd(database, None, requested_cwd)?;

  Ok(CrewToolContext {
    crew_id: request.id.clone(),
    agent_id: agent.id.clone(),
    task_id: task_id.to_string(),
    cwd,
    requested_tool_ids,
    mcp_server_names: agent.mcp_server_names.iter().cloned().collect(),
    request_snapshot: request.clone(),
    delegation_enabled: crew_agent_can_delegate(request, agent),
    delegation_depth,
  })
}

fn crew_active_delegate_targets(context: &CrewToolContext) -> Vec<CrewExecuteAgentRequest> {
  context
    .request_snapshot
    .agents
    .iter()
    .filter(|agent| agent.enabled && agent.id != context.agent_id)
    .cloned()
    .collect()
}

fn crew_tool_allowed_by_flags(policy: &PolicyStatePayload, tool_id: &str) -> bool {
  match tool_id {
    "bash" => policy.flags.allow_shell_execution,
    "read_file" => policy.flags.allow_file_read_extraction,
    "web_fetch" => policy.flags.allow_web_fetch,
    "web_search" => policy.flags.allow_web_search,
    "mcp" => policy.flags.allow_mcp_tool_calls,
    _ => true,
  }
}

fn crew_tool_available_for_agent(policy: &PolicyStatePayload, context: &CrewToolContext, tool_id: &str) -> bool {
  if tool_id == "delegate_task" {
    return context.delegation_enabled
      && context.delegation_depth < MAX_CREW_DELEGATION_DEPTH
      && !crew_active_delegate_targets(context).is_empty();
  }

  if !context.requested_tool_ids.iter().any(|requested| requested == tool_id) {
    return false;
  }

  if !crew_tool_allowed_by_flags(policy, tool_id) {
    return false;
  }

  if !policy.flags.strict_policy_enforcement {
    return true;
  }

  policy.enabled_tool_ids.iter().any(|enabled| enabled == tool_id)
}

fn build_crew_chat_tool_def(name: &str, description: &str, parameters: Value) -> ChatToolDef {
  ChatToolDef {
    kind: "function".to_string(),
    function: ollama::ChatToolFunctionDef {
      name: name.to_string(),
      description: description.to_string(),
      parameters,
    },
  }
}

fn build_crew_chat_tool_defs(policy: &PolicyStatePayload, context: &CrewToolContext) -> Vec<ChatToolDef> {
  let mut tools = Vec::new();

  if crew_tool_available_for_agent(policy, context, "bash") {
    tools.push(build_crew_chat_tool_def(
      "bash",
      "Fuehrt einen Shell-Befehl im freigegebenen Arbeitsverzeichnis aus.",
      serde_json::json!({
        "type": "object",
        "properties": {
          "command": { "type": "string" },
          "timeout_ms": { "type": "integer", "minimum": 500, "maximum": 120000 }
        },
        "required": ["command"]
      }),
    ));
  }

  if crew_tool_available_for_agent(policy, context, "read_file") {
    tools.push(build_crew_chat_tool_def(
      "read_file",
      "Liest eine Datei und gibt Zeilen mit Nummern zurueck.",
      serde_json::json!({
        "type": "object",
        "properties": {
          "file_path": { "type": "string" },
          "offset": { "type": "integer", "minimum": 0 },
          "limit": { "type": "integer", "minimum": 1, "maximum": 400 }
        },
        "required": ["file_path"]
      }),
    ));
  }

  if crew_tool_available_for_agent(policy, context, "edit_file") {
    tools.push(build_crew_chat_tool_def(
      "edit_file",
      "Ersetzt genau einen Textabschnitt in einer UTF-8-Textdatei.",
      serde_json::json!({
        "type": "object",
        "properties": {
          "file_path": { "type": "string" },
          "old_string": { "type": "string" },
          "new_string": { "type": "string" }
        },
        "required": ["file_path", "old_string", "new_string"]
      }),
    ));
  }

  if crew_tool_available_for_agent(policy, context, "create_directory") {
    tools.push(build_crew_chat_tool_def(
      "create_directory",
      "Erstellt einen Ordner innerhalb der erlaubten Pfade.",
      serde_json::json!({
        "type": "object",
        "properties": {
          "path": { "type": "string" }
        },
        "required": ["path"]
      }),
    ));
  }

  if crew_tool_available_for_agent(policy, context, "move_path") {
    tools.push(build_crew_chat_tool_def(
      "move_path",
      "Verschiebt oder benennt eine Datei oder einen Ordner um.",
      serde_json::json!({
        "type": "object",
        "properties": {
          "source_path": { "type": "string" },
          "destination_path": { "type": "string" },
          "overwrite": { "type": "boolean" }
        },
        "required": ["source_path", "destination_path"]
      }),
    ));
  }

  if crew_tool_available_for_agent(policy, context, "copy_path") {
    tools.push(build_crew_chat_tool_def(
      "copy_path",
      "Kopiert eine Datei oder einen Ordner.",
      serde_json::json!({
        "type": "object",
        "properties": {
          "source_path": { "type": "string" },
          "destination_path": { "type": "string" },
          "overwrite": { "type": "boolean" }
        },
        "required": ["source_path", "destination_path"]
      }),
    ));
  }

  if crew_tool_available_for_agent(policy, context, "glob") {
    tools.push(build_crew_chat_tool_def(
      "glob",
      "Sucht Dateien ueber ein einfaches Glob-Muster innerhalb des Arbeitsverzeichnisses oder eines angegebenen Basisordners.",
      serde_json::json!({
        "type": "object",
        "properties": {
          "pattern": { "type": "string" },
          "path": { "type": "string" },
          "limit": { "type": "integer", "minimum": 1, "maximum": 200 }
        },
        "required": ["pattern"]
      }),
    ));
  }

  if crew_tool_available_for_agent(policy, context, "grep") {
    tools.push(build_crew_chat_tool_def(
      "grep",
      "Fuehrt eine einfache Textsuche ohne Regex ueber Dateien aus.",
      serde_json::json!({
        "type": "object",
        "properties": {
          "pattern": { "type": "string" },
          "path": { "type": "string" },
          "include": { "type": "string" },
          "limit": { "type": "integer", "minimum": 1, "maximum": 200 }
        },
        "required": ["pattern"]
      }),
    ));
  }

  if crew_tool_available_for_agent(policy, context, "web_fetch") {
    tools.push(build_crew_chat_tool_def(
      "web_fetch",
      "Laedt den bereinigten Inhalt einer Webseite.",
      serde_json::json!({
        "type": "object",
        "properties": {
          "url": { "type": "string" },
          "max_chars": { "type": "integer", "minimum": 500, "maximum": 30000 }
        },
        "required": ["url"]
      }),
    ));
  }

  if crew_tool_available_for_agent(policy, context, "web_search") {
    tools.push(build_crew_chat_tool_def(
      "web_search",
      "Sucht im Web und liefert die wichtigsten Treffer.",
      serde_json::json!({
        "type": "object",
        "properties": {
          "query": { "type": "string" },
          "max_results": { "type": "integer", "minimum": 1, "maximum": 10 }
        },
        "required": ["query"]
      }),
    ));
  }

  if crew_tool_available_for_agent(policy, context, "todo") {
    tools.push(build_crew_chat_tool_def(
      "todo",
      "Verwaltet interne Todo-Eintraege ueber create, list und update.",
      serde_json::json!({
        "type": "object",
        "properties": {
          "action": { "type": "string", "enum": ["create", "list", "update"] },
          "task_id": { "type": "string" },
          "title": { "type": "string" },
          "prompt": { "type": "string" },
          "status": { "type": "string" },
          "limit": { "type": "integer", "minimum": 1, "maximum": 50 }
        },
        "required": ["action"]
      }),
    ));
  }

  let delegate_targets = crew_active_delegate_targets(context);
  if crew_tool_available_for_agent(policy, context, "delegate_task") {
    let delegation_hint = delegate_targets
      .iter()
      .map(|agent| format!("{} [{}] Rolle: {}", agent.name, agent.id, agent.role))
      .collect::<Vec<_>>()
      .join("; ");
    let agent_id_enum = delegate_targets
      .iter()
      .map(|agent| Value::String(agent.id.clone()))
      .collect::<Vec<_>>();
    let description = format!(
      "Delegiert einen klar abgegrenzten Unterauftrag an ein anderes aktives Crew-Mitglied. Verfuegbare Ziele: {}.",
      delegation_hint
    );
    tools.push(build_crew_chat_tool_def(
      "delegate_task",
      description.as_str(),
      serde_json::json!({
        "type": "object",
        "properties": {
          "agent_id": { "type": "string", "enum": agent_id_enum },
          "task_description": { "type": "string" },
          "expected_output": { "type": "string" },
          "context": {
            "type": "array",
            "items": { "type": "string" },
            "maxItems": 12
          }
        },
        "required": ["agent_id", "task_description"]
      }),
    ));
  }

  if crew_tool_available_for_agent(policy, context, "mcp") && !context.mcp_server_names.is_empty() {
    tools.push(build_crew_chat_tool_def(
      "mcp",
      "Ruft ein Tool auf einem freigegebenen und gestarteten MCP-Server auf.",
      serde_json::json!({
        "type": "object",
        "properties": {
          "server_name": { "type": "string" },
          "tool_name": { "type": "string" },
          "arguments": { "type": "object" }
        },
        "required": ["server_name", "tool_name"]
      }),
    ));
  }

  tools
}

fn build_crew_log_row(context: &CrewToolContext, action: String, result: String) -> CrewExecutionLogRow {
  CrewExecutionLogRow {
    id: uuid::Uuid::new_v4().to_string(),
    crew_id: context.crew_id.clone(),
    agent_id: context.agent_id.clone(),
    task_id: context.task_id.clone(),
    action,
    result,
    timestamp: chrono::Utc::now().timestamp_millis(),
  }
}

fn format_crew_tool_call_summary(tool_call: &OllamaToolCall) -> String {
  format!(
    "- {} {}",
    canonical_policy_tool_id(&tool_call.function.name),
    serde_json::to_string(&Value::Object(tool_call.function.arguments.clone()))
      .unwrap_or_else(|_| "{}".to_string())
  )
}

fn tool_arg_string(arguments: &serde_json::Map<String, Value>, names: &[&str]) -> Option<String> {
  names.iter().find_map(|name| {
    arguments
      .get(*name)
      .and_then(|value| value.as_str())
      .map(str::trim)
      .filter(|value| !value.is_empty())
      .map(|value| value.to_string())
  })
}

fn tool_arg_u64(arguments: &serde_json::Map<String, Value>, names: &[&str]) -> Option<u64> {
  names.iter().find_map(|name| arguments.get(*name).and_then(|value| value.as_u64()))
}

fn tool_arg_bool(arguments: &serde_json::Map<String, Value>, names: &[&str]) -> Option<bool> {
  names.iter().find_map(|name| arguments.get(*name).and_then(|value| value.as_bool()))
}

fn tool_arg_object(
  arguments: &serde_json::Map<String, Value>,
  names: &[&str],
) -> Option<serde_json::Map<String, Value>> {
  names.iter().find_map(|name| {
    arguments
      .get(*name)
      .and_then(|value| value.as_object())
      .cloned()
  })
}

fn tool_arg_string_array(arguments: &serde_json::Map<String, Value>, names: &[&str]) -> Vec<String> {
  names
    .iter()
    .find_map(|name| {
      arguments
        .get(*name)
        .and_then(|value| value.as_array())
        .map(|values| {
          values
            .iter()
            .filter_map(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
        })
    })
    .unwrap_or_default()
}

fn resolve_crew_tool_path(path: &str, cwd: Option<&str>) -> Result<String, String> {
  let trimmed = path.trim();
  if trimmed.is_empty() {
    return Err("Pfad darf nicht leer sein".to_string());
  }

  let candidate = PathBuf::from(trimmed);
  if candidate.is_absolute() {
    return Ok(candidate.display().to_string());
  }

  let base_cwd = cwd.ok_or_else(|| format!("Relativer Pfad ohne Arbeitsverzeichnis nicht moeglich: {}", trimmed))?;
  Ok(Path::new(base_cwd).join(candidate).display().to_string())
}

fn normalize_path_for_match(path: &Path) -> String {
  path.display().to_string().replace('\\', "/")
}

fn collect_crew_tool_paths(root: &Path, max_entries: usize, entries: &mut Vec<PathBuf>) -> Result<(), String> {
  if entries.len() >= max_entries {
    return Ok(());
  }

  if root.is_file() {
    entries.push(root.to_path_buf());
    return Ok(());
  }

  let read_dir = fs::read_dir(root).map_err(|err| err.to_string())?;
  for child in read_dir {
    let child = child.map_err(|err| err.to_string())?;
    let child_path = child.path();
    if child_path.is_dir() {
      collect_crew_tool_paths(child_path.as_path(), max_entries, entries)?;
    } else {
      entries.push(child_path);
    }

    if entries.len() >= max_entries {
      break;
    }
  }

  Ok(())
}

fn crew_glob_matches(pattern: &str, path: &Path, base_path: &Path) -> bool {
  let normalized_pattern = pattern.trim().replace('\\', "/");
  let absolute_text = normalize_path_for_match(path);
  let relative_text = path
    .strip_prefix(base_path)
    .ok()
    .map(normalize_path_for_match)
    .unwrap_or_else(|| absolute_text.clone());
  let file_name = path.file_name().and_then(|value| value.to_str()).unwrap_or_default();

  wildcard_match(&normalized_pattern, &relative_text)
    || wildcard_match(&normalized_pattern, &absolute_text)
    || wildcard_match(&normalized_pattern, file_name)
}

async fn execute_crew_tool_call(
  app: &tauri::AppHandle,
  database: &Arc<Database>,
  policy: &PolicyStatePayload,
  context: &CrewToolContext,
  tool_call: &OllamaToolCall,
) -> (String, Vec<CrewExecutionLogRow>) {
  let tool_id = canonical_policy_tool_id(&tool_call.function.name);
  let args_json = serde_json::to_string_pretty(&Value::Object(tool_call.function.arguments.clone()))
    .unwrap_or_else(|_| "{}".to_string());
  let mut logs = vec![build_crew_log_row(
    context,
    format!("Tool gestartet: {}", tool_id),
    truncate_chars(&args_json, 1_200),
  )];

  if tool_id == "delegate_task" {
    match execute_crew_delegate_tool_call(app, database, policy, context, &tool_call.function.arguments).await {
      Ok((result, extra_logs)) => {
        logs.extend(extra_logs);
        logs.push(build_crew_log_row(
          context,
          format!("Tool abgeschlossen: {}", tool_id),
          truncate_chars(&result, 1_200),
        ));
        return (result, logs);
      }
      Err((error, extra_logs)) => {
        logs.extend(extra_logs);
        logs.push(build_crew_log_row(
          context,
          format!("Tool fehlgeschlagen: {}", tool_id),
          truncate_chars(&error, 1_200),
        ));
        return (error, logs);
      }
    }
  }

  match execute_crew_tool_inner(app, database, policy, context, &tool_id, &tool_call.function.arguments).await {
    Ok(result) => {
      logs.push(build_crew_log_row(
        context,
        format!("Tool abgeschlossen: {}", tool_id),
        truncate_chars(&result, 1_200),
      ));
      (result, logs)
    }
    Err(error) => {
      let error_text = format!("Fehler: {}", error);
      logs.push(build_crew_log_row(
        context,
        format!("Tool fehlgeschlagen: {}", tool_id),
        truncate_chars(&error_text, 1_200),
      ));
      (error_text, logs)
    }
  }
}

async fn execute_crew_delegate_tool_call(
  app: &tauri::AppHandle,
  database: &Arc<Database>,
  policy: &PolicyStatePayload,
  context: &CrewToolContext,
  arguments: &serde_json::Map<String, Value>,
) -> Result<(String, Vec<CrewExecutionLogRow>), (String, Vec<CrewExecutionLogRow>)> {
  let mut logs = Vec::new();

  if !context.delegation_enabled {
    return Err(("Delegation ist fuer diesen Agenten nicht aktiviert".to_string(), logs));
  }

  if context.delegation_depth >= MAX_CREW_DELEGATION_DEPTH {
    return Err((
      format!("Maximale Delegationstiefe von {} erreicht", MAX_CREW_DELEGATION_DEPTH),
      logs,
    ));
  }

  let Some(delegator) = context
    .request_snapshot
    .agents
    .iter()
    .find(|agent| agent.id == context.agent_id)
    .cloned() else {
      return Err((
        format!("Delegierender Agent {} konnte nicht aufgeloest werden", context.agent_id),
        logs,
      ));
    };

  let available_targets = crew_active_delegate_targets(context);
  if available_targets.is_empty() {
    return Err(("Keine weiteren aktiven Crew-Mitglieder fuer Delegation verfuegbar".to_string(), logs));
  }

  let Some(target_agent_id) = tool_arg_string(arguments, &["agent_id", "target_agent_id", "delegate_to"]) else {
    return Err(("delegate_task benoetigt agent_id".to_string(), logs));
  };

  let Some(target_agent) = available_targets
    .iter()
    .find(|agent| agent.id == target_agent_id)
    .cloned() else {
      let candidate_ids = available_targets
        .iter()
        .map(|agent| format!("{} [{}]", agent.name, agent.id))
        .collect::<Vec<_>>()
        .join(", ");
      return Err((
        format!(
          "Delegationsziel {} ist nicht verfuegbar. Erlaubte Ziele: {}",
          target_agent_id,
          candidate_ids,
        ),
        logs,
      ));
    };

  let Some(task_description) = tool_arg_string(arguments, &["task_description", "description", "prompt"]) else {
    return Err(("delegate_task benoetigt task_description".to_string(), logs));
  };
  let expected_output = tool_arg_string(arguments, &["expected_output", "expectedOutput"]).unwrap_or_default();
  let mut context_lines = tool_arg_string_array(arguments, &["context", "context_lines", "notes"]);
  context_lines.push(format!(
    "Delegiert von {} [{}]. Bearbeite nur diesen Unterauftrag und liefere kein Gesamtfazit fuer die gesamte Crew.",
    delegator.name,
    delegator.id,
  ));

  let synthetic_task_id = format!("{}::delegate::{}", context.task_id, uuid::Uuid::new_v4().simple());
  logs.push(CrewExecutionLogRow {
    id: uuid::Uuid::new_v4().to_string(),
    crew_id: context.crew_id.clone(),
    agent_id: delegator.id.clone(),
    task_id: synthetic_task_id.clone(),
    action: format!("Delegation gestartet an {}", target_agent.id),
    result: truncate_chars(&task_description, 400),
    timestamp: chrono::Utc::now().timestamp_millis(),
  });

  let delegated_task = CrewExecuteTaskRequest {
    id: synthetic_task_id.clone(),
    description: task_description.clone(),
    expected_output,
    agent_id: target_agent.id.clone(),
    context: context_lines,
    dependencies: Vec::new(),
    async_execution: false,
  };

  let mut history = build_crew_task_history(&context.request_snapshot, &target_agent, &delegated_task, &HashMap::new());
  history.push(ChatMessage {
    role: "delegation".to_string(),
    content: format!(
      "Delegationsauftrag von {} [{}] an {} [{}]. Liefere nur den angeforderten Teilbeitrag in direkt nutzbarer Form.",
      delegator.name,
      delegator.id,
      target_agent.name,
      target_agent.id,
    ),
  });
  let prompt = build_crew_task_prompt(&context.request_snapshot, &delegated_task);

  let tool_context = match build_crew_tool_context(
    &context.request_snapshot,
    &target_agent,
    &synthetic_task_id,
    database,
    context.delegation_depth + 1,
  ) {
    Ok(tool_context) => tool_context,
    Err(error) => return Err((error, logs)),
  };

  let task_config = match resolve_crew_task_config(&context.request_snapshot, &target_agent) {
    Ok(task_config) => task_config,
    Err(error) => return Err((error, logs)),
  };

  let delegated_execution = Box::pin(execute_crew_chat(
    app,
    database,
    policy,
    task_config,
    prompt,
    history,
    Some(&tool_context),
    target_agent.max_iterations.max(1) as usize,
  ));

  match delegated_execution.await {
    Ok(response) => {
      logs.extend(response.logs);
      logs.push(CrewExecutionLogRow {
        id: uuid::Uuid::new_v4().to_string(),
        crew_id: context.crew_id.clone(),
        agent_id: target_agent.id.clone(),
        task_id: synthetic_task_id,
        action: format!("Delegation abgeschlossen: {}", target_agent.id),
        result: truncate_chars(&response.assistant_message, 600),
        timestamp: chrono::Utc::now().timestamp_millis(),
      });
      Ok((
        format!(
          "Delegiertes Ergebnis von {} [{}]:\n\n{}",
          target_agent.name,
          target_agent.id,
          response.assistant_message,
        ),
        logs,
      ))
    }
    Err(error) => {
      logs.extend(error.logs);
      logs.push(CrewExecutionLogRow {
        id: uuid::Uuid::new_v4().to_string(),
        crew_id: context.crew_id.clone(),
        agent_id: target_agent.id.clone(),
        task_id: synthetic_task_id,
        action: format!("Delegation fehlgeschlagen: {}", target_agent.id),
        result: truncate_chars(&error.message, 600),
        timestamp: chrono::Utc::now().timestamp_millis(),
      });
      Err((
        format!(
          "Delegation an {} [{}] fehlgeschlagen: {}",
          target_agent.name,
          target_agent.id,
          error.message,
        ),
        logs,
      ))
    }
  }
}

async fn execute_crew_tool_inner(
  app: &tauri::AppHandle,
  database: &Arc<Database>,
  policy: &PolicyStatePayload,
  context: &CrewToolContext,
  tool_id: &str,
  arguments: &serde_json::Map<String, Value>,
) -> Result<String, String> {
  if !context.requested_tool_ids.iter().any(|requested| requested == tool_id) {
    return Err(format!("Tool {} ist fuer diesen Agenten nicht freigegeben", tool_id));
  }

  match tool_id {
    "bash" => {
      let command_text = tool_arg_string(arguments, &["command", "cmd"])
        .ok_or_else(|| "bash benoetigt command".to_string())?;
      enforce_tool_policy(policy, "bash", &command_text, policy.flags.allow_shell_execution)?;
      enforce_shell_command_guard(database, None, &command_text, context.cwd.as_deref())?;
      let timeout_ms = tool_arg_u64(arguments, &["timeout_ms", "timeout"])
        .unwrap_or(20_000)
        .clamp(500, 120_000);
      let (shell_override, env_vars, runtime_mode) = resolve_exec_runtime(database, None, None)?;
      let response = run_command_once(
        app,
        None,
        &command_text,
        context.cwd.as_deref(),
        timeout_ms,
        shell_override.as_deref(),
        runtime_mode.as_deref(),
        &env_vars,
      )?;

      Ok(format!(
        "Status: {}\nExit-Code: {}\nCWD: {}\nTimed out: {}\n\nSTDOUT:\n{}\n\nSTDERR:\n{}",
        response.normalized_status,
        response.exit_code.map(|value| value.to_string()).unwrap_or_else(|| "none".to_string()),
        response.current_cwd.unwrap_or_else(|| context.cwd.clone().unwrap_or_default()),
        response.timed_out,
        response.stdout,
        response.stderr,
      ))
    }
    "read_file" => {
      let file_path = tool_arg_string(arguments, &["file_path", "path"])
        .ok_or_else(|| "read_file benoetigt file_path".to_string())?;
      let resolved_path = resolve_crew_tool_path(&file_path, context.cwd.as_deref())?;
      let canonical_target = ensure_run_file_access(database, None, &resolved_path, false)?;
      let target_text = canonical_target.display().to_string();
      enforce_tool_policy(policy, "read_file", &target_text, policy.flags.allow_file_read_extraction)?;

      let offset = tool_arg_u64(arguments, &["offset", "start_line", "startLine"]).unwrap_or(0) as usize;
      let limit = tool_arg_u64(arguments, &["limit", "max_lines", "maxLines"]).unwrap_or(120).clamp(1, 400) as usize;
      let text = artifact_pipeline::extract_text_for_llm(canonical_target.as_path())?;
      let lines = text.lines().collect::<Vec<_>>();
      let excerpt = lines
        .iter()
        .enumerate()
        .skip(offset)
        .take(limit)
        .map(|(index, line)| format!("{:>4}: {}", index + 1, line))
        .collect::<Vec<_>>();

      if excerpt.is_empty() {
        return Ok(format!("Datei: {}\nKeine Zeilen im angeforderten Bereich.", target_text));
      }

      let end_line = offset + excerpt.len();
      Ok(format!(
        "Datei: {}\nZeilen {} bis {} von {}\n\n{}",
        target_text,
        offset + 1,
        end_line,
        lines.len(),
        excerpt.join("\n"),
      ))
    }
    "edit_file" => {
      let file_path = tool_arg_string(arguments, &["file_path", "path"])
        .ok_or_else(|| "edit_file benoetigt file_path".to_string())?;
      let old_string = tool_arg_string(arguments, &["old_string", "oldText", "old_text"])
        .ok_or_else(|| "edit_file benoetigt old_string".to_string())?;
      let new_string = tool_arg_string(arguments, &["new_string", "newText", "new_text"])
        .ok_or_else(|| "edit_file benoetigt new_string".to_string())?;
      let resolved_path = resolve_crew_tool_path(&file_path, context.cwd.as_deref())?;
      let canonical_target = ensure_run_file_access(database, None, &resolved_path, true)?;
      let target_text = canonical_target.display().to_string();
      enforce_tool_policy(policy, "edit_file", &target_text, true)?;

      let current_content = fs::read_to_string(&canonical_target).map_err(|err| err.to_string())?;
      let occurrences = current_content.matches(&old_string).count();
      if occurrences == 0 {
        return Err("old_string wurde nicht gefunden".to_string());
      }
      if occurrences > 1 {
        return Err("old_string ist nicht eindeutig und kommt mehrfach vor".to_string());
      }

      let next_content = current_content.replacen(&old_string, &new_string, 1);
      let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
      let response = file_safety::write_text_file(&app_data_dir, &canonical_target, &next_content, true)?;
      let details = file_safety::write_file_audit_details(
        &response.path,
        response.backup_path.as_deref(),
        response.bytes_written,
      );
      let _ = audit::append_audit_event(app_data_dir, "file_safety", "write_text_file", Some(details));

      Ok(format!("Datei aktualisiert: {}\n{}", response.path, truncate_chars(&response.diff, 4_000)))
    }
    "create_directory" => {
      let path_text = tool_arg_string(arguments, &["path", "directory_path", "dir"])
        .ok_or_else(|| "create_directory benoetigt path".to_string())?;
      let resolved_path = resolve_crew_tool_path(&path_text, context.cwd.as_deref())?;
      let canonical_target = ensure_run_file_access(database, None, &resolved_path, true)?;
      let target_text = canonical_target.display().to_string();
      enforce_tool_policy(policy, "create_directory", &target_text, true)?;
      let response = file_safety::create_directory(&canonical_target)?;

      let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
      let details = file_safety::create_directory_audit_details(&response.path, response.created);
      let _ = audit::append_audit_event(app_data_dir, "file_safety", "create_directory", Some(details));

      Ok(serde_json::to_string_pretty(&serde_json::json!({
        "path": response.path,
        "created": response.created,
      }))
      .unwrap_or_else(|_| "{}".to_string()))
    }
    "move_path" => {
      let source_path = tool_arg_string(arguments, &["source_path", "from", "path"])
        .ok_or_else(|| "move_path benoetigt source_path".to_string())?;
      let destination_path = tool_arg_string(arguments, &["destination_path", "to"])
        .ok_or_else(|| "move_path benoetigt destination_path".to_string())?;
      let overwrite = tool_arg_bool(arguments, &["overwrite"]).unwrap_or(false);
      let resolved_source = resolve_crew_tool_path(&source_path, context.cwd.as_deref())?;
      let resolved_destination = resolve_crew_tool_path(&destination_path, context.cwd.as_deref())?;
      let canonical_source = ensure_run_file_access(database, None, &resolved_source, true)?;
      let canonical_destination = ensure_run_file_access(database, None, &resolved_destination, true)?;
      enforce_tool_policy(
        policy,
        "move_path",
        &format!("{} -> {}", canonical_source.display(), canonical_destination.display()),
        true,
      )?;
      let response = file_safety::move_path(&canonical_source, &canonical_destination, overwrite)?;

      let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
      let details = file_safety::mutate_path_audit_details(
        "move",
        &response.source_path,
        &response.destination_path,
        &response.item_kind,
        response.created_parent,
        response.replaced_existing,
      );
      let _ = audit::append_audit_event(app_data_dir, "file_safety", "move_path", Some(details));

      Ok(serde_json::to_string_pretty(&serde_json::json!({
        "sourcePath": response.source_path,
        "destinationPath": response.destination_path,
        "itemKind": response.item_kind,
        "createdParent": response.created_parent,
        "replacedExisting": response.replaced_existing,
      }))
      .unwrap_or_else(|_| "{}".to_string()))
    }
    "copy_path" => {
      let source_path = tool_arg_string(arguments, &["source_path", "from", "path"])
        .ok_or_else(|| "copy_path benoetigt source_path".to_string())?;
      let destination_path = tool_arg_string(arguments, &["destination_path", "to"])
        .ok_or_else(|| "copy_path benoetigt destination_path".to_string())?;
      let overwrite = tool_arg_bool(arguments, &["overwrite"]).unwrap_or(false);
      let resolved_source = resolve_crew_tool_path(&source_path, context.cwd.as_deref())?;
      let resolved_destination = resolve_crew_tool_path(&destination_path, context.cwd.as_deref())?;
      let canonical_source = ensure_run_file_access(database, None, &resolved_source, false)?;
      let canonical_destination = ensure_run_file_access(database, None, &resolved_destination, true)?;
      enforce_tool_policy(
        policy,
        "copy_path",
        &format!("{} -> {}", canonical_source.display(), canonical_destination.display()),
        true,
      )?;
      let response = file_safety::copy_path(&canonical_source, &canonical_destination, overwrite)?;

      let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
      let details = file_safety::mutate_path_audit_details(
        "copy",
        &response.source_path,
        &response.destination_path,
        &response.item_kind,
        response.created_parent,
        response.replaced_existing,
      );
      let _ = audit::append_audit_event(app_data_dir, "file_safety", "copy_path", Some(details));

      Ok(serde_json::to_string_pretty(&serde_json::json!({
        "sourcePath": response.source_path,
        "destinationPath": response.destination_path,
        "itemKind": response.item_kind,
        "createdParent": response.created_parent,
        "replacedExisting": response.replaced_existing,
      }))
      .unwrap_or_else(|_| "{}".to_string()))
    }
    "glob" => {
      let pattern = tool_arg_string(arguments, &["pattern", "glob"])
        .ok_or_else(|| "glob benoetigt pattern".to_string())?;
      let base_input = tool_arg_string(arguments, &["path", "base_path", "basePath"]).unwrap_or_else(|| ".".to_string());
      let resolved_base = resolve_crew_tool_path(&base_input, context.cwd.as_deref())?;
      let canonical_base = ensure_run_file_access(database, None, &resolved_base, false)?;
      enforce_tool_policy(
        policy,
        "glob",
        &format!("{}:{}", canonical_base.display(), pattern),
        true,
      )?;

      let limit = tool_arg_u64(arguments, &["limit", "max_results", "maxResults"]).unwrap_or(50).clamp(1, 200) as usize;
      let scan_limit = limit.saturating_mul(20).clamp(200, 2_000);
      let mut entries = Vec::new();
      collect_crew_tool_paths(canonical_base.as_path(), scan_limit, &mut entries)?;

      let matches = entries
        .into_iter()
        .filter(|path| crew_glob_matches(&pattern, path.as_path(), canonical_base.as_path()))
        .take(limit)
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();

      Ok(serde_json::to_string_pretty(&matches).unwrap_or_else(|_| "[]".to_string()))
    }
    "grep" => {
      let pattern = tool_arg_string(arguments, &["pattern", "query", "text"])
        .ok_or_else(|| "grep benoetigt pattern".to_string())?;
      let base_input = tool_arg_string(arguments, &["path", "base_path", "basePath"]).unwrap_or_else(|| ".".to_string());
      let include_pattern = tool_arg_string(arguments, &["include", "include_pattern", "includePattern"])
        .unwrap_or_else(|| "*".to_string());
      let resolved_base = resolve_crew_tool_path(&base_input, context.cwd.as_deref())?;
      let canonical_base = ensure_run_file_access(database, None, &resolved_base, false)?;
      enforce_tool_policy(
        policy,
        "grep",
        &format!("{}:{}", canonical_base.display(), pattern),
        true,
      )?;

      let limit = tool_arg_u64(arguments, &["limit", "max_results", "maxResults"]).unwrap_or(50).clamp(1, 200) as usize;
      let scan_limit = limit.saturating_mul(20).clamp(200, 2_000);
      let mut entries = Vec::new();
      collect_crew_tool_paths(canonical_base.as_path(), scan_limit, &mut entries)?;

      let pattern_lower = pattern.to_lowercase();
      let mut matches = Vec::new();

      for path in entries {
        if !crew_glob_matches(&include_pattern, path.as_path(), canonical_base.as_path()) {
          continue;
        }

        let text = match artifact_pipeline::extract_text_for_llm_limited(path.as_path(), 20_000) {
          Ok((text, _)) => text,
          Err(_) => continue,
        };

        for (index, line) in text.lines().enumerate() {
          if line.to_lowercase().contains(&pattern_lower) {
            matches.push(serde_json::json!({
              "path": path.display().to_string(),
              "line": index + 1,
              "content": truncate_chars(line.trim(), 400),
            }));

            if matches.len() >= limit {
              break;
            }
          }
        }

        if matches.len() >= limit {
          break;
        }
      }

      Ok(serde_json::to_string_pretty(&matches).unwrap_or_else(|_| "[]".to_string()))
    }
    "web_fetch" => {
      let requested_url = tool_arg_string(arguments, &["url"])
        .ok_or_else(|| "web_fetch benoetigt url".to_string())?;
      let max_chars = tool_arg_u64(arguments, &["max_chars", "maxChars"]).unwrap_or(4_000).clamp(500, 30_000) as usize;
      enforce_tool_policy(policy, "web_fetch", &requested_url, policy.flags.allow_web_fetch)?;
      let result = execute_pipeline_web_fetch(&requested_url).await?;
      Ok(truncate_chars(&result, max_chars))
    }
    "web_search" => {
      let query = tool_arg_string(arguments, &["query", "q"])
        .ok_or_else(|| "web_search benoetigt query".to_string())?;
      enforce_tool_policy(policy, "web_search", &query, policy.flags.allow_web_search)?;
      execute_pipeline_web_search(&query).await
    }
    "todo" => {
      let action = tool_arg_string(arguments, &["action"])
        .ok_or_else(|| "todo benoetigt action".to_string())?
        .to_ascii_lowercase();
      enforce_tool_policy(policy, "todo", &action, true)?;

      match action.as_str() {
        "create" => {
          let title = tool_arg_string(arguments, &["title"])
            .ok_or_else(|| "todo create benoetigt title".to_string())?;
          let prompt = tool_arg_string(arguments, &["prompt", "description"]).unwrap_or_default();
          let task_id = uuid::Uuid::new_v4().to_string();
          let created_at = chrono::Utc::now().to_rfc3339();
          database
            .insert_task(&task_id, &title, &prompt, "todo", None, &created_at)
            .map_err(|err| err.to_string())?;
          Ok(serde_json::to_string_pretty(&serde_json::json!({
            "taskId": task_id,
            "title": title,
            "prompt": prompt,
            "status": "todo",
          }))
          .unwrap_or_else(|_| "{}".to_string()))
        }
        "list" => {
          let limit = tool_arg_u64(arguments, &["limit", "max_results", "maxResults"]).unwrap_or(20).clamp(1, 50) as usize;
          let tasks = database
            .list_tasks()
            .map_err(|err| err.to_string())?
            .into_iter()
            .take(limit)
            .map(|(id, title, prompt, status, thread_id, created_at, updated_at, error)| {
              serde_json::json!({
                "id": id,
                "title": title,
                "prompt": prompt,
                "status": status,
                "threadId": thread_id,
                "createdAt": created_at,
                "updatedAt": updated_at,
                "error": error,
              })
            })
            .collect::<Vec<_>>();
          Ok(serde_json::to_string_pretty(&tasks).unwrap_or_else(|_| "[]".to_string()))
        }
        "update" => {
          let task_id = tool_arg_string(arguments, &["task_id", "id"])
            .ok_or_else(|| "todo update benoetigt task_id".to_string())?;
          let status = tool_arg_string(arguments, &["status"])
            .ok_or_else(|| "todo update benoetigt status".to_string())?;
          database.update_task_status(&task_id, &status).map_err(|err| err.to_string())?;
          Ok(serde_json::to_string_pretty(&serde_json::json!({
            "taskId": task_id,
            "status": status,
          }))
          .unwrap_or_else(|_| "{}".to_string()))
        }
        _ => Err(format!("todo action {} wird nicht unterstuetzt", action)),
      }
    }
    "mcp" => {
      let server_name = tool_arg_string(arguments, &["server_name", "name"])
        .ok_or_else(|| "mcp benoetigt server_name".to_string())?;
      let tool_name = tool_arg_string(arguments, &["tool_name", "tool"])
        .ok_or_else(|| "mcp benoetigt tool_name".to_string())?;
      let tool_args = tool_arg_object(arguments, &["arguments", "tool_args", "toolArgs"])
        .unwrap_or_default()
        .into_iter()
        .collect::<HashMap<_, _>>();

      if !context.mcp_server_names.contains(&server_name) {
        return Err(format!("MCP-Server {} ist fuer diesen Agenten nicht freigegeben", server_name));
      }

      enforce_tool_policy(
        policy,
        "mcp",
        &format!("{}::{}", server_name, tool_name),
        policy.flags.allow_mcp_tool_calls,
      )?;

      if !runtime_has_server(&server_name) {
        return Err(format!("MCP-Server {} ist nicht gestartet", server_name));
      }

      let response = runtime_call_tool(&server_name, &tool_name, tool_args).map_err(map_mcp_error)?;
      Ok(response.result)
    }
    "ask_user" => Err("ask_user ist in synchronen Crew-Laeufen derzeit nicht interaktiv verfuegbar".to_string()),
    _ => Err(format!("Unbekanntes Crew-Tool: {}", tool_id)),
  }
}

fn build_crew_system_prompt(request: &CrewExecuteRequest, agent: &CrewExecuteAgentRequest) -> String {
  let effective_tools = build_effective_crew_tool_ids(request, agent);
  let active_members = request
    .agents
    .iter()
    .filter(|member| member.enabled)
    .map(|member| {
      let member_tools = build_effective_crew_tool_ids(request, member);
      format!(
        "- {} [{}] Rolle: {}; Ziel: {}; Delegation: {}; Tools: {}",
        member.name,
        member.id,
        member.role,
        member.goal,
        crew_agent_can_delegate(request, member),
        if member_tools.is_empty() { "keine".to_string() } else { member_tools.join(", ") },
      )
    })
    .collect::<Vec<_>>()
    .join("\n");
  let delegation_hint = if crew_agent_can_delegate(request, agent) {
    "Delegationshinweis: Du darfst delegate_task nutzen, um klar abgegrenzte Unterauftraege an andere aktive Crew-Mitglieder zu uebergeben. Verwende dafuer die exakte agent_id aus der Crew-Liste."
  } else {
    "Delegationshinweis: Delegation ist fuer dich deaktiviert. Bearbeite nur deinen eigenen Teilauftrag."
  };
  let manager_hint = request.manager_agent_id.as_ref().map_or(String::new(), |manager_id| {
    if manager_id == &agent.id {
      if request.process.eq_ignore_ascii_case("hierarchical") {
        "Du bist der koordinierende Manager-Agent dieser Crew und darfst Aufgaben priorisieren, Ergebnisse bewerten und bei Bedarf Eskalationen formulieren.".to_string()
      } else {
        "Du bist zusaetzlich der koordinierende Manager-Agent dieser Crew.".to_string()
      }
    } else {
      format!("Der koordinierende Manager-Agent hat die ID {} und kann Rueckfragen sowie Eskalationen ausloesen.", manager_id)
    }
  });
  let provider_hint = agent.provider_kind.as_deref().unwrap_or("ollama");
  let mcp_scope = if agent.mcp_server_names.is_empty() {
    "keine MCP-Server freigegeben".to_string()
  } else {
    format!("freigegebene MCP-Server: {}", agent.mcp_server_names.join(", "))
  };
  let personality_hint = agent
    .personality_id
    .as_ref()
    .map(|id| format!("Persoenlichkeitsprofil: {}", id))
    .unwrap_or_else(|| "Persoenlichkeitsprofil: keines".to_string());
  let skills_hint = if agent.skills_markdown.trim().is_empty() {
    "kein skills.md hinterlegt".to_string()
  } else {
    agent.skills_markdown.trim().to_string()
  };
  let execution_guidelines = if request.execution_guidelines.trim().is_empty() {
    "keine zusaetzlichen Crew-Anweisungen".to_string()
  } else {
    request.execution_guidelines.trim().to_string()
  };
  let workdir_hint = request
    .cwd
    .as_deref()
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .unwrap_or("(nicht gesetzt)");
  let output_mode_hint = match request.output_mode.trim() {
    "bullet-report" => "Liefere die Antwort als klar gegliederten Stichpunkt-Report mit Ergebnissen, Risiken und naechsten Schritten.",
    "json" => "Liefere die Antwort als valides JSON-Objekt ohne Markdown-Codeblock.",
    _ => "Liefere eine direkte, umsetzbare Antwort auf Deutsch.",
  };

  format!(
    "Du arbeitest als Agent in der Crew \"{}\".\nBeschreibung: {}\nProzess: {}\n{}\n\nAgent:\n- Name: {}\n- Rolle: {}\n- Ziel: {}\n- Hintergrund: {}\n- skills.md: {}\n- {}\n- Provider: {}\n- Tools: {}\n- MCP-Zugriffe: {}\n- Arbeitsverzeichnis: {}\n- Delegation erlaubt: {}\n- Verbose: {}\n- Max Iterationen: {}\n- Max RPM der Crew: {}\n- Max parallele Tasks der Crew: {}\n- Verbose Crew-Kontext: {}\n- Crew-Zusatzanweisungen: {}\n- Gewuenschtes Ausgabeformat: {}\n- Stoppt nach erstem Fehler: {}\n- Retry-Versuche pro Task: {}\n- Manager-Review aktiv: {}\n- Teilt alle bisherigen Task-Ergebnisse global: {}\n- Zeichenlimit fuer geteilte Ergebnisse: {}\n\nAktive Crew-Mitglieder:\n{}\n\n{}\n\n{}",
    request.name,
    request.description,
    request.process,
    manager_hint,
    agent.name,
    agent.role,
    agent.goal,
    agent.backstory,
    skills_hint,
    personality_hint,
    provider_hint,
    effective_tools.join(", "),
    mcp_scope,
    workdir_hint,
    crew_agent_can_delegate(request, agent),
    agent.verbose,
    agent.max_iterations,
    request.max_rpm,
    request.max_parallel_tasks.unwrap_or(1).max(1),
    request.verbose,
    execution_guidelines,
    request.output_mode,
    request.stop_on_failure,
    request.retry_count.max(0),
    request.manager_review_enabled,
    request.share_all_task_outputs,
    request.shared_output_char_limit.max(0),
    active_members,
    delegation_hint,
    output_mode_hint,
  )
}

fn truncate_shared_output(output: &str, limit: i32) -> String {
  if limit <= 0 {
    return output.to_string();
  }

  output.chars().take(limit as usize).collect()
}

fn build_crew_task_prompt(request: &CrewExecuteRequest, task: &CrewExecuteTaskRequest) -> String {
  let manager_mode_hint = if request.process.eq_ignore_ascii_case("hierarchical") {
    "Manager-Modus aktiv: Ergebnisse sollen fuer nachgelagerte Delegation oder Bewertung klar strukturiert sein."
  } else {
    ""
  };
  let output_mode_hint = match request.output_mode.trim() {
    "bullet-report" => "Ausgabeformat fuer diesen Task: Stichpunkt-Report.",
    "json" => "Ausgabeformat fuer diesen Task: valides JSON ohne Markdown-Codeblock.",
    _ => "Ausgabeformat fuer diesen Task: Standardantwort.",
  };
  let execution_guidelines = if request.execution_guidelines.trim().is_empty() {
    String::new()
  } else {
    format!("\nCrew-Zusatzanweisungen: {}", request.execution_guidelines.trim())
  };
  let workdir_hint = request
    .cwd
    .as_deref()
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .map(|value| format!("\nArbeitsverzeichnis: {}", value))
    .unwrap_or_default();

  format!(
    "Crew: {}\nTask-ID: {}\nBeschreibung: {}\nErwartetes Ergebnis: {}\nAsynchrone Ausfuehrung erlaubt: {}\nCrew stoppt bei Fehler: {}\nRetry-Versuche fuer diesen Task: {}\n{}\n{}{}{}\n\nLiefere das Task-Ergebnis direkt.",
    request.name,
    task.id,
    task.description,
    if task.expected_output.trim().is_empty() { "(nicht angegeben)" } else { &task.expected_output },
    task.async_execution,
    request.stop_on_failure,
    request.retry_count.max(0),
    manager_mode_hint,
    output_mode_hint,
    workdir_hint,
    execution_guidelines,
  )
}

fn mark_skipped_crew_tasks(
  request: &CrewExecuteRequest,
  tasks: Vec<CrewExecuteTaskRequest>,
  task_results: &mut Vec<CrewTaskExecutionRow>,
  logs: &mut Vec<CrewExecutionLogRow>,
  reason: &str,
) {
  for task in tasks {
    task_results.push(CrewTaskExecutionRow {
      task_id: task.id.clone(),
      agent_id: task.agent_id.clone(),
      status: "failed".to_string(),
      output: Some(reason.to_string()),
    });
    logs.push(CrewExecutionLogRow {
      id: uuid::Uuid::new_v4().to_string(),
      crew_id: request.id.clone(),
      agent_id: task.agent_id,
      task_id: task.id,
      action: "Task uebersprungen".to_string(),
      result: reason.to_string(),
      timestamp: chrono::Utc::now().timestamp_millis(),
    });
  }
}

fn build_crew_task_history(
  request: &CrewExecuteRequest,
  agent: &CrewExecuteAgentRequest,
  task: &CrewExecuteTaskRequest,
  task_outputs: &HashMap<String, String>,
) -> Vec<ChatMessage> {
  let mut history = vec![ChatMessage {
    role: "system".to_string(),
    content: build_crew_system_prompt(request, agent),
  }];

  for context_line in &task.context {
    if !context_line.trim().is_empty() {
      history.push(ChatMessage {
        role: "context".to_string(),
        content: context_line.clone(),
      });
    }
  }

  for dependency in &task.dependencies {
    if let Some(output) = task_outputs.get(dependency) {
      history.push(ChatMessage {
        role: "dependency".to_string(),
        content: format!(
          "Ergebnis aus {}:\n{}",
          dependency,
          truncate_shared_output(output, request.shared_output_char_limit)
        ),
      });
    }
  }

  if request.process.eq_ignore_ascii_case("hierarchical") {
    if let Some(manager_id) = &request.manager_agent_id {
      history.push(ChatMessage {
        role: "manager".to_string(),
        content: format!("Arbeite kompatibel mit dem Manager-Agenten {} und gib ein delegierbares Ergebnis zurueck.", manager_id),
      });
    }
  }

  if request.share_all_task_outputs {
    for (task_id, output) in task_outputs {
      if task_id != &task.id && !task.dependencies.contains(task_id) {
        history.push(ChatMessage {
          role: "memory".to_string(),
          content: format!(
            "Vorheriges Crew-Ergebnis aus {}:\n{}",
            task_id,
            truncate_shared_output(output, request.shared_output_char_limit)
          ),
        });
      }
    }
  }

  history
}

fn detect_crew_cycle(
  task_id: &str,
  dependency_graph: &HashMap<String, Vec<String>>,
  visiting: &mut HashSet<String>,
  visited: &mut HashSet<String>,
) -> bool {
  if visited.contains(task_id) {
    return false;
  }

  if !visiting.insert(task_id.to_string()) {
    return true;
  }

  if let Some(dependencies) = dependency_graph.get(task_id) {
    for dependency in dependencies {
      if detect_crew_cycle(dependency, dependency_graph, visiting, visited) {
        return true;
      }
    }
  }

  visiting.remove(task_id);
  visited.insert(task_id.to_string());
  false
}

fn validate_crew_request(
  request: &CrewExecuteRequest,
  enabled_agents: &HashMap<String, CrewExecuteAgentRequest>,
) -> Result<(), String> {
  let mut task_ids = HashSet::new();
  let mut dependency_graph: HashMap<String, Vec<String>> = HashMap::new();

  if request.process.eq_ignore_ascii_case("hierarchical") {
    if let Some(manager_id) = &request.manager_agent_id {
      if !enabled_agents.contains_key(manager_id) {
        return Err(format!("Manager-Agent {} ist nicht aktiv oder nicht vorhanden", manager_id));
      }
    } else {
      return Err("Hierarchische Crew benoetigt einen aktiven Manager-Agenten".to_string());
    }
  }

  for task in &request.tasks {
    if !enabled_agents.contains_key(&task.agent_id) {
      return Err(format!("Task {} verweist auf einen inaktiven oder fehlenden Agenten {}", task.id, task.agent_id));
    }

    if !task_ids.insert(task.id.clone()) {
      return Err(format!("Task-ID {} ist mehrfach vorhanden", task.id));
    }

    if task.dependencies.iter().any(|dependency| dependency == &task.id) {
      return Err(format!("Task {} darf nicht von sich selbst abhaengen", task.id));
    }

    dependency_graph.insert(task.id.clone(), task.dependencies.clone());
  }

  for task in &request.tasks {
    for dependency in &task.dependencies {
      if !task_ids.contains(dependency) {
        return Err(format!("Task {} verweist auf unbekannte Abhaengigkeit {}", task.id, dependency));
      }
    }
  }

  let mut visiting = HashSet::new();
  let mut visited = HashSet::new();
  for task in &request.tasks {
    if detect_crew_cycle(&task.id, &dependency_graph, &mut visiting, &mut visited) {
      return Err(format!("Zyklische Task-Abhaengigkeit erkannt, ausgehend von {}", task.id));
    }
  }

  Ok(())
}

async fn execute_manager_review(
  app: &tauri::AppHandle,
  database: &Arc<Database>,
  policy: &PolicyStatePayload,
  request: &CrewExecuteRequest,
  manager_agent: CrewExecuteAgentRequest,
  batch_summary: &str,
) -> Vec<CrewExecutionLogRow> {
  let history = vec![
    ChatMessage {
      role: "system".to_string(),
      content: build_crew_system_prompt(request, &manager_agent),
    },
    ChatMessage {
      role: "context".to_string(),
      content: "Du bist im hierarchischen Crew-Modus fuer Review, Priorisierung und Eskalation zustaendig.".to_string(),
    },
  ];

  let prompt = format!(
    "Bewerte die folgenden Crew-Ergebnisse knapp und praxisnah. Benenne Risiken, notwendige Folgeaktionen und Eskalationen.{}\n\n{}",
    if request.manager_review_guidelines.trim().is_empty() {
      String::new()
    } else {
      format!("\nZusaetzliche Review-Anweisungen: {}", request.manager_review_guidelines.trim())
    },
    batch_summary,
  );
  let review_task_id = request
    .manager_agent_id
    .clone()
    .unwrap_or_else(|| "manager-review".to_string());
  let task_config = match resolve_crew_task_config(request, &manager_agent) {
    Ok(config) => config,
    Err(error) => {
      return vec![CrewExecutionLogRow {
        id: uuid::Uuid::new_v4().to_string(),
        crew_id: request.id.clone(),
        agent_id: manager_agent.id,
        task_id: review_task_id,
        action: "Manager Review fehlgeschlagen".to_string(),
        result: error.chars().take(1200).collect(),
        timestamp: chrono::Utc::now().timestamp_millis(),
      }];
    }
  };

  let tool_context = match build_crew_tool_context(request, &manager_agent, &review_task_id, database, 0) {
    Ok(context) => context,
    Err(error) => {
      return vec![CrewExecutionLogRow {
        id: uuid::Uuid::new_v4().to_string(),
        crew_id: request.id.clone(),
        agent_id: manager_agent.id,
        task_id: review_task_id,
        action: "Manager Review fehlgeschlagen".to_string(),
        result: error.chars().take(1200).collect(),
        timestamp: chrono::Utc::now().timestamp_millis(),
      }];
    }
  };

  match execute_crew_chat(
    app,
    database,
    policy,
    task_config,
    prompt,
    history,
    Some(&tool_context),
    manager_agent.max_iterations.max(1) as usize,
  )
  .await
  {
    Ok(response) => {
      let mut logs = response.logs;
      logs.push(CrewExecutionLogRow {
        id: uuid::Uuid::new_v4().to_string(),
        crew_id: request.id.clone(),
        agent_id: manager_agent.id,
        task_id: review_task_id,
        action: "Manager Review".to_string(),
        result: response.assistant_message.chars().take(1200).collect(),
        timestamp: chrono::Utc::now().timestamp_millis(),
      });
      logs
    }
    Err(error) => {
      let mut logs = error.logs;
      logs.push(CrewExecutionLogRow {
        id: uuid::Uuid::new_v4().to_string(),
        crew_id: request.id.clone(),
        agent_id: manager_agent.id,
        task_id: review_task_id,
        action: "Manager Review fehlgeschlagen".to_string(),
        result: error.message.chars().take(1200).collect(),
        timestamp: chrono::Utc::now().timestamp_millis(),
      });
      logs
    }
  }
}

fn resolve_crew_task_config(
  request: &CrewExecuteRequest,
  agent: &CrewExecuteAgentRequest,
) -> Result<CrewTaskProviderConfig, String> {
  match agent.provider_kind.as_deref().unwrap_or("ollama") {
    "openai-compatible" => {
      let mut config = request
        .provider_configs
        .open_ai_compatible
        .clone()
        .ok_or_else(|| "Kein OpenAI-kompatibles Crew-Profil konfiguriert".to_string())?;
      if let Some(model_override) = agent.model_override.clone().filter(|value| !value.trim().is_empty()) {
        config.model = model_override;
      }
      Ok(CrewTaskProviderConfig::OpenAiCompatible(config))
    }
    "openrouter" => {
      let mut config = request
        .provider_configs
        .open_router
        .clone()
        .ok_or_else(|| "Kein OpenRouter-Crew-Profil konfiguriert".to_string())?;
      if let Some(model_override) = agent.model_override.clone().filter(|value| !value.trim().is_empty()) {
        config.model = model_override;
      }
      Ok(CrewTaskProviderConfig::OpenRouter(config))
    }
    _ => {
      let mut task_config = request.config.clone();
      if let Some(model_override) = agent.model_override.clone().filter(|value| !value.trim().is_empty()) {
        let mut config = task_config.unwrap_or_default();
        config.model = model_override;
        task_config = Some(config);
      }
      Ok(CrewTaskProviderConfig::Ollama(task_config))
    }
  }
}

fn sanitize_crew_request_snapshot(request: &CrewExecuteRequest) -> CrewExecuteRequest {
  let mut snapshot = request.clone();

  if let Some(config) = snapshot.provider_configs.open_ai_compatible.as_mut() {
    if !config.api_key.trim().is_empty() {
      config.api_key = "***redacted***".to_string();
    }
  }

  if let Some(config) = snapshot.provider_configs.open_router.as_mut() {
    if !config.api_key.trim().is_empty() {
      config.api_key = "***redacted***".to_string();
    }
  }

  snapshot
}

fn replay_provider_config_needs_restore(config: Option<&CrewExternalProviderConfigRequest>) -> bool {
  config
    .map(|entry| entry.api_key.trim().is_empty() || entry.api_key.trim() == "***redacted***")
    .unwrap_or(true)
}

fn extract_replay_provider_config(value: Option<&Value>) -> Option<CrewExternalProviderConfigRequest> {
  let profile = value?;
  if !profile.get("enabled").and_then(Value::as_bool).unwrap_or(false) {
    return None;
  }

  Some(CrewExternalProviderConfigRequest {
    base_url: profile.get("baseUrl").and_then(Value::as_str).unwrap_or_default().to_string(),
    model: profile.get("model").and_then(Value::as_str).unwrap_or_default().to_string(),
    api_key: profile.get("apiKey").and_then(Value::as_str).unwrap_or_default().to_string(),
    timeout_ms: profile.get("timeoutMs").and_then(Value::as_u64).unwrap_or(600000),
  })
}

fn restore_replay_provider_configs(
  database: &Arc<Database>,
  request: &mut CrewExecuteRequest,
) {
  let needs_openai = replay_provider_config_needs_restore(request.provider_configs.open_ai_compatible.as_ref());
  let needs_openrouter = replay_provider_config_needs_restore(request.provider_configs.open_router.as_ref());

  if !needs_openai && !needs_openrouter {
    return;
  }

  let latest_definition = database
    .list_crew_definition_versions(&request.id)
    .ok()
    .and_then(|versions| versions.into_iter().next());
  let Some(definition) = latest_definition else {
    return;
  };

  let Ok(definition_json) = serde_json::from_str::<Value>(&definition.definition_json) else {
    return;
  };
  let provider_profiles = definition_json.get("providerProfiles");

  if needs_openai {
    if let Some(config) = extract_replay_provider_config(provider_profiles.and_then(|profiles| profiles.get("openAICompatible"))) {
      request.provider_configs.open_ai_compatible = Some(config);
    }
  }

  if needs_openrouter {
    if let Some(config) = extract_replay_provider_config(provider_profiles.and_then(|profiles| profiles.get("openRouter"))) {
      request.provider_configs.open_router = Some(config);
    }
  }
}

fn ensure_crew_run_is_approved(
  database: &Arc<Database>,
  crew_id: &str,
) -> Result<(), String> {
  let pending_approvals = database
    .list_crew_approvals(Some("pending"), Some(crew_id))
    .map_err(|error| error.to_string())?;

  let pending_run_gates = pending_approvals
    .into_iter()
    .filter(|approval| approval.approval_type.eq_ignore_ascii_case("run_gate"))
    .collect::<Vec<_>>();

  if pending_run_gates.is_empty() {
    return Ok(());
  }

  let approval_ids = pending_run_gates
    .iter()
    .map(|approval| approval.id.clone())
    .collect::<Vec<_>>()
    .join(", ");

  Err(format!(
    "Crew-Start blockiert: offene run_gate-Freigaben fuer Crew {} ({}).",
    crew_id,
    approval_ids,
  ))
}

fn persist_crew_execution_response(
  database: &Arc<Database>,
  request: &CrewExecuteRequest,
  run_id: &str,
  started_at: &str,
  request_snapshot_json: &str,
  response: &CrewExecutionResponse,
) {
  let finished_at = chrono::Utc::now().to_rfc3339();

  let _ = database.insert_crew_run(
    run_id,
    &request.id,
    &request.name,
    &request.process,
    &response.status,
    request.manager_agent_id.as_deref(),
    response.error.as_deref(),
    request_snapshot_json,
    started_at,
    Some(&finished_at),
  );
  let _ = database.insert_crew_run_logs(run_id, &response.logs);

  let response_payload = serde_json::to_string(response).ok();
  let _ = database.insert_crew_run_event(
    &uuid::Uuid::new_v4().to_string(),
    run_id,
    &request.id,
    "run_completed",
    response_payload.as_deref(),
  );

  for log in &response.logs {
    let payload = serde_json::json!({
      "agentId": log.agent_id,
      "taskId": log.task_id,
      "action": log.action,
      "result": log.result,
      "timestamp": log.timestamp,
    });
    let payload_json = serde_json::to_string(&payload).ok();
    let _ = database.insert_crew_run_event(
      &uuid::Uuid::new_v4().to_string(),
      run_id,
      &request.id,
      "crew_log",
      payload_json.as_deref(),
    );
  }
}

async fn execute_crew_request(
  app: &tauri::AppHandle,
  database: &Arc<Database>,
  registry: &CrewExecutionRegistry,
  bridge: &CrewPythonBridge,
  request: CrewExecuteRequest,
) -> Result<CrewExecutionResponse, String> {
  if request.tasks.is_empty() {
    return Err("Crew enthaelt keine Tasks".to_string());
  }

  let enabled_agents: HashMap<String, CrewExecuteAgentRequest> = request
    .agents
    .iter()
    .filter(|agent| agent.enabled)
    .cloned()
    .map(|agent| (agent.id.clone(), agent))
    .collect();

  if enabled_agents.is_empty() {
    return Err("Crew enthaelt keine aktiven Agenten".to_string());
  }

  validate_crew_request(&request, &enabled_agents)?;
  ensure_crew_run_is_approved(database, &request.id)?;

  let runtime_payload = serde_json::to_value(&request).map_err(|error| error.to_string())?;
  let started_at = chrono::Utc::now().to_rfc3339();
  let run_id = uuid::Uuid::new_v4().to_string();
  let request_snapshot_json = serde_json::to_string(&sanitize_crew_request_snapshot(&request)).unwrap_or_else(|_| "{}".to_string());

  let _ = database.insert_crew_run_event(
    &uuid::Uuid::new_v4().to_string(),
    &run_id,
    &request.id,
    "run_started",
    Some(&request_snapshot_json),
  );

  if let Ok(mut canceled) = registry.canceled.lock() {
    canceled.remove(&request.id);
  }

  let runtime_result = crew_runtime_execute_request(app, bridge, &runtime_payload);

  if let Ok(mut canceled) = registry.canceled.lock() {
    canceled.remove(&request.id);
  }

  match runtime_result {
    Ok(runtime_response) => {
      let response = CrewExecutionResponse {
        crew_id: runtime_response.crew_id,
        status: runtime_response.status,
        task_results: runtime_response
          .task_results
          .into_iter()
          .map(|entry| CrewTaskExecutionRow {
            task_id: entry.task_id,
            agent_id: entry.agent_id,
            status: entry.status,
            output: entry.output,
          })
          .collect(),
        logs: runtime_response
          .logs
          .into_iter()
          .map(|entry| CrewExecutionLogRow {
            id: entry.id,
            crew_id: entry.crew_id,
            agent_id: entry.agent_id,
            task_id: entry.task_id,
            action: entry.action,
            result: entry.result,
            timestamp: entry.timestamp,
          })
          .collect(),
        error: runtime_response.error,
      };

      persist_crew_execution_response(database, &request, &run_id, &started_at, &request_snapshot_json, &response);
      Ok(response)
    }
    Err(error) => {
      if error.contains("Crew runtime ist nicht vorbereitet") || error.contains("Crew runtime Skript fehlt") {
        return Err(error);
      }

      let response = CrewExecutionResponse {
        crew_id: request.id.clone(),
        status: "failed".to_string(),
        task_results: Vec::new(),
        logs: vec![CrewExecutionLogRow {
          id: uuid::Uuid::new_v4().to_string(),
          crew_id: request.id.clone(),
          agent_id: request.manager_agent_id.clone().unwrap_or_else(|| "python-runtime".to_string()),
          task_id: request.tasks.first().map(|task| task.id.clone()).unwrap_or_else(|| "runtime".to_string()),
          action: "Runtime-Fehler".to_string(),
          result: error.clone(),
          timestamp: chrono::Utc::now().timestamp_millis(),
        }],
        error: Some(error.clone()),
      };

      persist_crew_execution_response(database, &request, &run_id, &started_at, &request_snapshot_json, &response);
      Ok(response)
    }
  }
}

async fn execute_crew_task(
  app: tauri::AppHandle,
  database: Arc<Database>,
  policy: PolicyStatePayload,
  request: CrewExecuteRequest,
  agent: CrewExecuteAgentRequest,
  task: CrewExecuteTaskRequest,
  task_outputs: HashMap<String, String>,
) -> (CrewTaskExecutionRow, Vec<CrewExecutionLogRow>, Option<String>) {
  let mut logs = vec![CrewExecutionLogRow {
    id: uuid::Uuid::new_v4().to_string(),
    crew_id: request.id.clone(),
    agent_id: agent.id.clone(),
    task_id: task.id.clone(),
    action: format!("Task gestartet: {}", task.description.chars().take(80).collect::<String>()),
    result: format!("Agent: {} [{}]", agent.name, agent.provider_kind.clone().unwrap_or_else(|| "ollama".to_string())),
    timestamp: chrono::Utc::now().timestamp_millis(),
  }];

  let history = build_crew_task_history(&request, &agent, &task, &task_outputs);
  let prompt = build_crew_task_prompt(&request, &task);
  let tool_context = match build_crew_tool_context(&request, &agent, &task.id, &database, 0) {
    Ok(context) => context,
    Err(error) => {
      logs.push(CrewExecutionLogRow {
        id: uuid::Uuid::new_v4().to_string(),
        crew_id: request.id.clone(),
        agent_id: agent.id.clone(),
        task_id: task.id.clone(),
        action: "Task fehlgeschlagen".to_string(),
        result: error.chars().take(200).collect(),
        timestamp: chrono::Utc::now().timestamp_millis(),
      });
      return (
        CrewTaskExecutionRow {
          task_id: task.id,
          agent_id: agent.id,
          status: "failed".to_string(),
          output: Some(error),
        },
        logs,
        None,
      );
    }
  };
  let task_config = match resolve_crew_task_config(&request, &agent) {
    Ok(config) => config,
    Err(error) => {
      logs.push(CrewExecutionLogRow {
        id: uuid::Uuid::new_v4().to_string(),
        crew_id: request.id.clone(),
        agent_id: agent.id.clone(),
        task_id: task.id.clone(),
        action: "Task fehlgeschlagen".to_string(),
        result: error.chars().take(200).collect(),
        timestamp: chrono::Utc::now().timestamp_millis(),
      });
      return (
        CrewTaskExecutionRow {
          task_id: task.id,
          agent_id: agent.id,
          status: "failed".to_string(),
          output: Some(error),
        },
        logs,
        None,
      );
    }
  };

  if crew_agent_can_delegate(&request, &agent) && !matches!(&task_config, CrewTaskProviderConfig::Ollama(_)) {
    logs.push(CrewExecutionLogRow {
      id: uuid::Uuid::new_v4().to_string(),
      crew_id: request.id.clone(),
      agent_id: agent.id.clone(),
      task_id: task.id.clone(),
      action: "Delegation eingeschraenkt".to_string(),
      result: "Dieser Provider unterstuetzt im Crew-Lauf derzeit keinen Tool-Loop. Echte Delegation ueber delegate_task ist aktuell nur mit Ollama verfuegbar.".to_string(),
      timestamp: chrono::Utc::now().timestamp_millis(),
    });
  }

  let max_attempts = request.retry_count.max(0) as usize + 1;
  let mut last_error_text = String::new();

  for attempt in 1..=max_attempts {
    match execute_crew_chat(
      &app,
      &database,
      &policy,
      task_config.clone(),
      prompt.clone(),
      history.clone(),
      Some(&tool_context),
      agent.max_iterations.max(1) as usize,
    )
    .await
    {
      Ok(response) => {
        let output = response.assistant_message;
        logs.extend(response.logs);
        logs.push(CrewExecutionLogRow {
          id: uuid::Uuid::new_v4().to_string(),
          crew_id: request.id.clone(),
          agent_id: agent.id.clone(),
          task_id: task.id.clone(),
          action: if attempt > 1 {
            format!("Task abgeschlossen nach Retry {}", attempt - 1)
          } else {
            "Task abgeschlossen".to_string()
          },
          result: output.chars().take(200).collect(),
          timestamp: chrono::Utc::now().timestamp_millis(),
        });
        return (
          CrewTaskExecutionRow {
            task_id: task.id,
            agent_id: agent.id,
            status: "completed".to_string(),
            output: Some(output.clone()),
          },
          logs,
          Some(output),
        );
      }
      Err(error) => {
        logs.extend(error.logs);
        last_error_text = error.message;
        if attempt < max_attempts {
          logs.push(CrewExecutionLogRow {
            id: uuid::Uuid::new_v4().to_string(),
            crew_id: request.id.clone(),
            agent_id: agent.id.clone(),
            task_id: task.id.clone(),
            action: format!("Retry {} geplant", attempt),
            result: last_error_text.chars().take(200).collect(),
            timestamp: chrono::Utc::now().timestamp_millis(),
          });
          continue;
        }
      }
    }
  }

  logs.push(CrewExecutionLogRow {
    id: uuid::Uuid::new_v4().to_string(),
    crew_id: request.id,
    agent_id: agent.id.clone(),
    task_id: task.id.clone(),
    action: "Task fehlgeschlagen".to_string(),
    result: last_error_text.chars().take(200).collect(),
    timestamp: chrono::Utc::now().timestamp_millis(),
  });
  (
    CrewTaskExecutionRow {
      task_id: task.id,
      agent_id: agent.id,
      status: "failed".to_string(),
      output: Some(last_error_text),
    },
    logs,
    None,
  )
}

fn is_crew_canceled(registry: &CrewExecutionRegistry, crew_id: &str) -> bool {
  registry
    .canceled
    .lock()
    .map(|canceled| canceled.contains(crew_id))
    .unwrap_or(false)
}

#[tauri::command]
async fn pipeline_execute(
  state: tauri::State<'_, Arc<Database>>,
  request: PipelineExecuteRequest,
) -> Result<PipelineExecutionResponse, String> {
  let pipeline = state
    .list_rpc_pipelines()
    .map_err(|err| err.to_string())?
    .into_iter()
    .find(|entry| entry.id == request.id)
    .ok_or_else(|| format!("Pipeline {} nicht gefunden", request.id))?;

  let steps: Vec<PipelineStepDefinition> = serde_json::from_str(&pipeline.steps_json)
    .map_err(|err| format!("Ungueltige Steps-JSON: {}", err))?;
  let gateways = state.list_tool_gateway_entries().unwrap_or_default();
  let mut step_results = Vec::with_capacity(steps.len());
  let mut previous_context = String::new();

  for (index, step) in steps.iter().enumerate() {
    let tool_name = step.tool.clone().unwrap_or_else(|| "ollama".to_string());
    let args_text = step.args.as_ref().map(value_to_step_text).unwrap_or_default();
    let prompt = step
      .prompt
      .clone()
      .filter(|value| !value.trim().is_empty())
      .unwrap_or_else(|| {
        if args_text.trim().is_empty() {
          format!("Pipeline-Schritt {} ohne Eingabetext", index + 1)
        } else {
          args_text.clone()
        }
      });

    let execution = match tool_name.as_str() {
      "web_fetch" => execute_pipeline_web_fetch(if args_text.trim().is_empty() { &prompt } else { &args_text }).await,
      "web_search" => execute_pipeline_web_search(if args_text.trim().is_empty() { &prompt } else { &args_text }).await,
      _ => {
        let context = if pipeline.zero_context {
          String::new()
        } else {
          previous_context.clone()
        };
        execute_pipeline_llm_step(
          request.config.clone(),
          &tool_name,
          &prompt,
          &context,
          find_gateway_context(&tool_name, &gateways),
        )
        .await
      }
    };

    match execution {
      Ok(result) => {
        if !pipeline.zero_context {
          previous_context.push_str(&format!("[{}] {}\n\n", tool_name, result));
        }
        step_results.push(PipelineExecutionStepResult {
          step: (index + 1) as i32,
          tool: tool_name,
          result,
          success: true,
        });
      }
      Err(error) => {
        step_results.push(PipelineExecutionStepResult {
          step: (index + 1) as i32,
          tool: tool_name,
          result: error.clone(),
          success: false,
        });

        return Ok(PipelineExecutionResponse {
          pipeline_id: pipeline.id,
          status: "failed".to_string(),
          step_results,
          error: Some(error),
        });
      }
    }
  }

  Ok(PipelineExecutionResponse {
    pipeline_id: pipeline.id,
    status: "completed".to_string(),
    step_results,
    error: None,
  })
}

async fn crew_execute_internal(
  app: &tauri::AppHandle,
  database: &Arc<Database>,
  registry: &CrewExecutionRegistry,
  request: CrewExecuteRequest,
) -> Result<CrewExecutionResponse, String> {
  let started_at = chrono::Utc::now().to_rfc3339();
  let run_id = uuid::Uuid::new_v4().to_string();
  let request_snapshot_json = serde_json::to_string(&sanitize_crew_request_snapshot(&request)).unwrap_or_else(|_| "{}".to_string());

  if request.tasks.is_empty() {
    return Err("Crew enthaelt keine Tasks".to_string());
  }

  let enabled_agents: HashMap<String, CrewExecuteAgentRequest> = request
    .agents
    .iter()
    .filter(|agent| agent.enabled)
    .cloned()
    .map(|agent| (agent.id.clone(), agent))
    .collect();

  if enabled_agents.is_empty() {
    return Err("Crew enthaelt keine aktiven Agenten".to_string());
  }

  validate_crew_request(&request, &enabled_agents)?;
  let policy = load_policy_state(database)?;
  let _ = database.insert_crew_run_event(
    &uuid::Uuid::new_v4().to_string(),
    &run_id,
    &request.id,
    "run_started",
    Some(&request_snapshot_json),
  );

  if let Ok(mut canceled) = registry.canceled.lock() {
    canceled.remove(&request.id);
  }

  let mut task_outputs: HashMap<String, String> = HashMap::new();
  let mut task_results = Vec::with_capacity(request.tasks.len());
  let mut logs = Vec::new();
  let mut overall_status = "completed".to_string();
  let mut overall_error: Option<String> = None;
  let mut remaining_tasks = request.tasks.clone();

  while !remaining_tasks.is_empty() {
    if is_crew_canceled(&registry, &request.id) {
      overall_status = "canceled".to_string();
      overall_error = Some("Crew-Ausfuehrung abgebrochen".to_string());
      break;
    }

    let mut ready_tasks = Vec::new();
    let mut waiting_tasks = Vec::new();

    for task in remaining_tasks.into_iter() {
      if !enabled_agents.contains_key(&task.agent_id) {
        let error = format!("Agent {} fuer Task {} ist deaktiviert oder nicht vorhanden", task.agent_id, task.id);
        task_results.push(CrewTaskExecutionRow {
          task_id: task.id.clone(),
          agent_id: task.agent_id.clone(),
          status: "failed".to_string(),
          output: Some(error.clone()),
        });
        logs.push(CrewExecutionLogRow {
          id: uuid::Uuid::new_v4().to_string(),
          crew_id: request.id.clone(),
          agent_id: task.agent_id.clone(),
          task_id: task.id.clone(),
          action: "Task blockiert".to_string(),
          result: error.clone(),
          timestamp: chrono::Utc::now().timestamp_millis(),
        });
        overall_status = "failed".to_string();
        if overall_error.is_none() {
          overall_error = Some(error);
        }
        continue;
      }

      if task.dependencies.iter().all(|dependency| task_outputs.contains_key(dependency)) {
        ready_tasks.push(task);
      } else {
        waiting_tasks.push(task);
      }
    }

    if ready_tasks.is_empty() {
      let unresolved = waiting_tasks
        .iter()
        .map(|task| task.id.clone())
        .collect::<Vec<_>>()
        .join(", ");
      let error = format!("Keine ausfuehrbaren Tasks mehr. Unerfuellte Abhaengigkeiten fuer: {}", unresolved);
      for task in waiting_tasks {
        task_results.push(CrewTaskExecutionRow {
          task_id: task.id.clone(),
          agent_id: task.agent_id.clone(),
          status: "failed".to_string(),
          output: Some(error.clone()),
        });
        logs.push(CrewExecutionLogRow {
          id: uuid::Uuid::new_v4().to_string(),
          crew_id: request.id.clone(),
          agent_id: task.agent_id,
          task_id: task.id,
          action: "Task blockiert".to_string(),
          result: error.clone(),
          timestamp: chrono::Utc::now().timestamp_millis(),
        });
      }
      overall_status = "failed".to_string();
      if overall_error.is_none() {
        overall_error = Some(error);
      }
      break;
    }

    if request.process.eq_ignore_ascii_case("parallel") {
      let mut pending_parallel = ready_tasks;
      let max_parallel = request.max_parallel_tasks.unwrap_or(1).max(1) as usize;

      while !pending_parallel.is_empty() {
        let batch_size = pending_parallel.len().min(max_parallel);
        let current_batch = pending_parallel.drain(0..batch_size).collect::<Vec<_>>();
        let mut batch_review_summary = Vec::new();
        let batch_results = join_all(current_batch.into_iter().map(|task| {
          let app_handle = app.clone();
          let database = Arc::clone(database);
          let policy = policy.clone();
          let request_clone = request.clone();
          let task_outputs_clone = task_outputs.clone();
          let agent = enabled_agents
            .get(&task.agent_id)
            .cloned()
            .expect("enabled agent missing for ready task");
          async move { execute_crew_task(app_handle, database, policy, request_clone, agent, task, task_outputs_clone).await }
        })).await;

        for (task_row, task_logs, output) in batch_results {
          batch_review_summary.push(format!(
            "Task {} [{}]: {}",
            task_row.task_id,
            task_row.status,
            task_row.output.clone().unwrap_or_default()
          ));
          if task_row.status == "completed" {
            if let Some(text) = output {
              task_outputs.insert(task_row.task_id.clone(), text);
            }
          } else {
            overall_status = "failed".to_string();
            if overall_error.is_none() {
              overall_error = task_row.output.clone();
            }
          }
          task_results.push(task_row);
          logs.extend(task_logs);
        }

        if request.stop_on_failure && overall_status == "failed" {
          let reason = overall_error
            .clone()
            .unwrap_or_else(|| "Crew wurde nach einem Task-Fehler gestoppt".to_string());
          let mut skipped_tasks = pending_parallel.drain(..).collect::<Vec<_>>();
          skipped_tasks.extend(waiting_tasks.drain(..));
          mark_skipped_crew_tasks(&request, skipped_tasks, &mut task_results, &mut logs, &reason);
          break;
        }

        if request.process.eq_ignore_ascii_case("hierarchical") && request.manager_review_enabled {
          if let Some(manager_id) = &request.manager_agent_id {
            if let Some(manager_agent) = enabled_agents.get(manager_id).cloned() {
              if !batch_review_summary.is_empty() {
                logs.extend(
                  execute_manager_review(app, database, &policy, &request, manager_agent, &batch_review_summary.join("\n\n")).await,
                );
              }
            }
          }
        }
      }
    } else {
      let mut batch_review_summary = Vec::new();
      let mut pending_ready = ready_tasks.into_iter();
      while let Some(task) = pending_ready.next() {
        let Some(agent) = enabled_agents.get(&task.agent_id).cloned() else {
          continue;
        };
        let (task_row, task_logs, output) = execute_crew_task(
          app.clone(),
          Arc::clone(database),
          policy.clone(),
          request.clone(),
          agent,
          task,
          task_outputs.clone(),
        ).await;
        batch_review_summary.push(format!(
          "Task {} [{}]: {}",
          task_row.task_id,
          task_row.status,
          task_row.output.clone().unwrap_or_default()
        ));
        if task_row.status == "completed" {
          if let Some(text) = output {
            task_outputs.insert(task_row.task_id.clone(), text);
          }
        } else {
          overall_status = "failed".to_string();
          if overall_error.is_none() {
            overall_error = task_row.output.clone();
          }
        }
        task_results.push(task_row);
        logs.extend(task_logs);

        if request.stop_on_failure && overall_status == "failed" {
          let reason = overall_error
            .clone()
            .unwrap_or_else(|| "Crew wurde nach einem Task-Fehler gestoppt".to_string());
          let mut skipped_tasks = pending_ready.collect::<Vec<_>>();
          skipped_tasks.extend(waiting_tasks.drain(..));
          mark_skipped_crew_tasks(&request, skipped_tasks, &mut task_results, &mut logs, &reason);
          break;
        }
      }

      if request.process.eq_ignore_ascii_case("hierarchical") && request.manager_review_enabled {
        if let Some(manager_id) = &request.manager_agent_id {
          if let Some(manager_agent) = enabled_agents.get(manager_id).cloned() {
            if !batch_review_summary.is_empty() {
              logs.extend(
                execute_manager_review(app, database, &policy, &request, manager_agent, &batch_review_summary.join("\n\n")).await,
              );
            }
          }
        }
      }
    }

    remaining_tasks = waiting_tasks;
  }

  if let Ok(mut canceled) = registry.canceled.lock() {
    canceled.remove(&request.id);
  }

  let finished_at = chrono::Utc::now().to_rfc3339();
  let crew_snapshot_json = request_snapshot_json;
  let response = CrewExecutionResponse {
    crew_id: request.id.clone(),
    status: overall_status,
    task_results,
    logs,
    error: overall_error,
  };

  let _ = database.insert_crew_run(
    &run_id,
    &request.id,
    &request.name,
    &request.process,
    &response.status,
    request.manager_agent_id.as_deref(),
    response.error.as_deref(),
    &crew_snapshot_json,
    &started_at,
    Some(&finished_at),
  );
  let _ = database.insert_crew_run_logs(&run_id, &response.logs);
  let response_payload = serde_json::to_string(&response).ok();
  let _ = database.insert_crew_run_event(
    &uuid::Uuid::new_v4().to_string(),
    &run_id,
    &request.id,
    "run_completed",
    response_payload.as_deref(),
  );
  for log in &response.logs {
    let payload = serde_json::json!({
      "agentId": log.agent_id,
      "taskId": log.task_id,
      "action": log.action,
      "result": log.result,
      "timestamp": log.timestamp,
    });
    let payload_json = serde_json::to_string(&payload).ok();
    let _ = database.insert_crew_run_event(
      &uuid::Uuid::new_v4().to_string(),
      &run_id,
      &request.id,
      "crew_log",
      payload_json.as_deref(),
    );
  }

  Ok(response)
}

#[tauri::command]
async fn crew_execute(
  app: tauri::AppHandle,
  registry: tauri::State<'_, CrewExecutionRegistry>,
  bridge: tauri::State<'_, CrewPythonBridge>,
  request: CrewExecuteRequest,
) -> Result<CrewExecutionResponse, String> {
  let database = app.state::<Arc<Database>>();
  execute_crew_request(&app, database.inner(), &registry, bridge.inner(), request).await
}

#[tauri::command]
fn crew_stop(
  registry: tauri::State<'_, CrewExecutionRegistry>,
  bridge: tauri::State<'_, CrewPythonBridge>,
  request: CrewStopRequest,
) -> Result<(), String> {
  let mut canceled = registry.canceled.lock().map_err(|_| "Crew-Registry gesperrt".to_string())?;
  let crew_id = request.crew_id;
  canceled.insert(crew_id.clone());
  let _ = bridge.stop_active_run(&crew_id)?;
  Ok(())
}

#[tauri::command]
fn crew_runs_list(
  state: tauri::State<'_, Arc<Database>>,
  crew_id: Option<String>,
  limit: Option<u32>,
) -> Result<Vec<CrewRunHistoryRow>, String> {
  state
    .list_crew_runs(crew_id.as_deref(), limit.unwrap_or(20).clamp(1, 100) as i64)
    .map_err(|err| err.to_string())
    .map(|rows| {
      rows
        .into_iter()
        .map(|(id, crew_id, crew_name, process, status, manager_agent_id, error, started_at, finished_at)| CrewRunHistoryRow {
          id,
          crew_id,
          crew_name,
          process,
          status,
          manager_agent_id,
          error,
          started_at,
          finished_at,
        })
        .collect()
    })
}

#[tauri::command]
fn crew_run_logs_list(
  state: tauri::State<'_, Arc<Database>>,
  run_id: String,
) -> Result<Vec<CrewExecutionLogRow>, String> {
  state.list_crew_run_logs(&run_id).map_err(|err| err.to_string())
}

#[tauri::command]
fn crew_run_snapshot_get(
  state: tauri::State<'_, Arc<Database>>,
  run_id: String,
) -> Result<Option<CrewExecuteRequest>, String> {
  let snapshot = state.get_crew_run_snapshot(&run_id).map_err(|err| err.to_string())?;
  match snapshot {
    Some(snapshot_json) => serde_json::from_str::<CrewExecuteRequest>(&snapshot_json)
      .map(Some)
      .map_err(|err| err.to_string()),
    None => Ok(None),
  }
}

#[tauri::command]
async fn crew_run_replay(
  state: tauri::State<'_, Arc<Database>>,
  app: tauri::AppHandle,
  registry: tauri::State<'_, CrewExecutionRegistry>,
  bridge: tauri::State<'_, CrewPythonBridge>,
  run_id: String,
) -> Result<CrewExecutionResponse, String> {
  let snapshot_json = state
    .get_crew_run_snapshot(&run_id)
    .map_err(|error| error.to_string())?
    .ok_or_else(|| format!("Kein Crew-Snapshot fuer Run {} vorhanden", run_id))?;

  let mut request = serde_json::from_str::<CrewExecuteRequest>(&snapshot_json).map_err(|error| error.to_string())?;
  restore_replay_provider_configs(state.inner(), &mut request);
  execute_crew_request(&app, state.inner(), &registry, bridge.inner(), request).await
}

#[tauri::command]
fn crew_definition_upsert(
  state: tauri::State<'_, Arc<Database>>,
  request: CrewDefinitionUpsertRequest,
) -> Result<db::CrewDefinitionRow, String> {
  state
    .upsert_crew_definition(
      &request.id,
      &request.name,
      request.description.as_deref().unwrap_or(""),
      &request.definition_json,
      request.flow_json.as_deref(),
      request.change_summary.as_deref(),
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
fn crew_definition_list(
  state: tauri::State<'_, Arc<Database>>,
) -> Result<Vec<db::CrewDefinitionRow>, String> {
  state.list_crew_definitions().map_err(|error| error.to_string())
}

#[tauri::command]
fn crew_definition_versions_list(
  state: tauri::State<'_, Arc<Database>>,
  crew_id: String,
) -> Result<Vec<db::CrewDefinitionVersionRow>, String> {
  state.list_crew_definition_versions(&crew_id).map_err(|error| error.to_string())
}

#[tauri::command]
fn crew_role_binding_upsert(
  state: tauri::State<'_, Arc<Database>>,
  request: CrewRoleBindingUpsertRequest,
) -> Result<db::CrewRoleBindingRow, String> {
  state
    .upsert_crew_role_binding(
      &request.id,
      &request.scope_type,
      request.scope_ref.as_deref(),
      &request.role,
      &request.subject,
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
fn crew_role_binding_list(
  state: tauri::State<'_, Arc<Database>>,
  scope_type: Option<String>,
  scope_ref: Option<String>,
) -> Result<Vec<db::CrewRoleBindingRow>, String> {
  state
    .list_crew_role_bindings(scope_type.as_deref(), scope_ref.as_deref())
    .map_err(|error| error.to_string())
}

#[tauri::command]
fn crew_approval_create(
  state: tauri::State<'_, Arc<Database>>,
  request: CrewApprovalCreateRequest,
) -> Result<db::CrewApprovalRow, String> {
  state
    .insert_crew_approval(
      &request.id,
      request.crew_id.as_deref(),
      request.run_id.as_deref(),
      &request.approval_type,
      request.scope_ref.as_deref(),
      request.status.as_deref().unwrap_or("pending"),
      request.requested_by.as_deref(),
      request.payload_json.as_deref(),
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
fn crew_approval_resolve(
  state: tauri::State<'_, Arc<Database>>,
  request: CrewApprovalResolveRequest,
) -> Result<db::CrewApprovalRow, String> {
  state
    .resolve_crew_approval(
      &request.id,
      &request.status,
      request.resolved_by.as_deref(),
      request.resolution_note.as_deref(),
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
fn crew_approval_list(
  state: tauri::State<'_, Arc<Database>>,
  status: Option<String>,
  crew_id: Option<String>,
) -> Result<Vec<db::CrewApprovalRow>, String> {
  state
    .list_crew_approvals(status.as_deref(), crew_id.as_deref())
    .map_err(|error| error.to_string())
}

#[tauri::command]
fn crew_run_events_list(
  state: tauri::State<'_, Arc<Database>>,
  run_id: Option<String>,
  crew_id: Option<String>,
  limit: Option<u32>,
) -> Result<Vec<db::CrewRunEventRow>, String> {
  state
    .list_crew_run_events(run_id.as_deref(), crew_id.as_deref(), limit.unwrap_or(100).clamp(1, 500) as i64)
    .map_err(|error| error.to_string())
}

// -- Ollama commands --------------------------------------------------------

#[tauri::command]
async fn ollama_health_check(config: Option<OllamaConfig>) -> Result<ollama::OllamaHealthResponse, String> {
  check_health(config).await.map_err(map_ollama_error)
}

#[tauri::command]
async fn generate_plan(request: PlanRequest) -> Result<ollama::PlanResponse, String> {
  generate_plan_internal(request.config, request.prompt)
    .await
    .map_err(map_ollama_error)
}

#[tauri::command]
async fn chat_turn(request: ChatTurnRequest) -> Result<ollama::ChatTurnResponse, String> {
  chat_turn_internal(
    request.config,
    request.prompt,
    request.history,
    request.tools.unwrap_or_default(),
  )
    .await
    .map_err(map_ollama_error)
}

#[tauri::command]
async fn chat_turn_stream(app: tauri::AppHandle, request: ChatTurnRequest) -> Result<ollama::ChatTurnResponse, String> {
  let stream_id = request
    .stream_id
    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
  let app_for_emit = app.clone();

  chat_turn_stream_internal(
    stream_id,
    request.config,
    request.prompt,
    request.history,
    request.tools.unwrap_or_default(),
    move |payload: ChatStreamChunkPayload| {
      app_for_emit
        .emit("ollama-chat-chunk", payload)
        .map_err(|error| OllamaError::RequestFailed(error.to_string()))
    },
  )
    .await
    .map_err(map_ollama_error)
}

// -- Claude Code Bridge commands --------------------------------------------

#[tauri::command]
fn claude_code_start(
  bridge: tauri::State<'_, ClaudeCodeBridge>,
  config: claude_code_bridge::ClaudeCodeConfig,
) -> Result<claude_code_bridge::ClaudeCodeStatus, String> {
  bridge.start(&config)
}

#[tauri::command]
fn claude_code_stop(bridge: tauri::State<'_, ClaudeCodeBridge>) -> Result<(), String> {
  bridge.stop()
}

#[tauri::command]
fn claude_code_status(bridge: tauri::State<'_, ClaudeCodeBridge>) -> claude_code_bridge::ClaudeCodeStatus {
  bridge.status()
}

#[tauri::command]
async fn claude_code_send(
  config: claude_code_bridge::ClaudeCodeConfig,
  prompt: String,
) -> Result<claude_code_bridge::ClaudeCodeResponse, String> {
  tauri::async_runtime::spawn_blocking(move || {
    ClaudeCodeBridge::send_prompt(&config, &prompt, "json")
  })
  .await
  .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn claude_code_send_stream(
  app: tauri::AppHandle,
  config: claude_code_bridge::ClaudeCodeConfig,
  prompt: String,
  session_id: String,
) -> Result<claude_code_bridge::ClaudeCodeResponse, String> {
  let app_for_emit = app.clone();
  let sid = session_id.clone();

  tauri::async_runtime::spawn_blocking(move || {
    ClaudeCodeBridge::send_prompt_streaming(
      &config,
      &prompt,
      &sid,
      move |chunk| {
        let _ = app_for_emit.emit("claude-code-chunk", &chunk);
      },
    )
  })
  .await
  .map_err(|e| e.to_string())?
}

#[tauri::command]
fn claude_code_list_commands() -> Vec<claude_code_bridge::ClaudeCodeCommandInfo> {
  claude_code_bridge::get_claude_code_commands()
}

#[tauri::command]
fn claude_code_list_tools() -> Vec<claude_code_bridge::ClaudeCodeToolInfo> {
  claude_code_bridge::get_claude_code_tools()
}

// -- MCP commands -----------------------------------------------------------

fn local_docs_mcp_probe(name: String) -> mcp::McpProbeResponse {
  mcp::McpProbeResponse {
    server_name: name,
    protocol_version: Some("2024-11-05".to_string()),
    server_info: Some("Open_Cowork Local Docs MCP 0.1.0".to_string()),
    tools: vec![
      mcp::McpTool {
        name: "extract_full_text".to_string(),
        description: "Extract full text from one file inside allowed folders".to_string(),
      },
      mcp::McpTool {
        name: "get_chunk".to_string(),
        description: "Read a text chunk by character offset and length".to_string(),
      },
      mcp::McpTool {
        name: "search_in_document".to_string(),
        description: "Search case-insensitive matches in extracted text".to_string(),
      },
      mcp::McpTool {
        name: "list_allowed_folders".to_string(),
        description: "List currently allowed root folders".to_string(),
      },
    ],
  }
}

fn local_screenshot_mcp_probe(name: String) -> mcp::McpProbeResponse {
  mcp::McpProbeResponse {
    server_name: name,
    protocol_version: Some("2024-11-05".to_string()),
    server_info: Some("Open_Cowork Screenshot MCP 0.1.0".to_string()),
    tools: vec![
      mcp::McpTool {
        name: "list_screens".to_string(),
        description: "List connected screens/monitors with bounds and primary flag".to_string(),
      },
      mcp::McpTool {
        name: "capture_screenshot".to_string(),
        description: "Capture screenshots for all connected screens (always all screens). Optional arg: outputDir".to_string(),
      },
      mcp::McpTool {
        name: "screenshot_for_display".to_string(),
        description: "Capture a screenshot for direct UI display. Returns image data + display metadata and an in-image coordinate grid (50px minor, 100px major) for reliable local display coordinates. Supports short-term reuse cache. Args: displayIndex/display_index, region, reason, forceRefresh/force_refresh.".to_string(),
      },
    ],
  }
}

fn escape_powershell_single_quoted(value: &str) -> String {
  value.replace('\'', "''")
}

fn run_powershell_script(script: &str) -> Result<String, String> {
  let allow_bypass = std::env::var("OPEN_COWORK_ALLOW_POWERSHELL_BYPASS")
    .map(|value| value == "1")
    .unwrap_or(false);
  let policies: Vec<&str> = if allow_bypass {
    vec!["RemoteSigned", "Bypass"]
  } else {
    vec!["RemoteSigned"]
  };

  let mut last_error = String::new();
  for policy in policies {
    let output = Command::new("powershell")
      .args([
        "-NoProfile",
        "-NonInteractive",
        "-STA",
        "-ExecutionPolicy",
        policy,
        "-Command",
        script,
      ])
      .output()
      .map_err(|err| format!("failed to launch powershell: {}", err))?;

    if output.status.success() {
      return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let details = if stderr.is_empty() { stdout } else { stderr };
    last_error = format!("policy={} details={}", policy, details);
  }

  Err(format!("powershell command failed: {}", last_error))
}

fn run_powershell_json_script<T: DeserializeOwned>(script: &str) -> Result<T, String> {
  let output = run_powershell_script(script)?;
  serde_json::from_str::<T>(&output).map_err(|err| format!("invalid powershell json: {}", err))
}

fn ensure_windows_desktop_support() -> Result<(), String> {
  if cfg!(target_os = "windows") {
    Ok(())
  } else {
    Err("desktop automation is currently supported only on Windows".to_string())
  }
}

fn desktop_powershell_prelude() -> &'static str {
  r#"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName Microsoft.VisualBasic
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class OpenCoworkDesktop {
  public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
  [StructLayout(LayoutKind.Sequential)]
  public struct RECT {
    public int Left;
    public int Top;
    public int Right;
    public int Bottom;
  }
  [DllImport("user32.dll")]
  public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr extraData);
  [DllImport("user32.dll")]
  public static extern bool IsWindowVisible(IntPtr hWnd);
  [DllImport("user32.dll")]
  public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)]
  public static extern int GetWindowText(IntPtr hWnd, StringBuilder text, int count);
  [DllImport("user32.dll")]
  public static extern int GetWindowTextLength(IntPtr hWnd);
  [DllImport("user32.dll")]
  public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
  [DllImport("user32.dll")]
  public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")]
  public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")]
  public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
  [DllImport("user32.dll")]
  public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")]
  public static extern bool SetProcessDpiAwarenessContext(IntPtr dpiContext);
  [DllImport("user32.dll")]
  public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")]
  public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extraInfo);

  public static string ReadWindowText(IntPtr hWnd) {
    int size = GetWindowTextLength(hWnd);
    var buffer = new StringBuilder(size + 1);
    GetWindowText(hWnd, buffer, buffer.Capacity);
    return buffer.ToString();
  }
}
"@

try {
  [void][OpenCoworkDesktop]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))
} catch {
  try {
    [void][OpenCoworkDesktop]::SetProcessDPIAware()
  } catch {
    # Best effort only. Desktop automation can continue without this on older systems.
  }
}

function ConvertTo-OpenCoworkWindow {
  param([IntPtr]$Handle)

  if (-not [OpenCoworkDesktop]::IsWindowVisible($Handle)) { return $null }

  $title = [OpenCoworkDesktop]::ReadWindowText($Handle)
  if ([string]::IsNullOrWhiteSpace($title)) { return $null }

  $rect = New-Object OpenCoworkDesktop+RECT
  [void][OpenCoworkDesktop]::GetWindowRect($Handle, [ref]$rect)
  $width = $rect.Right - $rect.Left
  $height = $rect.Bottom - $rect.Top
  if ($width -le 0 -or $height -le 0) { return $null }

  $processId = 0
  [void][OpenCoworkDesktop]::GetWindowThreadProcessId($Handle, [ref]$processId)
  if ($processId -le 0) { return $null }

  try {
    $process = Get-Process -Id $processId -ErrorAction Stop
  } catch {
    return $null
  }

  return [PSCustomObject]@{
    title = $title
    processId = [int]$processId
    processName = $process.ProcessName
    handle = ('0x{0:X}' -f $Handle.ToInt64())
    handleValue = $Handle.ToInt64()
    x = $rect.Left
    y = $rect.Top
    width = $width
    height = $height
    isForeground = ($Handle -eq [OpenCoworkDesktop]::GetForegroundWindow())
  }
}

function Get-OpenCoworkWindows {
  $items = New-Object System.Collections.Generic.List[object]
  $callback = [OpenCoworkDesktop+EnumWindowsProc]{
    param([IntPtr]$hWnd, [IntPtr]$lParam)
    $window = ConvertTo-OpenCoworkWindow -Handle $hWnd
    if ($null -ne $window) {
      [void]$items.Add($window)
    }
    return $true
  }
  [void][OpenCoworkDesktop]::EnumWindows($callback, [IntPtr]::Zero)
  return $items
}

function Test-OpenCoworkWindowMatch {
  param(
    $Window,
    [string]$Title,
    [string]$ProcessName,
    [Nullable[int]]$ProcessId,
    [bool]$ExactMatch
  )

  if ($ProcessId.HasValue -and $Window.processId -ne $ProcessId.Value) { return $false }

  if (-not [string]::IsNullOrWhiteSpace($ProcessName)) {
    $candidate = $Window.processName.ToLowerInvariant()
    $expected = $ProcessName.ToLowerInvariant()
    if ($ExactMatch) {
      if ($candidate -ne $expected) { return $false }
    } elseif (-not $candidate.Contains($expected)) {
      return $false
    }
  }

  if (-not [string]::IsNullOrWhiteSpace($Title)) {
    $candidate = $Window.title.ToLowerInvariant()
    $expected = $Title.ToLowerInvariant()
    if ($ExactMatch) {
      if ($candidate -ne $expected) { return $false }
    } elseif (-not $candidate.Contains($expected)) {
      return $false
    }
  }

  return $true
}
"#
}

fn desktop_coordinate_overlay_powershell() -> &'static str {
  r#"
$minorPen = [System.Drawing.Pen]::new([System.Drawing.Color]::FromArgb(65, 0, 122, 204), [single]1)
$majorPen = [System.Drawing.Pen]::new([System.Drawing.Color]::FromArgb(150, 255, 92, 0), [single]1)
$labelBrush = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(235, 255, 255, 255))
$labelBgBrush = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(185, 0, 0, 0))
$font = [System.Drawing.Font]::new('Consolas', [single]10, [System.Drawing.FontStyle]::Bold)

for ($gx = 0; $gx -le $captureWidth; $gx += 50) {
  $pen = if (($gx % 100) -eq 0) { $majorPen } else { $minorPen }
  $graphics.DrawLine($pen, $gx, 0, $gx, $captureHeight)
  if (($gx % 100) -eq 0) {
    $label = 'x=' + $gx
    $graphics.FillRectangle($labelBgBrush, $gx + 2, 2, 56, 16)
    $graphics.DrawString($label, $font, $labelBrush, $gx + 4, 2)
  }
}

for ($gy = 0; $gy -le $captureHeight; $gy += 50) {
  $pen = if (($gy % 100) -eq 0) { $majorPen } else { $minorPen }
  $graphics.DrawLine($pen, 0, $gy, $captureWidth, $gy)
  if (($gy % 100) -eq 0) {
    $label = 'y=' + $gy
    $graphics.FillRectangle($labelBgBrush, 2, $gy + 2, 56, 16)
    $graphics.DrawString($label, $font, $labelBrush, 4, $gy + 2)
  }
}

$font.Dispose()
$labelBgBrush.Dispose()
$labelBrush.Dispose()
$majorPen.Dispose()
$minorPen.Dispose()
"#
}

fn desktop_capture_primary_display_with_overlay(coordinate_overlay: bool) -> Result<DesktopScreenshotResponse, String> {
  ensure_windows_desktop_support()?;
  let overlay_script = if coordinate_overlay {
    desktop_coordinate_overlay_powershell()
  } else {
    ""
  };
  let script = format!(
    r#"
{}
$screen = [System.Windows.Forms.Screen]::PrimaryScreen
$bounds = $screen.Bounds
$captureWidth = $bounds.Width
$captureHeight = $bounds.Height
$bitmap = New-Object System.Drawing.Bitmap $captureWidth, $captureHeight
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($bounds.X, $bounds.Y, 0, 0, $bounds.Size)
{overlay_script}
$stream = New-Object System.IO.MemoryStream
$bitmap.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
$bytes = $stream.ToArray()
$scaleFactor = 1
try {{
  $scaleFactor = [double]($graphics.DpiX / 96.0)
  if ($scaleFactor -le 0) {{ $scaleFactor = 1 }}
}} catch {{
  $scaleFactor = 1
}}
$graphics.Dispose()
$bitmap.Dispose()
$stream.Dispose()
[PSCustomObject]@{{
  dataUrl = 'data:image/png;base64,' + [System.Convert]::ToBase64String($bytes)
  width = $bounds.Width
  height = $bounds.Height
  x = $bounds.X
  y = $bounds.Y
  primary = $true
  deviceName = $screen.DeviceName
  scaleFactor = [double]$scaleFactor
  imageWidth = $captureWidth
  imageHeight = $captureHeight
  coordinateOverlay = {coordinate_overlay}
}} | ConvertTo-Json -Compress
"#,
    desktop_powershell_prelude(),
    overlay_script = overlay_script,
    coordinate_overlay = if coordinate_overlay { "$true" } else { "$false" },
  );

  run_powershell_json_script::<DesktopScreenshotResponse>(&script)
}

fn desktop_capture_primary_display() -> Result<DesktopScreenshotResponse, String> {
  desktop_capture_primary_display_with_overlay(false)
}

fn desktop_list_windows_internal() -> Result<Vec<DesktopWindowInfo>, String> {
  ensure_windows_desktop_support()?;
  let script = format!(
    r#"
{}
Get-OpenCoworkWindows | Select-Object title, processId, processName, handle, x, y, width, height, isForeground | ConvertTo-Json -Compress
"#,
    desktop_powershell_prelude()
  );

  run_powershell_json_script::<Vec<DesktopWindowInfo>>(&script)
}

fn desktop_match_window(request: &DesktopWindowRequest) -> Result<DesktopWindowInfo, String> {
  ensure_windows_desktop_support()?;
  let title = escape_powershell_single_quoted(request.title.as_deref().unwrap_or(""));
  let process_name = escape_powershell_single_quoted(request.process_name.as_deref().unwrap_or(""));
  let process_id = request
    .process_id
    .map(|value| value.to_string())
    .unwrap_or_else(|| "$null".to_string());
  let exact_match = if request.exact_match.unwrap_or(false) { "$true" } else { "$false" };
  let script = format!(
    r#"
{}
$title = '{title}'
$processName = '{process_name}'
$processId = {process_id}
$exactMatch = {exact_match}
$match = Get-OpenCoworkWindows |
  Where-Object {{ Test-OpenCoworkWindowMatch $_ $title $processName $processId $exactMatch }} |
  Select-Object -First 1
if ($null -eq $match) {{
  throw 'desktop window not found'
}}
$match | Select-Object title, processId, processName, handle, x, y, width, height, isForeground | ConvertTo-Json -Compress
"#,
    desktop_powershell_prelude(),
    title = title,
    process_name = process_name,
    process_id = process_id,
    exact_match = exact_match,
  );

  run_powershell_json_script::<DesktopWindowInfo>(&script)
}

fn screenshot_region_key(region: Option<&ScreenshotDisplayRegion>) -> String {
  if let Some(value) = region {
    return format!("{},{},{},{}", value.x, value.y, value.width, value.height);
  }
  "full".to_string()
}

fn parse_i64_tool_arg(tool_args: &HashMap<String, Value>, camel: &str, snake: &str) -> Option<i64> {
  tool_args
    .get(camel)
    .and_then(|value| value.as_i64())
    .or_else(|| tool_args.get(snake).and_then(|value| value.as_i64()))
}

fn parse_bool_tool_arg(tool_args: &HashMap<String, Value>, camel: &str, snake: &str) -> Option<bool> {
  tool_args
    .get(camel)
    .and_then(|value| value.as_bool())
    .or_else(|| tool_args.get(snake).and_then(|value| value.as_bool()))
}

fn parse_string_tool_arg<'a>(tool_args: &'a HashMap<String, Value>, camel: &str, snake: &str) -> Option<&'a str> {
  tool_args
    .get(camel)
    .and_then(|value| value.as_str())
    .or_else(|| tool_args.get(snake).and_then(|value| value.as_str()))
}

fn parse_screenshot_region(tool_args: &HashMap<String, Value>) -> Result<Option<ScreenshotDisplayRegion>, String> {
  let Some(raw_region) = tool_args.get("region") else {
    return Ok(None);
  };

  serde_json::from_value::<ScreenshotDisplayRegion>(raw_region.clone())
    .map(Some)
    .map_err(|err| format!("invalid region payload: {}", err))
}

fn capture_screenshot_for_display_payload(
  app: &tauri::AppHandle,
  display_index: i64,
  region: Option<&ScreenshotDisplayRegion>,
  reason: Option<&str>,
  force_refresh: bool,
) -> Result<Value, String> {
  let region_key = screenshot_region_key(region);
  let request_key = format!("{}:{}", display_index, region_key);

  let (request_count, reusable_entry) = {
    let mut guard = screenshot_cache()
      .lock()
      .map_err(|_| "screenshot cache lock poisoned".to_string())?;
    let counter = guard.request_counts.entry(request_key).or_insert(0);
    *counter += 1;
    let request_count = *counter;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let last_entry = guard.last_entry.clone();

    let reusable = if force_refresh {
      None
    } else {
      last_entry.and_then(|entry| {
        if entry.display_index != display_index {
          return None;
        }
        if entry.region_key != region_key {
          return None;
        }
        if now_ms - entry.captured_at_ms > SCREENSHOT_REUSE_WINDOW_MS {
          return None;
        }
        Some(entry)
      })
    };

    (request_count, reusable)
  };

  if let Some(entry) = reusable_entry {
    let mut payload = serde_json::json!({
      "success": true,
      "reused": true,
      "path": entry.path,
      "displayIndex": entry.display_index,
      "displayInfo": entry.display_info,
      "duplicateCallCount": request_count,
      "timestamp": chrono::Utc::now().to_rfc3339(),
      "mimeType": entry.mime_type,
      "imageDataUrl": format!("{}{}", SCREENSHOT_DATA_URL_PREFIX, entry.base64_image),
    });

    if request_count > 1 {
      payload["nextStepHint"] = Value::String(
        "Screenshot wurde bereits vor kurzem aufgenommen. Nutze dieses Bild weiter, es sei denn ein Refresh ist explizit erforderlich."
          .to_string(),
      );
    }

    if let Some(reason_text) = reason {
      payload["reason"] = Value::String(reason_text.to_string());
    }
    if let Some(region_value) = region {
      payload["region"] = serde_json::to_value(region_value).unwrap_or(Value::Null);
    }

    return Ok(payload);
  }

  #[derive(Debug, Deserialize)]
  #[serde(rename_all = "camelCase")]
  struct RawScreenshotForDisplay {
    data_url: String,
    path: String,
    display_index: i64,
    primary: bool,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    device_name: String,
    #[serde(default)]
    scale_factor: Option<f64>,
  }

  let mut output_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
  output_dir.push("screenshots");
  fs::create_dir_all(&output_dir).map_err(|err| err.to_string())?;

  let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S-%3f").to_string();
  let screenshot_path = output_dir.join(format!("screenshot-display-{}-{}.png", display_index, timestamp));
  let escaped_path = escape_powershell_single_quoted(&screenshot_path.display().to_string());

  let region_script = if let Some(value) = region {
    format!(
      "\n$captureX = $bounds.X + {x}\n$captureY = $bounds.Y + {y}\n$captureWidth = {width}\n$captureHeight = {height}\nif ($captureWidth -le 0 -or $captureHeight -le 0) {{ throw 'region width/height must be positive' }}\nif ($captureX -lt $bounds.X -or $captureY -lt $bounds.Y -or ($captureX + $captureWidth) -gt ($bounds.X + $bounds.Width) -or ($captureY + $captureHeight) -gt ($bounds.Y + $bounds.Height)) {{ throw 'region is outside selected display bounds' }}\n",
      x = value.x,
      y = value.y,
      width = value.width,
      height = value.height,
    )
  } else {
    "\n$captureX = $bounds.X\n$captureY = $bounds.Y\n$captureWidth = $bounds.Width\n$captureHeight = $bounds.Height\n".to_string()
  };

  let script = format!(
    r#"
{prelude}
$displayIndex = {display_index}
$screens = [System.Windows.Forms.Screen]::AllScreens
if ($displayIndex -lt 0 -or $displayIndex -ge $screens.Length) {{
  throw ('display_index out of range: ' + $displayIndex)
}}
$screen = $screens[$displayIndex]
$bounds = $screen.Bounds
{region_script}

$bitmap = New-Object System.Drawing.Bitmap $captureWidth, $captureHeight
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($captureX, $captureY, 0, 0, [System.Drawing.Size]::new($captureWidth, $captureHeight))
{overlay_script}
$savePath = '{escaped_path}'
$bitmap.Save($savePath, [System.Drawing.Imaging.ImageFormat]::Png)

$stream = New-Object System.IO.MemoryStream
$bitmap.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
$bytes = $stream.ToArray()
$scaleFactor = 1
try {{
  $scaleFactor = [double]($graphics.DpiX / 96.0)
  if ($scaleFactor -le 0) {{ $scaleFactor = 1 }}
}} catch {{
  $scaleFactor = 1
}}

$graphics.Dispose()
$bitmap.Dispose()
$stream.Dispose()

[PSCustomObject]@{{
  dataUrl = 'data:image/png;base64,' + [System.Convert]::ToBase64String($bytes)
  path = $savePath
  displayIndex = [int]$displayIndex
  primary = $screen.Primary
  x = [int]$captureX
  y = [int]$captureY
  width = [int]$captureWidth
  height = [int]$captureHeight
  deviceName = $screen.DeviceName
  scaleFactor = [double]$scaleFactor
}} | ConvertTo-Json -Compress
"#,
    prelude = desktop_powershell_prelude(),
    display_index = display_index,
    region_script = region_script,
    escaped_path = escaped_path,
    overlay_script = desktop_coordinate_overlay_powershell(),
  );

  let captured = run_powershell_json_script::<RawScreenshotForDisplay>(&script)?;
  let base64_image = captured
    .data_url
    .strip_prefix(SCREENSHOT_DATA_URL_PREFIX)
    .ok_or_else(|| "unexpected screenshot payload format".to_string())?
    .to_string();
  let captured_at_ms = chrono::Utc::now().timestamp_millis();
  let display_info = serde_json::json!({
    "primary": captured.primary,
    "x": captured.x,
    "y": captured.y,
    "width": captured.width,
    "height": captured.height,
    "deviceName": captured.device_name,
    "scaleFactor": captured.scale_factor.unwrap_or(1.0),
    "imageWidth": captured.width,
    "imageHeight": captured.height,
    "coordinateOverlay": true,
    "coordinateGrid": {
      "minorStepPx": 50,
      "majorStepPx": 100,
      "origin": "top-left",
      "coordinateSpace": "display"
    },
  });

  {
    let mut guard = screenshot_cache()
      .lock()
      .map_err(|_| "screenshot cache lock poisoned".to_string())?;
    guard.last_entry = Some(ScreenshotCacheEntry {
      display_index: captured.display_index,
      region_key: region_key.clone(),
      path: captured.path.clone(),
      mime_type: "image/png".to_string(),
      base64_image: base64_image.clone(),
      captured_at_ms,
      display_info: display_info.clone(),
    });
  }

  let mut payload = serde_json::json!({
    "success": true,
    "reused": false,
    "path": captured.path,
    "displayIndex": captured.display_index,
    "displayInfo": display_info,
    "duplicateCallCount": request_count,
    "timestamp": chrono::Utc::now().to_rfc3339(),
    "mimeType": "image/png",
    "coordinateOverlay": true,
    "coordinateGrid": {
      "minorStepPx": 50,
      "majorStepPx": 100,
      "origin": "top-left",
      "coordinateSpace": "display"
    },
    "imageDataUrl": format!("{}{}", SCREENSHOT_DATA_URL_PREFIX, base64_image),
  });

  if force_refresh {
    payload["forceRefresh"] = Value::Bool(true);
  }
  if let Some(reason_text) = reason {
    payload["reason"] = Value::String(reason_text.to_string());
  }
  if let Some(region_value) = region {
    payload["region"] = serde_json::to_value(region_value).unwrap_or(Value::Null);
  }

  Ok(payload)
}

fn local_screenshot_mcp_call(
  request: McpCallRequest,
  app: &tauri::AppHandle,
) -> Result<mcp::McpCallResponse, String> {
  if !cfg!(target_os = "windows") {
    return Err("screenshot MCP is currently supported only on Windows".to_string());
  }

  let tool_name = request.tool_name.clone();
  let result_payload = match tool_name.as_str() {
    "list_screens" => {
      let script = format!(
        r#"
{}
[System.Windows.Forms.Screen]::AllScreens | ForEach-Object {{
  [PSCustomObject]@{{
    index = [array]::IndexOf([System.Windows.Forms.Screen]::AllScreens, $_)
    primary = $_.Primary
    x = $_.Bounds.X
    y = $_.Bounds.Y
    width = $_.Bounds.Width
    height = $_.Bounds.Height
    deviceName = $_.DeviceName
  }}
}} | ConvertTo-Json -Compress
"#,
        desktop_powershell_prelude(),
      );

      let output = run_powershell_script(&script)?;
      serde_json::from_str::<Value>(&output).unwrap_or(Value::String(output))
    }
    "capture_screenshot" => {
      let output_dir = if let Some(dir) = request.tool_args.get("outputDir").and_then(|value| value.as_str()) {
        PathBuf::from(dir)
      } else {
        let mut path = app.path().app_data_dir().map_err(|err| err.to_string())?;
        path.push("screenshots");
        path
      };

      fs::create_dir_all(&output_dir).map_err(|err| err.to_string())?;
      let escaped_dir = escape_powershell_single_quoted(&output_dir.display().to_string());
      let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S-%3f").to_string();
      let escaped_timestamp = escape_powershell_single_quoted(&timestamp);

      let script = format!(
        r#"
{}
$dir = '{escaped_dir}'
$ts = '{escaped_timestamp}'
New-Item -ItemType Directory -Force -Path $dir | Out-Null
$result = @()
$screens = [System.Windows.Forms.Screen]::AllScreens
for ($i = 0; $i -lt $screens.Length; $i++) {{
  $screen = $screens[$i]
  $bounds = $screen.Bounds
  $bmp = New-Object System.Drawing.Bitmap $bounds.Width, $bounds.Height
  $graphics = [System.Drawing.Graphics]::FromImage($bmp)
  $graphics.CopyFromScreen($bounds.X, $bounds.Y, 0, 0, $bounds.Size)
  $path = Join-Path $dir ('screenshot-' + $ts + '-' + $i + '.png')
  $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
  $graphics.Dispose()
  $bmp.Dispose()
  $result += [PSCustomObject]@{{
    index = $i
    path = $path
    primary = $screen.Primary
    x = $bounds.X
    y = $bounds.Y
    width = $bounds.Width
    height = $bounds.Height
  }}
}}
[PSCustomObject]@{{ allScreens = $true; forcedAllScreens = $true; outputDir = $dir; screenshots = $result }} | ConvertTo-Json -Compress
"#,
        desktop_powershell_prelude(),
      );

      let output = run_powershell_script(&script)?;
      serde_json::from_str::<Value>(&output).unwrap_or(Value::String(output))
    }
    "screenshot_for_display" => {
      let display_index = parse_i64_tool_arg(&request.tool_args, "displayIndex", "display_index").unwrap_or(0);
      let reason = parse_string_tool_arg(&request.tool_args, "reason", "reason");
      // User selected aggressive auto-refresh: default to true unless explicitly disabled.
      let force_refresh = parse_bool_tool_arg(&request.tool_args, "forceRefresh", "force_refresh").unwrap_or(true);
      let region = parse_screenshot_region(&request.tool_args)?;

      capture_screenshot_for_display_payload(
        app,
        display_index,
        region.as_ref(),
        reason,
        force_refresh,
      )?
    }
    _ => {
      return Err(format!("unsupported screenshot MCP tool: {}", tool_name));
    }
  };

  let formatted_result = if tool_name == "screenshot_for_display" {
    serde_json::to_string(&result_payload).unwrap_or_else(|_| "{}".to_string())
  } else {
    serde_json::to_string_pretty(&result_payload).unwrap_or_else(|_| result_payload.to_string())
  };

  Ok(mcp::McpCallResponse {
    server_name: request.name,
    tool_name,
    success: true,
    result: formatted_result,
    error: None,
  })
}

#[tauri::command]
async fn desktop_primary_display() -> Result<DesktopDisplayInfo, String> {
  let screenshot = desktop_capture_primary_display()?;
  Ok(DesktopDisplayInfo {
    primary: screenshot.primary,
    x: screenshot.x,
    y: screenshot.y,
    width: screenshot.width,
    height: screenshot.height,
    device_name: screenshot.device_name,
    scale_factor: screenshot.scale_factor,
  })
}

#[tauri::command]
async fn desktop_capture_primary_screenshot() -> Result<DesktopScreenshotResponse, String> {
  desktop_capture_primary_display()
}

#[tauri::command]
async fn desktop_capture_primary_annotated_screenshot() -> Result<DesktopScreenshotResponse, String> {
  desktop_capture_primary_display_with_overlay(true)
}

#[tauri::command]
async fn desktop_list_windows() -> Result<Vec<DesktopWindowInfo>, String> {
  desktop_list_windows_internal()
}

#[tauri::command]
async fn desktop_focus_window(request: DesktopWindowRequest) -> Result<DesktopWindowInfo, String> {
  let matched = desktop_match_window(&request)?;
  let handle_value = matched.handle.trim_start_matches("0x");
  let script = format!(
    r#"
{}
$handle = [IntPtr]::new([Int64]::Parse('{handle_value}', [System.Globalization.NumberStyles]::HexNumber))
[void][OpenCoworkDesktop]::ShowWindow($handle, 5)
Start-Sleep -Milliseconds 120
[void][Microsoft.VisualBasic.Interaction]::AppActivate({process_id})
Start-Sleep -Milliseconds 120
[void][OpenCoworkDesktop]::SetForegroundWindow($handle)
Start-Sleep -Milliseconds 150
$window = ConvertTo-OpenCoworkWindow -Handle $handle
if ($null -eq $window) {{
  throw 'desktop window disappeared after focus'
}}
$window | Select-Object title, processId, processName, handle, x, y, width, height, isForeground | ConvertTo-Json -Compress
"#,
    desktop_powershell_prelude(),
    handle_value = handle_value,
    process_id = matched.process_id,
  );

  run_powershell_json_script::<DesktopWindowInfo>(&script)
}

#[tauri::command]
async fn desktop_launch_app(request: DesktopLaunchRequest) -> Result<DesktopLaunchResponse, String> {
  ensure_windows_desktop_support()?;
  let args = request.args.unwrap_or_default();
  let mut command = Command::new(&request.path);
  if !args.is_empty() {
    command.args(&args);
  }
  if let Some(cwd) = request.cwd.as_ref().filter(|value| !value.trim().is_empty()) {
    command.current_dir(cwd);
  }

  let child = command
    .spawn()
    .map_err(|err| format!("failed to launch desktop app: {}", err))?;

  let delay_ms = request.initial_delay_ms.unwrap_or(1500);
  if delay_ms > 0 {
    thread::sleep(Duration::from_millis(delay_ms));
  }

  Ok(DesktopLaunchResponse {
    pid: child.id(),
    path: request.path,
    args,
  })
}

#[tauri::command]
async fn desktop_click(request: DesktopClickRequest) -> Result<DesktopActionResponse, String> {
  ensure_windows_desktop_support()?;
  let button = request.button.unwrap_or_else(|| "left".to_string()).to_lowercase();
  let (down_flag, up_flag) = match button.as_str() {
    "right" => ("0x0008", "0x0010"),
    _ => ("0x0002", "0x0004"),
  };
  let iterations = if request.double_click.unwrap_or(false) { 2 } else { 1 };
  let script = format!(
    r#"
{}
[void][OpenCoworkDesktop]::SetCursorPos({x}, {y})
Start-Sleep -Milliseconds 60
for ($i = 0; $i -lt {iterations}; $i++) {{
  [OpenCoworkDesktop]::mouse_event({down_flag}, 0, 0, 0, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 25
  [OpenCoworkDesktop]::mouse_event({up_flag}, 0, 0, 0, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 90
}}
[PSCustomObject]@{{ ok = $true; action = 'click' }} | ConvertTo-Json -Compress
"#,
    desktop_powershell_prelude(),
    x = request.x,
    y = request.y,
    iterations = iterations,
    down_flag = down_flag,
    up_flag = up_flag,
  );

  run_powershell_json_script::<DesktopActionResponse>(&script)
}

#[tauri::command]
async fn desktop_move_mouse(request: DesktopMoveMouseRequest) -> Result<DesktopActionResponse, String> {
  ensure_windows_desktop_support()?;
  let script = format!(
    r#"
{}
[void][OpenCoworkDesktop]::SetCursorPos({x}, {y})
[PSCustomObject]@{{ ok = $true; action = 'move_mouse' }} | ConvertTo-Json -Compress
"#,
    desktop_powershell_prelude(),
    x = request.x,
    y = request.y,
  );

  run_powershell_json_script::<DesktopActionResponse>(&script)
}

#[tauri::command]
async fn desktop_type_text(request: DesktopTypeRequest) -> Result<DesktopActionResponse, String> {
  ensure_windows_desktop_support()?;
  let text = escape_powershell_single_quoted(&request.text);
  let script = format!(
    r#"
{}
$text = '{text}'
Set-Clipboard -Value $text
Start-Sleep -Milliseconds 80
[System.Windows.Forms.SendKeys]::SendWait('^v')
Start-Sleep -Milliseconds 120
[PSCustomObject]@{{ ok = $true; action = 'type_text' }} | ConvertTo-Json -Compress
"#,
    desktop_powershell_prelude(),
    text = text,
  );

  run_powershell_json_script::<DesktopActionResponse>(&script)
}

#[tauri::command]
async fn desktop_keypress(request: DesktopKeypressRequest) -> Result<DesktopActionResponse, String> {
  ensure_windows_desktop_support()?;
  let keys_json = serde_json::to_string(&request.keys).map_err(|err| err.to_string())?;
  let keys_json = escape_powershell_single_quoted(&keys_json);
  let script = format!(
    r#"
{}
$keys = ConvertFrom-Json '{keys_json}'
$modifierMap = @{{
  'CTRL' = '^'
  'CONTROL' = '^'
  'ALT' = '%'
  'SHIFT' = '+'
}}
$keyMap = @{{
  'ENTER' = '{{ENTER}}'
  'TAB' = '{{TAB}}'
  'ESC' = '{{ESC}}'
  'ESCAPE' = '{{ESC}}'
  'UP' = '{{UP}}'
  'DOWN' = '{{DOWN}}'
  'LEFT' = '{{LEFT}}'
  'RIGHT' = '{{RIGHT}}'
  'BACKSPACE' = '{{BACKSPACE}}'
  'DELETE' = '{{DELETE}}'
  'HOME' = '{{HOME}}'
  'END' = '{{END}}'
  'PAGEUP' = '{{PGUP}}'
  'PAGEDOWN' = '{{PGDN}}'
  'SPACE' = ' '
  'F1' = '{{F1}}'
  'F2' = '{{F2}}'
  'F3' = '{{F3}}'
  'F4' = '{{F4}}'
  'F5' = '{{F5}}'
  'F6' = '{{F6}}'
  'F7' = '{{F7}}'
  'F8' = '{{F8}}'
  'F9' = '{{F9}}'
  'F10' = '{{F10}}'
  'F11' = '{{F11}}'
  'F12' = '{{F12}}'
}}
$modifiers = ''
$resolved = @()
foreach ($rawKey in $keys) {{
  $upperKey = [string]$rawKey
  $upperKey = $upperKey.ToUpperInvariant()
  if ($modifierMap.ContainsKey($upperKey)) {{
    $modifiers += $modifierMap[$upperKey]
  }} elseif ($keyMap.ContainsKey($upperKey)) {{
    $resolved += $keyMap[$upperKey]
  }} elseif ($upperKey.Length -eq 1) {{
    $resolved += $upperKey
  }} else {{
    $resolved += ('{{' + $upperKey + '}}')
  }}
}}
if ($resolved.Count -eq 0) {{
  throw 'desktop_keypress requires at least one non-modifier key'
}}
foreach ($entry in $resolved) {{
  [System.Windows.Forms.SendKeys]::SendWait($modifiers + $entry)
  Start-Sleep -Milliseconds 70
}}
[PSCustomObject]@{{ ok = $true; action = 'keypress' }} | ConvertTo-Json -Compress
"#,
    desktop_powershell_prelude(),
    keys_json = keys_json,
  );

  run_powershell_json_script::<DesktopActionResponse>(&script)
}

#[tauri::command]
async fn desktop_scroll(request: DesktopScrollRequest) -> Result<DesktopActionResponse, String> {
  ensure_windows_desktop_support()?;
  let maybe_move = match (request.x, request.y) {
    (Some(x), Some(y)) => format!("[void][OpenCoworkDesktop]::SetCursorPos({}, {})", x, y),
    _ => String::new(),
  };
  let script = format!(
    r#"
{}
{maybe_move}
Start-Sleep -Milliseconds 60
[OpenCoworkDesktop]::mouse_event(0x0800, 0, 0, [uint32]([int]{scroll_y}), [UIntPtr]::Zero)
[PSCustomObject]@{{ ok = $true; action = 'scroll' }} | ConvertTo-Json -Compress
"#,
    desktop_powershell_prelude(),
    maybe_move = maybe_move,
    scroll_y = request.scroll_y,
  );

  run_powershell_json_script::<DesktopActionResponse>(&script)
}

fn local_docs_mcp_call(
  request: McpCallRequest,
  state: tauri::State<'_, Arc<Database>>,
) -> Result<mcp::McpCallResponse, String> {
  let tool_name = request.tool_name.clone();

  if tool_name == "list_allowed_folders" {
    let folders = state.list_allowed_folders().map_err(|err| err.to_string())?;
    return Ok(mcp::McpCallResponse {
      server_name: request.name,
      tool_name,
      success: true,
      result: serde_json::to_string_pretty(&folders).unwrap_or_else(|_| "[]".to_string()),
      error: None,
    });
  }

  let path = request
    .tool_args
    .get("path")
    .and_then(|value| value.as_str())
    .ok_or_else(|| "missing required argument: path".to_string())?;

  let allowed_folders = state.list_allowed_folders().map_err(|err| err.to_string())?;
  let canonical_target = file_safety::ensure_path_allowed(PathBuf::from(path).as_path(), &allowed_folders)?;
  let text = artifact_pipeline::extract_text_for_llm(canonical_target.as_path())?;

  let result = match tool_name.as_str() {
    "extract_full_text" => text,
    "get_chunk" => {
      let start = request
        .tool_args
        .get("start")
        .and_then(|value| value.as_u64())
        .unwrap_or(0) as usize;
      let length = request
        .tool_args
        .get("length")
        .and_then(|value| value.as_u64())
        .unwrap_or(8_000) as usize;

      text.chars().skip(start).take(length).collect::<String>()
    }
    "search_in_document" => {
      let query = request
        .tool_args
        .get("query")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "missing required argument: query".to_string())?
        .to_lowercase();
      let limit = request
        .tool_args
        .get("limit")
        .and_then(|value| value.as_u64())
        .unwrap_or(12) as usize;

      let mut matches: Vec<String> = Vec::new();
      for line in text.lines() {
        if line.to_lowercase().contains(&query) {
          matches.push(line.to_string());
          if matches.len() >= limit {
            break;
          }
        }
      }

      serde_json::to_string_pretty(&matches).unwrap_or_else(|_| "[]".to_string())
    }
    _ => {
      return Err(format!("unsupported local docs MCP tool: {}", tool_name));
    }
  };

  Ok(mcp::McpCallResponse {
    server_name: request.name,
    tool_name,
    success: true,
    result,
    error: None,
  })
}

#[tauri::command]
async fn mcp_runtime_start(request: McpServerRequest) -> Result<McpRuntimeServerStatus, String> {
  runtime_start_server(request).map_err(map_mcp_error)
}

#[tauri::command]
async fn mcp_runtime_stop(name: String) -> Result<bool, String> {
  runtime_stop_server(&name).map_err(map_mcp_error)
}

#[tauri::command]
async fn mcp_runtime_restart(request: McpServerRequest) -> Result<McpRuntimeServerStatus, String> {
  runtime_restart_server(request).map_err(map_mcp_error)
}

#[tauri::command]
async fn mcp_runtime_list() -> Result<Vec<McpRuntimeServerStatus>, String> {
  runtime_list_servers().map_err(map_mcp_error)
}

#[tauri::command]
async fn mcp_probe(request: McpServerRequest) -> Result<mcp::McpProbeResponse, String> {
  if request.command.trim() == LOCAL_DOCS_MCP_COMMAND {
    return Ok(local_docs_mcp_probe(request.name));
  }

  if request.command.trim() == LOCAL_SCREENSHOT_MCP_COMMAND {
    return Ok(local_screenshot_mcp_probe(request.name));
  }

  if runtime_has_server(&request.name) {
    return runtime_probe_server(&request.name).map_err(map_mcp_error);
  }

  probe_server(request).map_err(map_mcp_error)
}

#[tauri::command]
async fn mcp_call_tool(
  app: tauri::AppHandle,
  request: McpCallRequest,
  state: tauri::State<'_, Arc<Database>>,
  run_id: Option<String>,
) -> Result<mcp::McpCallResponse, String> {
  let policy = load_policy_state(&state)?;
  enforce_tool_policy(
    &policy,
    "mcp",
    &format!("{}::{}", request.name, request.tool_name),
    policy.flags.allow_mcp_tool_calls,
  )?;
  if let Some(sandbox) = load_run_sandbox(&state, run_id.as_deref())? {
    enforce_worker_sandbox_flag(&sandbox, sandbox.allow_mcp, "mcp-aufrufe")?;
  }

  if request.command.trim() == LOCAL_DOCS_MCP_COMMAND {
    return local_docs_mcp_call(request, state);
  }

  if request.command.trim() == LOCAL_SCREENSHOT_MCP_COMMAND {
    return local_screenshot_mcp_call(request, &app);
  }

  if runtime_has_server(&request.name) {
    return runtime_call_tool(&request.name, &request.tool_name, request.tool_args.clone())
      .map_err(map_mcp_error);
  }

  call_tool(request).map_err(map_mcp_error)
}

#[tauri::command]
async fn web_fetch_url(
  app: tauri::AppHandle,
  state: tauri::State<'_, Arc<Database>>,
  request: WebFetchRequest,
  run_id: Option<String>,
) -> Result<WebFetchResponse, String> {
  let requested_url = request.url.trim();
  if requested_url.is_empty() {
    return Err("url darf nicht leer sein".to_string());
  }

  let policy = load_policy_state(&state)?;
  enforce_tool_policy(&policy, "web_fetch", requested_url, policy.flags.allow_web_fetch)?;
  if let Some(sandbox) = load_run_sandbox(&state, run_id.as_deref())? {
    enforce_worker_sandbox_flag(&sandbox, sandbox.allow_web_fetch, "web-fetch")?;
  }

  let max_chars = request.max_chars.unwrap_or(4_000).clamp(500, 30_000);
  let client = reqwest::Client::builder()
    .timeout(Duration::from_secs(30))
    .build()
    .map_err(|err| err.to_string())?;
  let response = client
    .get(requested_url)
    .send()
    .await
    .map_err(|err| err.to_string())?;
  let status = response.status();
  let body = response.text().await.map_err(|err| err.to_string())?;

  let title = extract_html_title(&body);
  let text = strip_html_like_content(&body);
  let trimmed = text.trim().to_string();
  let content: String = trimmed.chars().take(max_chars).collect();
  let truncated = trimmed.chars().count() > max_chars;

  let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
  let details = serde_json::json!({
    "url": requested_url,
    "status": status.as_u16(),
    "maxChars": max_chars,
    "truncated": truncated,
    "contentChars": content.chars().count(),
  });
  let _ = audit::append_audit_event(app_data_dir, "web", "fetch_url", Some(details));

  Ok(WebFetchResponse {
    url: requested_url.to_string(),
    status: status.as_u16(),
    ok: status == StatusCode::OK,
    title,
    content,
    truncated,
  })
}

#[tauri::command]
async fn web_search(
  app: tauri::AppHandle,
  state: tauri::State<'_, Arc<Database>>,
  request: WebSearchRequest,
  run_id: Option<String>,
) -> Result<WebSearchResponse, String> {
  let query = request.query.trim();
  if query.is_empty() {
    return Err("query darf nicht leer sein".to_string());
  }

  let policy = load_policy_state(&state)?;
  enforce_tool_policy(&policy, "web_search", query, policy.flags.allow_web_search)?;
  if let Some(sandbox) = load_run_sandbox(&state, run_id.as_deref())? {
    enforce_worker_sandbox_flag(&sandbox, sandbox.allow_web_search, "web-search")?;
  }

  let max_results = request.max_results.unwrap_or(5).clamp(1, 10);
  let encoded_query = url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>();
  let search_url = format!("https://html.duckduckgo.com/html/?q={}", encoded_query);
  let client = reqwest::Client::builder()
    .timeout(Duration::from_secs(30))
    .build()
    .map_err(|err| err.to_string())?;
  let body = client
    .get(&search_url)
    .send()
    .await
    .map_err(|err| err.to_string())?
    .text()
    .await
    .map_err(|err| err.to_string())?;

  let results = parse_duckduckgo_results(&body, max_results);

  let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
  let details = serde_json::json!({
    "query": query,
    "resultCount": results.len(),
  });
  let _ = audit::append_audit_event(app_data_dir, "web", "search", Some(details));

  Ok(WebSearchResponse {
    query: query.to_string(),
    results,
  })
}

#[tauri::command]
fn exec_command(
  app: tauri::AppHandle,
  state: tauri::State<'_, Arc<Database>>,
  command: String,
  cwd: Option<String>,
  timeout_ms: Option<u64>,
  stream_id: Option<String>,
  retry_count: Option<u32>,
  retry_backoff_ms: Option<u64>,
  run_id: Option<String>,
  backend_id: Option<String>,
) -> Result<ExecCommandResponse, String> {
  let request = ExecCommandRequest {
    command,
    cwd,
    timeout_ms,
    stream_id,
    retry_count,
    retry_backoff_ms,
    run_id,
    backend_id,
  };

  let command_text = request.command.trim();
  if command_text.is_empty() {
    return Err("command darf nicht leer sein".to_string());
  }

  let policy = load_policy_state(&state)?;
  enforce_tool_policy(
    &policy,
    "shell",
    command_text,
    policy.flags.allow_shell_execution,
  )?;

  if let Some(sandbox) = load_run_sandbox(&state, request.run_id.as_deref())? {
    enforce_worker_sandbox_flag(&sandbox, sandbox.allow_shell_execution, "shell-ausfuehrung")?;
    if process_manager::detect_admin_requirement(command_text) {
      return Err("sandbox blockiert shell-kommandos mit admin/elevation-anforderung".to_string());
    }
  }

  let timeout_ms = request.timeout_ms.unwrap_or(30_000).clamp(1_000, 600_000);
  let retry_count = request.retry_count.unwrap_or(0).min(3);
  let retry_backoff_ms = request.retry_backoff_ms.unwrap_or(1_000).clamp(100, 30_000);
  let start = Instant::now();
  let effective_cwd = ensure_run_cwd(&state, request.run_id.as_deref(), request.cwd.as_deref())?;
  enforce_shell_command_guard(
    &state,
    request.run_id.as_deref(),
    command_text,
    effective_cwd.as_deref(),
  )?;

  let (shell_override, env_vars, runtime_mode) = resolve_exec_runtime(
    &state,
    request.backend_id.as_deref(),
    request.run_id.as_deref(),
  )?;

  let mut last_response = ExecCommandResponse {
    stdout: String::new(),
    stderr: String::new(),
    exit_code: None,
    current_cwd: effective_cwd.clone(),
    timed_out: false,
    duration_ms: 0,
    attempts: 0,
    normalized_status: "error".to_string(),
  };
  let mut last_error: Option<String> = None;

  for attempt in 0..=retry_count {
    last_response.attempts = attempt + 1;
    match run_command_once(
      &app,
      request.stream_id.as_deref(),
      command_text,
      effective_cwd.as_deref(),
      timeout_ms,
      shell_override.as_deref(),
      runtime_mode.as_deref(),
      &env_vars,
    ) {
      Ok(response) => {
        last_response = ExecCommandResponse {
          attempts: attempt + 1,
          duration_ms: start.elapsed().as_millis() as u64,
          ..response
        };

        if last_response.normalized_status == "success" || attempt == retry_count {
          break;
        }

        thread::sleep(Duration::from_millis(retry_backoff_ms * (attempt as u64 + 1)));
      }
      Err(err) => {
        last_error = Some(err.clone());
        last_response.stderr = err;
        last_response.duration_ms = start.elapsed().as_millis() as u64;
        last_response.normalized_status = "spawn_error".to_string();

        if attempt == retry_count {
          break;
        }

        thread::sleep(Duration::from_millis(retry_backoff_ms * (attempt as u64 + 1)));
      }
    }
  }

  if let Some(run_id) = request.run_id.as_deref() {
    let payload = serde_json::json!({
      "command": command_text,
      "cwd": request.cwd,
      "backendId": request.backend_id,
      "stdout": truncate_chars(&last_response.stdout, 4000),
      "stderr": truncate_chars(&last_response.stderr, 4000),
      "exitCode": last_response.exit_code,
      "timedOut": last_response.timed_out,
      "status": last_response.normalized_status,
      "attempts": last_response.attempts,
      "error": last_error,
    });
    let payload_text = payload.to_string();
    let _ = state.insert_engine_run_event(
      &uuid::Uuid::new_v4().to_string(),
      run_id,
      "exec_command",
      Some(&payload_text),
    );
  }

  let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
  let details = serde_json::json!({
    "command": command_text,
    "cwd": effective_cwd,
    "backendId": request.backend_id,
    "exitCode": last_response.exit_code,
    "timedOut": last_response.timed_out,
    "status": last_response.normalized_status,
    "attempts": last_response.attempts,
    "durationMs": last_response.duration_ms,
  });
  let _ = audit::append_audit_event(app_data_dir, "shell", "exec_command", Some(details));

  Ok(last_response)
}

// -- Persistence commands ---------------------------------------------------

#[tauri::command]
fn db_save_thread(
  state: tauri::State<'_, Arc<Database>>,
  id: String,
  title: String,
  created_at: String,
  provider_settings_json: Option<String>,
  permission_config_json: Option<String>,
) -> Result<(), String> {
  state.insert_thread(
    &id,
    &title,
    &created_at,
    provider_settings_json.as_deref(),
    permission_config_json.as_deref(),
  ).map_err(|e| e.to_string())
}

#[tauri::command]
fn db_list_threads(state: tauri::State<'_, Arc<Database>>) -> Result<Vec<ThreadRow>, String> {
  state.list_threads().map_err(|e| e.to_string()).map(|rows| {
    rows.into_iter().map(|(id, title, ca, ua, provider_settings_json, permission_config_json)| ThreadRow {
      id,
      title,
      created_at: ca,
      updated_at: ua,
      provider_settings_json,
      permission_config_json,
    }).collect()
  })
}

#[tauri::command]
fn db_update_thread_provider_settings(
  state: tauri::State<'_, Arc<Database>>,
  id: String,
  provider_settings_json: Option<String>,
) -> Result<(), String> {
  state.update_thread_provider_settings(&id, provider_settings_json.as_deref()).map_err(|e| e.to_string())
}

#[tauri::command]
fn db_delete_thread(state: tauri::State<'_, Arc<Database>>, id: String) -> Result<(), String> {
  state.delete_thread(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn db_save_message(state: tauri::State<'_, Arc<Database>>, id: String, thread_id: String, role: String, content: String, timestamp: i64) -> Result<(), String> {
  state.insert_message(&id, &thread_id, &role, &content, timestamp).map_err(|e| e.to_string())
}

#[tauri::command]
fn db_update_message_content(state: tauri::State<'_, Arc<Database>>, id: String, content: String) -> Result<(), String> {
  state.update_message_content(&id, &content).map_err(|e| e.to_string())
}

#[tauri::command]
fn db_delete_messages(
  state: tauri::State<'_, Arc<Database>>,
  ids: Vec<String>,
) -> Result<DeletedMessagesResponse, String> {
  let deleted_count = state.delete_messages(&ids).map_err(|e| e.to_string())?;
  Ok(DeletedMessagesResponse { deleted_count })
}

#[tauri::command]
fn db_list_messages(state: tauri::State<'_, Arc<Database>>, thread_id: String) -> Result<Vec<MessageRow>, String> {
  state.list_messages(&thread_id).map_err(|e| e.to_string()).map(|rows| {
    rows.into_iter().map(|(id, role, content, ts)| MessageRow { id, role, content, timestamp: ts }).collect()
  })
}

#[tauri::command]
fn db_save_task(
  state: tauri::State<'_, Arc<Database>>,
  id: String, title: String, prompt: String, status: String,
  thread_id: Option<String>, created_at: String,
) -> Result<(), String> {
  state.insert_task(&id, &title, &prompt, &status, thread_id.as_deref(), &created_at).map_err(|e| e.to_string())
}

#[tauri::command]
fn db_update_task_status(state: tauri::State<'_, Arc<Database>>, id: String, status: String) -> Result<(), String> {
  state.update_task_status(&id, &status).map_err(|e| e.to_string())
}

#[tauri::command]
fn db_list_tasks(state: tauri::State<'_, Arc<Database>>) -> Result<Vec<TaskRow>, String> {
  state.list_tasks().map_err(|e| e.to_string()).map(|rows| {
    rows.into_iter().map(|(id, title, prompt, status, thread_id, ca, ua, error)| {
      TaskRow { id, title, prompt, status, thread_id, created_at: ca, updated_at: ua, error }
    }).collect()
  })
}

#[tauri::command]
fn db_save_step(
  state: tauri::State<'_, Arc<Database>>,
  id: String, task_id: String, idx: i32, title: String, state_val: String,
  requires_approval: bool, risk_level: String,
) -> Result<(), String> {
  state.insert_step(&id, &task_id, idx, &title, &state_val, requires_approval, &risk_level).map_err(|e| e.to_string())
}

#[tauri::command]
fn db_update_step(state: tauri::State<'_, Arc<Database>>, id: String, state_val: String, output: Option<String>) -> Result<(), String> {
  state.update_step_state(&id, &state_val, output.as_deref()).map_err(|e| e.to_string())
}

#[tauri::command]
fn db_list_steps(state: tauri::State<'_, Arc<Database>>, task_id: String) -> Result<Vec<StepRow>, String> {
  state.list_steps(&task_id).map_err(|e| e.to_string()).map(|rows| {
    rows.into_iter().map(|(id, idx, title, st, ra, rl, output)| {
      StepRow { id, idx, title, state: st, requires_approval: ra, risk_level: rl, output }
    }).collect()
  })
}

#[tauri::command]
fn execute_task(
  app: tauri::AppHandle,
  state: tauri::State<'_, Arc<Database>>,
  task_id: String,
) -> Result<(), String> {
  let task_exists = state
    .list_tasks()
    .map_err(|e| e.to_string())?
    .into_iter()
    .any(|(id, _, _, _, _, _, _, _)| id == task_id);
  if !task_exists {
    return Err("task not found".to_string());
  }

  let steps = state.list_steps(&task_id).map_err(|e| e.to_string())?;
  if steps.is_empty() {
    state.set_task_error(&task_id, "task has no steps").map_err(|e| e.to_string())?;
    return Err("task has no steps".to_string());
  }

  state
    .update_task_status(&task_id, "running")
    .map_err(|e| e.to_string())?;

  let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
  let _ = audit::append_audit_event(
    app_data_dir.clone(),
    "task_engine",
    "execute_task_started",
    Some(serde_json::json!({ "taskId": task_id, "stepCount": steps.len() })),
  );

  let task_id_for_audit = task_id.clone();
  let execution = (|| -> Result<(), String> {
    for (step_id, _, title, _, _, _, _) in steps {
      let current_status = state
        .list_tasks()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|(id, _, _, _, _, _, _, _)| id == &task_id)
        .map(|(_, _, _, status, _, _, _, _)| status)
        .unwrap_or_else(|| "failed".to_string());

      if current_status == "cancelled" {
        state
          .update_step_state(&step_id, "skipped", Some("Task wurde abgebrochen"))
          .map_err(|e| e.to_string())?;
        return Ok(());
      }

      state
        .update_step_state(&step_id, "running", None)
        .map_err(|e| e.to_string())?;
      thread::sleep(Duration::from_millis(50));

      let output = format!("Automatisch ausgefuehrt: {}", title);
      state
        .update_step_state(&step_id, "completed", Some(&output))
        .map_err(|e| e.to_string())?;
    }

    state
      .update_task_status(&task_id, "completed")
      .map_err(|e| e.to_string())?;
    Ok(())
  })();

  match execution {
    Ok(()) => {
      let _ = audit::append_audit_event(
        app_data_dir,
        "task_engine",
        "execute_task_completed",
        Some(serde_json::json!({ "taskId": task_id_for_audit })),
      );
      Ok(())
    }
    Err(err) => {
      let _ = state.set_task_error(&task_id, &err);
      let _ = audit::append_audit_event(
        app_data_dir,
        "task_engine",
        "execute_task_failed",
        Some(serde_json::json!({ "taskId": task_id_for_audit, "error": err })),
      );
      Err("task execution failed".to_string())
    }
  }
}

#[tauri::command]
fn audit_event(
  app: tauri::AppHandle,
  area: String,
  action: String,
  details: Option<Value>,
) -> Result<(), String> {
  let app_data_dir = app
    .path()
    .app_data_dir()
    .map_err(|err| err.to_string())?;

  audit::append_audit_event(app_data_dir, &area, &action, details)
}

#[tauri::command]
fn fs_list_allowed_folders(state: tauri::State<'_, Arc<Database>>) -> Result<Vec<String>, String> {
  state.list_allowed_folders().map_err(|err| err.to_string())
}

#[tauri::command]
fn fs_add_allowed_folder(state: tauri::State<'_, Arc<Database>>, path: String) -> Result<(), String> {
  let canonical = PathBuf::from(path)
    .canonicalize()
    .map_err(|err| err.to_string())?;
  state
    .add_allowed_folder(&canonical.display().to_string())
    .map_err(|err| err.to_string())
}

#[tauri::command]
fn fs_remove_allowed_folder(state: tauri::State<'_, Arc<Database>>, path: String) -> Result<(), String> {
  state.remove_allowed_folder(&path).map_err(|err| err.to_string())
}

fn sanitize_attachment_file_name(value: &str) -> String {
  let sanitized: String = value
    .chars()
    .map(|ch| match ch {
      '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
      ch if ch.is_control() => '_',
      ch => ch,
    })
    .collect();

  let trimmed = sanitized.trim().trim_matches('.').to_string();
  if trimmed.is_empty() {
    "attachment".to_string()
  } else {
    trimmed
  }
}

#[tauri::command]
fn fs_import_attachment(
  app: tauri::AppHandle,
  state: tauri::State<'_, Arc<Database>>,
  path: String,
) -> Result<ImportedAttachmentRow, String> {
  let source = PathBuf::from(&path)
    .canonicalize()
    .map_err(|err| err.to_string())?;
  let metadata = fs::metadata(&source).map_err(|err| err.to_string())?;
  if !metadata.is_file() {
    return Err("attachment source is not a file".to_string());
  }

  let mut attachment_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
  attachment_dir.push("attachments");
  fs::create_dir_all(&attachment_dir).map_err(|err| err.to_string())?;

  let original_name = source
    .file_name()
    .and_then(|value| value.to_str())
    .unwrap_or("attachment");
  let safe_name = sanitize_attachment_file_name(original_name);
  let target_name = format!("{}_{}", uuid::Uuid::new_v4(), safe_name);
  let target_path = attachment_dir.join(target_name);
  fs::copy(&source, &target_path).map_err(|err| err.to_string())?;

  let canonical_attachment_dir = attachment_dir
    .canonicalize()
    .map_err(|err| err.to_string())?;
  state
    .add_allowed_folder(&canonical_attachment_dir.display().to_string())
    .map_err(|err| err.to_string())?;

  let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
  let details = serde_json::json!({
    "originalPath": source.display().to_string(),
    "importedPath": target_path.display().to_string(),
    "sizeBytes": metadata.len(),
  });
  let _ = audit::append_audit_event(app_data_dir, "file_safety", "import_attachment", Some(details));

  Ok(ImportedAttachmentRow {
    original_path: source.display().to_string(),
    imported_path: target_path.display().to_string(),
    file_name: original_name.to_string(),
    size_bytes: metadata.len(),
  })
}

fn infer_language_from_extension(path: &Path) -> Option<String> {
  let ext = path
    .extension()
    .and_then(|value| value.to_str())?
    .to_lowercase();

  let language = match ext.as_str() {
    "rs" => "Rust",
    "ts" | "tsx" => "TypeScript",
    "js" | "jsx" | "mjs" | "cjs" => "JavaScript",
    "py" => "Python",
    "java" => "Java",
    "kt" | "kts" => "Kotlin",
    "cs" => "C#",
    "cpp" | "cc" | "cxx" | "hpp" | "h" => "C/C++",
    "go" => "Go",
    "php" => "PHP",
    "rb" => "Ruby",
    "swift" => "Swift",
    "scala" => "Scala",
    "sh" | "bash" | "zsh" | "ps1" => "Shell",
    "sql" => "SQL",
    "html" | "htm" => "HTML",
    "css" | "scss" | "sass" | "less" => "CSS",
    "json" => "JSON",
    "yaml" | "yml" => "YAML",
    "toml" => "TOML",
    "xml" => "XML",
    "md" => "Markdown",
    _ => return None,
  };

  Some(language.to_string())
}

fn push_metadata_entry(path: &Path, metadata: &fs::Metadata, files: &mut Vec<FsAttachmentMetadataEntry>) {
  let file_name = path
    .file_name()
    .and_then(|value| value.to_str())
    .unwrap_or_default()
    .to_string();
  let extension = path
    .extension()
    .and_then(|value| value.to_str())
    .map(|value| value.to_lowercase());

  files.push(FsAttachmentMetadataEntry {
    path: path.display().to_string(),
    file_name,
    extension,
    language: infer_language_from_extension(path),
    size_bytes: metadata.len(),
  });
}

#[tauri::command]
fn fs_collect_attachment_metadata(
  state: tauri::State<'_, Arc<Database>>,
  path: String,
  max_entries: Option<usize>,
  run_id: Option<String>,
) -> Result<FsAttachmentMetadataResponse, String> {
  let policy = load_policy_state(&state)?;
  enforce_tool_policy(
    &policy,
    "read_file",
    path.as_str(),
    policy.flags.allow_file_read_extraction,
  )?;

  let allowed_folders = resolve_allowed_folders_for_run(&state, run_id.as_deref())?;
  let canonical_target = file_safety::ensure_path_allowed(PathBuf::from(&path).as_path(), &allowed_folders)?;
  let bounded_max_entries = max_entries.unwrap_or(120).clamp(1, 2_000);

  let mut files: Vec<FsAttachmentMetadataEntry> = Vec::new();
  let mut total_files: usize = 0;

  if canonical_target.is_file() {
    let metadata = fs::metadata(&canonical_target).map_err(|err| err.to_string())?;
    total_files = 1;
    push_metadata_entry(&canonical_target, &metadata, &mut files);

    return Ok(FsAttachmentMetadataResponse {
      root_path: canonical_target.display().to_string(),
      root_kind: "file".to_string(),
      total_files,
      returned_files: files.len(),
      truncated: false,
      files,
    });
  }

  let mut stack = vec![canonical_target.clone()];
  while let Some(current_dir) = stack.pop() {
    let entries = fs::read_dir(&current_dir).map_err(|err| err.to_string())?;
    for entry in entries {
      let entry = entry.map_err(|err| err.to_string())?;
      let candidate_path = entry.path();
      let file_type = entry.file_type().map_err(|err| err.to_string())?;

      if file_type.is_symlink() {
        continue;
      }

      if file_type.is_dir() {
        stack.push(candidate_path);
        continue;
      }

      if file_type.is_file() {
        total_files += 1;
        if files.len() < bounded_max_entries {
          let metadata = entry.metadata().map_err(|err| err.to_string())?;
          push_metadata_entry(&candidate_path, &metadata, &mut files);
        }
      }
    }
  }

  Ok(FsAttachmentMetadataResponse {
    root_path: canonical_target.display().to_string(),
    root_kind: "folder".to_string(),
    total_files,
    returned_files: files.len(),
    truncated: total_files > files.len(),
    files,
  })
}

#[tauri::command]
fn fs_write_text_file(
  app: tauri::AppHandle,
  state: tauri::State<'_, Arc<Database>>,
  path: String,
  content: String,
  create_backup: bool,
  run_id: Option<String>,
) -> Result<file_safety::FileWriteResponse, String> {
  let canonical_target = ensure_run_file_access(&state, run_id.as_deref(), &path, true)?;

  let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
  let response = file_safety::write_text_file(&app_data_dir, &canonical_target, &content, create_backup)?;

  let details = file_safety::write_file_audit_details(
    &response.path,
    response.backup_path.as_deref(),
    response.bytes_written,
  );
  let _ = audit::append_audit_event(app_data_dir, "file_safety", "write_text_file", Some(details));

  Ok(response)
}

#[tauri::command]
fn fs_create_directory(
  app: tauri::AppHandle,
  state: tauri::State<'_, Arc<Database>>,
  path: String,
  run_id: Option<String>,
) -> Result<file_safety::DirectoryCreateResponse, String> {
  let canonical_target = ensure_run_file_access(&state, run_id.as_deref(), &path, true)?;
  let response = file_safety::create_directory(&canonical_target)?;

  let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
  let details = file_safety::create_directory_audit_details(&response.path, response.created);
  let _ = audit::append_audit_event(app_data_dir, "file_safety", "create_directory", Some(details));

  Ok(response)
}

#[tauri::command]
fn fs_move_path(
  app: tauri::AppHandle,
  state: tauri::State<'_, Arc<Database>>,
  source_path: String,
  destination_path: String,
  overwrite: bool,
  run_id: Option<String>,
) -> Result<file_safety::PathMutationResponse, String> {
  let canonical_source = ensure_run_file_access(&state, run_id.as_deref(), &source_path, true)?;
  let canonical_destination = ensure_run_file_access(&state, run_id.as_deref(), &destination_path, true)?;
  let response = file_safety::move_path(&canonical_source, &canonical_destination, overwrite)?;

  let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
  let details = file_safety::mutate_path_audit_details(
    "move",
    &response.source_path,
    &response.destination_path,
    &response.item_kind,
    response.created_parent,
    response.replaced_existing,
  );
  let _ = audit::append_audit_event(app_data_dir, "file_safety", "move_path", Some(details));

  Ok(response)
}

#[tauri::command]
fn fs_copy_path(
  app: tauri::AppHandle,
  state: tauri::State<'_, Arc<Database>>,
  source_path: String,
  destination_path: String,
  overwrite: bool,
  run_id: Option<String>,
) -> Result<file_safety::PathMutationResponse, String> {
  let canonical_source = ensure_run_file_access(&state, run_id.as_deref(), &source_path, false)?;
  let canonical_destination = ensure_run_file_access(&state, run_id.as_deref(), &destination_path, true)?;
  let response = file_safety::copy_path(&canonical_source, &canonical_destination, overwrite)?;

  let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
  let details = file_safety::mutate_path_audit_details(
    "copy",
    &response.source_path,
    &response.destination_path,
    &response.item_kind,
    response.created_parent,
    response.replaced_existing,
  );
  let _ = audit::append_audit_event(app_data_dir, "file_safety", "copy_path", Some(details));

  Ok(response)
}

#[tauri::command]
fn fs_delete_file(
  app: tauri::AppHandle,
  state: tauri::State<'_, Arc<Database>>,
  path: String,
  confirm_token: String,
) -> Result<(), String> {
  let allowed_folders = state.list_allowed_folders().map_err(|err| err.to_string())?;
  let canonical_target = file_safety::ensure_path_allowed(PathBuf::from(&path).as_path(), &allowed_folders)?;

  file_safety::delete_file(&canonical_target, &confirm_token)?;

  let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
  let details = file_safety::delete_file_audit_details(&canonical_target.display().to_string());
  let _ = audit::append_audit_event(app_data_dir, "file_safety", "delete_file", Some(details));

  Ok(())
}

#[tauri::command]
fn fs_list_backups(app: tauri::AppHandle) -> Result<Vec<file_safety::BackupEntry>, String> {
  let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
  file_safety::list_backups(&app_data_dir)
}

#[tauri::command]
fn fs_restore_backup(
  app: tauri::AppHandle,
  state: tauri::State<'_, Arc<Database>>,
  backup_file_name: String,
  target_path: String,
  create_backup: bool,
) -> Result<file_safety::FileWriteResponse, String> {
  let allowed_folders = state.list_allowed_folders().map_err(|err| err.to_string())?;
  let canonical_target = file_safety::ensure_path_allowed(PathBuf::from(&target_path).as_path(), &allowed_folders)?;

  let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
  let response = file_safety::restore_backup(
    &app_data_dir,
    &backup_file_name,
    &canonical_target,
    create_backup,
  )?;

  let details = file_safety::restore_file_audit_details(&response.path, &backup_file_name);
  let _ = audit::append_audit_event(app_data_dir, "file_safety", "restore_backup", Some(details));

  Ok(response)
}

#[tauri::command]
fn fs_watch_list(watch_registry: tauri::State<'_, WatchRegistry>) -> Result<Vec<String>, String> {
  let watchers = watch_registry.watchers.lock().map_err(|_| "watch registry is poisoned")?;
  Ok(watchers.keys().cloned().collect())
}

#[tauri::command]
fn fs_watch_start(
  app: tauri::AppHandle,
  state: tauri::State<'_, Arc<Database>>,
  watch_registry: tauri::State<'_, WatchRegistry>,
  path: String,
) -> Result<(), String> {
  let allowed_folders = state.list_allowed_folders().map_err(|err| err.to_string())?;
  let canonical_target = file_safety::ensure_path_allowed(PathBuf::from(&path).as_path(), &allowed_folders)?;
  let watched_path = canonical_target.display().to_string();

  {
    let watchers = watch_registry.watchers.lock().map_err(|_| "watch registry is poisoned")?;
    if watchers.contains_key(&watched_path) {
      return Ok(());
    }
  }

  let app_handle = app.clone();
  let watched_path_for_callback = watched_path.clone();

  let mut watcher = notify::recommended_watcher(move |result: Result<notify::Event, notify::Error>| {
    if let Ok(event) = result {
      let payload = file_watch::to_payload(&watched_path_for_callback, &event);
      let _ = app_handle.emit("file_safety://watch_event", payload.clone());

      if let Ok(app_data_dir) = app_handle.path().app_data_dir() {
        let details = serde_json::to_value(payload).ok();
        let _ = audit::append_audit_event(app_data_dir, "file_safety", "watch_event", details);
      }
    }
  })
  .map_err(|err| err.to_string())?;

  watcher
    .watch(canonical_target.as_path(), RecursiveMode::Recursive)
    .map_err(|err| err.to_string())?;

  let mut watchers = watch_registry.watchers.lock().map_err(|_| "watch registry is poisoned")?;
  watchers.insert(watched_path, watcher);

  Ok(())
}

#[tauri::command]
fn fs_watch_stop(
  watch_registry: tauri::State<'_, WatchRegistry>,
  path: String,
) -> Result<(), String> {
  let canonical = PathBuf::from(&path)
    .canonicalize()
    .map_err(|err| err.to_string())?;
  let watched_path = canonical.display().to_string();

  let mut watchers = watch_registry.watchers.lock().map_err(|_| "watch registry is poisoned")?;
  if let Some(mut watcher) = watchers.remove(&watched_path) {
    let _ = watcher.unwatch(canonical.as_path());
  }

  Ok(())
}

#[tauri::command]
fn fs_parse_artifact(
  app: tauri::AppHandle,
  state: tauri::State<'_, Arc<Database>>,
  path: String,
) -> Result<artifact_pipeline::ArtifactParseResponse, String> {
  let allowed_folders = state.list_allowed_folders().map_err(|err| err.to_string())?;
  let canonical_target = file_safety::ensure_path_allowed(PathBuf::from(&path).as_path(), &allowed_folders)?;

  let response = artifact_pipeline::parse_artifact(canonical_target.as_path())?;
  let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
  let details = serde_json::json!({
    "path": response.path,
    "format": response.format,
    "sizeBytes": response.size_bytes,
  });
  let _ = audit::append_audit_event(app_data_dir, "file_safety", "parse_artifact", Some(details));

  Ok(response)
}

#[tauri::command]
fn fs_extract_text(
  app: tauri::AppHandle,
  state: tauri::State<'_, Arc<Database>>,
  path: String,
  run_id: Option<String>,
) -> Result<String, String> {
  let policy = load_policy_state(&state)?;
  enforce_tool_policy(
    &policy,
    "read_file",
    path.as_str(),
    policy.flags.allow_file_read_extraction,
  )?;

  let canonical_target = ensure_run_file_access(&state, run_id.as_deref(), &path, false)?;

  let text = artifact_pipeline::extract_text_for_llm(canonical_target.as_path())?;
  let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
  let details = serde_json::json!({
    "path": canonical_target.display().to_string(),
    "chars": text.chars().count(),
  });
  let _ = audit::append_audit_event(app_data_dir, "file_safety", "extract_text", Some(details));

  Ok(text)
}

#[tauri::command]
fn fs_extract_text_limited(
  app: tauri::AppHandle,
  state: tauri::State<'_, Arc<Database>>,
  path: String,
  max_chars: usize,
  run_id: Option<String>,
) -> Result<ExtractTextLimitedResponse, String> {
  let bounded_max_chars = max_chars.clamp(1_000, 120_000);
  let policy = load_policy_state(&state)?;
  enforce_tool_policy(
    &policy,
    "read_file",
    path.as_str(),
    policy.flags.allow_file_read_extraction,
  )?;

  let canonical_target = ensure_run_file_access(&state, run_id.as_deref(), &path, false)?;

  let (text, truncated) = artifact_pipeline::extract_text_for_llm_limited(
    canonical_target.as_path(),
    bounded_max_chars,
  )?;
  let chars = text.chars().count();
  let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
  let details = serde_json::json!({
    "path": canonical_target.display().to_string(),
    "chars": chars,
    "maxChars": bounded_max_chars,
    "truncated": truncated,
  });
  let _ = audit::append_audit_event(app_data_dir, "file_safety", "extract_text_limited", Some(details));

  Ok(ExtractTextLimitedResponse {
    text,
    chars,
    truncated,
  })
}

#[tauri::command]
fn fs_save_artifact_version(
  app: tauri::AppHandle,
  state: tauri::State<'_, Arc<Database>>,
  path: String,
  run_id: Option<String>,
  label: Option<String>,
) -> Result<ArtifactVersionRow, String> {
  let allowed_folders = state.list_allowed_folders().map_err(|err| err.to_string())?;
  let canonical_target = file_safety::ensure_path_allowed(PathBuf::from(&path).as_path(), &allowed_folders)?;
  let parsed = artifact_pipeline::parse_artifact(canonical_target.as_path())?;

  let id = uuid::Uuid::new_v4().to_string();
  let created_at = chrono::Utc::now().to_rfc3339();
  let metadata_json = serde_json::to_string(&parsed.metadata).map_err(|err| err.to_string())?;

  state
    .insert_artifact_version(
      &id,
      run_id.as_deref(),
      label.as_deref(),
      &parsed.path,
      &parsed.format,
      parsed.size_bytes as i64,
      &parsed.summary,
      &parsed.preview,
      &metadata_json,
      &created_at,
    )
    .map_err(|err| err.to_string())?;

  let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
  let details = serde_json::json!({
    "artifactVersionId": id,
    "runId": run_id,
    "label": label,
    "sourcePath": parsed.path,
    "format": parsed.format,
    "sizeBytes": parsed.size_bytes,
  });
  let _ = audit::append_audit_event(app_data_dir, "file_safety", "save_artifact_version", Some(details));

  Ok(ArtifactVersionRow {
    id,
    run_id,
    label,
    source_path: parsed.path,
    format: parsed.format,
    size_bytes: parsed.size_bytes as i64,
    summary: parsed.summary,
    preview: parsed.preview,
    metadata: parsed.metadata,
    created_at,
  })
}

#[tauri::command]
fn fs_list_artifact_versions(
  state: tauri::State<'_, Arc<Database>>,
  limit: Option<u32>,
) -> Result<Vec<ArtifactVersionRow>, String> {
  let bounded_limit = limit.unwrap_or(30).clamp(1, 200) as i64;

  state
    .list_artifact_versions(bounded_limit)
    .map_err(|err| err.to_string())
    .map(|rows| {
      rows
        .into_iter()
        .map(
          |(id, run_id, label, source_path, format, size_bytes, summary, preview, metadata_json, created_at)| {
            let metadata: Value = serde_json::from_str(&metadata_json).unwrap_or_else(|_| serde_json::json!({}));
            ArtifactVersionRow {
              id,
              run_id,
              label,
              source_path,
              format,
              size_bytes,
              summary,
              preview,
              metadata,
              created_at,
            }
          },
        )
        .collect()
    })
}

#[tauri::command]
fn fs_export_artifact_version(
  app: tauri::AppHandle,
  state: tauri::State<'_, Arc<Database>>,
  artifact_version_id: String,
  target_dir: String,
  export_format: String,
) -> Result<ArtifactExportRow, String> {
  let allowed_folders = state.list_allowed_folders().map_err(|err| err.to_string())?;
  let canonical_dir = file_safety::ensure_path_allowed(PathBuf::from(&target_dir).as_path(), &allowed_folders)?;
  fs::create_dir_all(&canonical_dir).map_err(|err| err.to_string())?;

  let version = state
    .get_artifact_version_by_id(&artifact_version_id)
    .map_err(|err| err.to_string())?
    .ok_or_else(|| "artifact version not found".to_string())?;

  let (
    version_id,
    run_id,
    label,
    source_path,
    source_format,
    size_bytes,
    summary,
    preview,
    metadata_json,
    _created_at,
  ) = version;

  let format = export_format.trim().to_lowercase();
  let extension = match format.as_str() {
    "json" => "json",
    "md" | "markdown" => "md",
    "txt" | "text" => "txt",
    "pdf" => "pdf",
    "docx" => "docx",
    "xlsx" => "xlsx",
    "pptx" => "pptx",
    _ => return Err("unsupported export format (allowed: json, md, txt, pdf, docx, xlsx, pptx)".to_string()),
  };

  let source_stem = PathBuf::from(&source_path)
    .file_stem()
    .and_then(|value| value.to_str())
    .unwrap_or("artifact")
    .chars()
    .map(|ch| if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' { ch } else { '_' })
    .collect::<String>();

  let short_id: String = version_id.chars().take(8).collect();
  let file_name = format!("{}_{}_export.{}", source_stem, short_id, extension);
  let target_path = canonical_dir.join(file_name);

  let metadata = serde_json::from_str::<Value>(&metadata_json).unwrap_or_else(|_| serde_json::json!({}));

  let written_size = if matches!(format.as_str(), "json" | "md" | "markdown" | "txt" | "text") {
    let content = match format.as_str() {
      "json" => serde_json::to_string_pretty(&serde_json::json!({
        "artifactVersionId": version_id,
        "runId": run_id,
        "label": label,
        "sourcePath": source_path,
        "sourceFormat": source_format,
        "sourceSizeBytes": size_bytes,
        "summary": summary,
        "preview": preview,
        "metadata": metadata,
      }))
      .map_err(|err| err.to_string())?,
      "md" | "markdown" => format!(
        "# Artefakt-Export\n\n- Artefakt-Version: {}\n- Run-ID: {}\n- Label: {}\n- Quelle: {}\n- Format: {}\n- Groesse: {} Bytes\n\n## Summary\n\n{}\n\n## Preview\n\n```\n{}\n```\n",
        version_id,
        run_id.clone().unwrap_or_else(|| "-".to_string()),
        label.clone().unwrap_or_else(|| "-".to_string()),
        source_path,
        source_format,
        size_bytes,
        summary,
        preview,
      ),
      _ => format!(
        "Artefakt-Version: {}\nRun-ID: {}\nLabel: {}\nQuelle: {}\nFormat: {}\nGroesse: {} Bytes\n\nSummary:\n{}\n\nPreview:\n{}\n",
        version_id,
        run_id.clone().unwrap_or_else(|| "-".to_string()),
        label.clone().unwrap_or_else(|| "-".to_string()),
        source_path,
        source_format,
        size_bytes,
        summary,
        preview,
      ),
    };

    fs::write(&target_path, &content).map_err(|err| err.to_string())?;
    content.len() as i64
  } else {
    let native_input = cowork_features::ArtifactVersionExportInput {
      artifact_version_id: version_id.clone(),
      run_id: run_id.clone(),
      label: label.clone(),
      source_path: source_path.clone(),
      source_format: source_format.clone(),
      source_size_bytes: size_bytes,
      summary: summary.clone(),
      preview: preview.clone(),
      metadata,
    };
    cowork_features::export_artifact_version_native(target_path.as_path(), &format, &native_input)?;
    fs::metadata(&target_path).map_err(|err| err.to_string())?.len() as i64
  };
  let created_at = chrono::Utc::now().to_rfc3339();
  let export_id = uuid::Uuid::new_v4().to_string();

  state
    .insert_artifact_export(
      &export_id,
      &version_id,
      &format,
      &target_path.display().to_string(),
      written_size,
      &created_at,
    )
    .map_err(|err| err.to_string())?;

  let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
  let details = serde_json::json!({
    "exportId": export_id,
    "artifactVersionId": version_id,
    "format": format,
    "targetPath": target_path.display().to_string(),
    "sizeBytes": written_size,
  });
  let _ = audit::append_audit_event(app_data_dir, "artifact_pipeline", "export_artifact_version", Some(details));

  Ok(ArtifactExportRow {
    id: export_id,
    artifact_version_id: version_id,
    export_format: format,
    target_path: target_path.display().to_string(),
    size_bytes: written_size,
    created_at,
    source_path,
    run_id,
    label,
    source_format,
  })
}

#[tauri::command]
fn fs_list_artifact_exports(
  state: tauri::State<'_, Arc<Database>>,
  limit: Option<u32>,
) -> Result<Vec<ArtifactExportRow>, String> {
  let bounded_limit = limit.unwrap_or(30).clamp(1, 200) as i64;
  state
    .list_artifact_exports(bounded_limit)
    .map_err(|err| err.to_string())
    .map(|rows| {
      rows
        .into_iter()
        .map(
          |(id, artifact_version_id, export_format, target_path, size_bytes, created_at, source_path, run_id, label, source_format)| {
            ArtifactExportRow {
              id,
              artifact_version_id,
              export_format,
              target_path,
              size_bytes,
              created_at,
              source_path,
              run_id,
              label,
              source_format,
            }
          },
        )
        .collect()
    })
}

#[tauri::command]
async fn task_run_sub_agents(
  app: tauri::AppHandle,
  state: tauri::State<'_, Arc<Database>>,
  request: cowork_features::SubAgentRequest,
) -> Result<cowork_features::SubAgentRunResponse, String> {
  let allowed_folders = state.list_allowed_folders().map_err(|err| err.to_string())?;
  let mut canonical_paths = Vec::new();

  for path in &request.paths {
    let canonical = file_safety::ensure_path_allowed(PathBuf::from(path).as_path(), &allowed_folders)?;
    canonical_paths.push(canonical);
  }

  let response = cowork_features::run_sub_agents(request, canonical_paths).await;
  let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
  let details = serde_json::json!({
    "totalItems": response.total_items,
    "successfulItems": response.successful_items,
    "failedItems": response.failed_items,
    "parallelism": response.parallelism,
    "durationMs": response.duration_ms,
  });
  let _ = audit::append_audit_event(app_data_dir, "task_engine", "run_sub_agents", Some(details));

  Ok(response)
}

#[tauri::command]
fn fs_generate_pro_outputs(
  app: tauri::AppHandle,
  state: tauri::State<'_, Arc<Database>>,
  request: cowork_features::ProOutputRequest,
) -> Result<cowork_features::ProOutputResponse, String> {
  let allowed_folders = state.list_allowed_folders().map_err(|err| err.to_string())?;
  let csv_path = file_safety::ensure_path_allowed(PathBuf::from(&request.csv_path).as_path(), &allowed_folders)?;
  let output_dir = file_safety::ensure_path_allowed(PathBuf::from(&request.output_dir).as_path(), &allowed_folders)?;

  let response = cowork_features::generate_pro_outputs(request, csv_path.as_path(), output_dir.as_path())?;
  let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
  let details = serde_json::json!({
    "csvPath": response.csv_path,
    "outputDir": response.output_dir,
    "generatedFiles": response.generated_files,
    "rows": response.rows,
    "columns": response.columns,
    "numericColumns": response.numeric_columns,
  });
  let _ = audit::append_audit_event(app_data_dir, "artifact_pipeline", "generate_pro_outputs", Some(details));

  Ok(response)
}

#[tauri::command]
fn fs_generate_office_workflow(
  app: tauri::AppHandle,
  state: tauri::State<'_, Arc<Database>>,
  request: cowork_features::OfficeWorkflowRequest,
  run_id: Option<String>,
) -> Result<cowork_features::OfficeWorkflowResponse, String> {
  let mut normalized_request = request;

  let output_path = ensure_run_file_access(
    &state,
    run_id.as_deref(),
    &normalized_request.output_path,
    true,
  )?;
  normalized_request.output_path = output_path.display().to_string();

  if let Some(template_path) = normalized_request.template_path.clone() {
    let canonical_template = ensure_run_file_access(
      &state,
      run_id.as_deref(),
      template_path.as_str(),
      false,
    )?;
    normalized_request.template_path = Some(canonical_template.display().to_string());
  }

  let response = cowork_features::generate_office_workflow(normalized_request)?;
  let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
  let details = serde_json::json!({
    "format": response.format,
    "mode": response.mode,
    "generatedArtifacts": response.generated.len(),
    "placeholdersApplied": response.placeholders_applied,
  });
  let _ = audit::append_audit_event(app_data_dir, "artifact_pipeline", "generate_office_workflow", Some(details));

  Ok(response)
}

fn map_scheduled_task_row(
  row: (String, String, String, String, String, Option<String>, Option<String>, Option<String>, i64, String, bool, Option<String>, Option<String>, String, String),
) -> ScheduledTaskRow {
  let (id, name, prompt, schedule_expr, task_kind, crew_id, crew_snapshot_json, model_config_json, priority, depends_on_task_ids_json, active, last_run_at, next_run_at, created_at, updated_at) = row;
  ScheduledTaskRow {
    id,
    name,
    prompt,
    schedule_expr,
    task_kind,
    crew_id,
    crew_snapshot_json,
    model_config_json,
    priority,
    depends_on_task_ids: serde_json::from_str::<Vec<String>>(&depends_on_task_ids_json).unwrap_or_default(),
    active,
    last_run_at,
    next_run_at,
    created_at,
    updated_at,
  }
}

fn run_scheduled_task_once(
  app: &tauri::AppHandle,
  database: &Arc<Database>,
  task_id: &str,
  task_name: &str,
  task_prompt: &str,
  schedule_expr: &str,
  task_kind: &str,
  crew_id: Option<&str>,
  crew_snapshot_json: Option<&str>,
  model_config_json: Option<&str>,
) {
  let started_at = chrono::Utc::now().to_rfc3339();
  let run_id = uuid::Uuid::new_v4().to_string();
  let execution_result: Result<(String, Option<String>, Option<String>), String> = if task_kind.eq_ignore_ascii_case("crew") {
    let snapshot = crew_snapshot_json
      .filter(|value| !value.trim().is_empty())
      .ok_or_else(|| "crewSnapshotJson fehlt fuer geplanten Crew-Job".to_string());

    match snapshot {
      Ok(snapshot_json) => match serde_json::from_str::<CrewExecuteRequest>(snapshot_json) {
        Ok(request) => {
          let registry = app.state::<CrewExecutionRegistry>();
          let bridge = app.state::<CrewPythonBridge>();
          match tauri::async_runtime::block_on(execute_crew_request(&app, database, &registry, bridge.inner(), request)) {
            Ok(response) => {
              let status = if response.status == "completed" { "succeeded".to_string() } else { response.status.clone() };
              let result_json = serde_json::to_string(&response).unwrap_or_else(|_| String::new());
              Ok((status, Some(result_json), response.error))
            }
            Err(error) => Err(error),
          }
        }
        Err(error) => Err(format!("Crew-Snapshot ungueltig: {}", error)),
      },
      Err(error) => Err(error),
    }
  } else {
    let runtime_config = match model_config_json.filter(|value| !value.trim().is_empty()) {
      Some(raw_json) => serde_json::from_str::<ScheduledPromptRuntimeConfig>(raw_json)
        .map(Some)
        .map_err(|error| format!("modelConfigJson ungueltig: {}", error)),
      None => Ok(None),
    };

    match runtime_config {
      Ok(runtime_config) => {
        let config = runtime_config.as_ref().map(|entry| entry.config.clone());
        let history = runtime_config
          .as_ref()
          .and_then(|entry| entry.cwd.as_deref())
          .map(str::trim)
          .filter(|value| !value.is_empty())
          .map(|value| vec![ChatMessage {
            role: "system".to_string(),
            content: format!("Arbeitsverzeichnis: {}", value),
          }])
          .unwrap_or_default();

        tauri::async_runtime::block_on(chat_turn_internal(
          config,
          task_prompt.to_string(),
          history,
          vec![],
        ))
        .map(|response| {
          ("succeeded".to_string(), Some(response.assistant_message), None)
        })
        .map_err(|error| error.to_string())
      }
      Err(error) => Err(error),
    }
  };

  let finished_at = chrono::Utc::now().to_rfc3339();

  let next_run_at = scheduler::next_run_from_expression(schedule_expr, chrono::Utc::now())
    .ok()
    .map(|next| next.to_rfc3339());

  match execution_result {
    Ok((status, result_json, error_json)) => {
      let _ = database.insert_scheduled_run(
        &run_id,
        task_id,
        &status,
        &started_at,
        Some(&finished_at),
        result_json.as_deref(),
        error_json.as_deref(),
      );
      let _ = database.update_scheduled_task_runtime(task_id, Some(&finished_at), next_run_at.as_deref());

      if let Ok(app_data_dir) = app.path().app_data_dir() {
        let details = serde_json::json!({
          "taskId": task_id,
          "taskName": task_name,
          "runId": run_id,
          "taskKind": task_kind,
          "crewId": crew_id,
          "status": status,
        });
        let _ = audit::append_audit_event(app_data_dir, "scheduler", "task_run_completed", Some(details));
      }
    }
    Err(err) => {
      let error_text = err.to_string();
      let _ = database.insert_scheduled_run(
        &run_id,
        task_id,
        "failed",
        &started_at,
        Some(&finished_at),
        None,
        Some(&error_text),
      );
      let _ = database.update_scheduled_task_runtime(task_id, Some(&finished_at), next_run_at.as_deref());

      if let Ok(app_data_dir) = app.path().app_data_dir() {
        let details = serde_json::json!({
          "taskId": task_id,
          "taskName": task_name,
          "runId": run_id,
          "taskKind": task_kind,
          "crewId": crew_id,
          "status": "failed",
          "error": error_text,
        });
        let _ = audit::append_audit_event(app_data_dir, "scheduler", "task_run_completed", Some(details));
      }
    }
  }
}

fn scheduled_task_dependencies_ready(
  database: &Arc<Database>,
  depends_on_task_ids_json: &str,
  task_last_run_at: Option<&str>,
) -> bool {
  let dependency_ids = serde_json::from_str::<Vec<String>>(depends_on_task_ids_json).unwrap_or_default();
  if dependency_ids.is_empty() {
    return true;
  }

  dependency_ids.into_iter().all(|dependency_id| {
    match database.latest_scheduled_run_status(&dependency_id) {
      Ok(Some((status, finished_at))) if status == "succeeded" => {
        if let Some(current_last_run) = task_last_run_at {
          finished_at.as_deref().map(|value| value > current_last_run).unwrap_or(false)
        } else {
          true
        }
      }
      _ => false,
    }
  })
}

fn start_scheduler_worker(app: tauri::AppHandle, database: Arc<Database>) {
  std::thread::spawn(move || loop {
    let now = chrono::Utc::now().to_rfc3339();
    let due_tasks: Vec<(String, String, String, String, Option<String>, String, Option<String>, Option<String>, Option<String>, i64, String, Option<String>)> =
      database.list_due_scheduled_tasks(&now).unwrap_or_default();
    for (task_id, task_name, task_prompt, schedule_expr, _, task_kind, crew_id, crew_snapshot_json, model_config_json, _, depends_on_task_ids_json, task_last_run_at) in due_tasks {
      if !scheduled_task_dependencies_ready(&database, &depends_on_task_ids_json, task_last_run_at.as_deref()) {
        continue;
      }
      let crew_id_ref: Option<&str> = crew_id.as_deref();
      let crew_snapshot_json_ref: Option<&str> = crew_snapshot_json.as_deref();
      let model_config_json_ref: Option<&str> = model_config_json.as_deref();
      run_scheduled_task_once(
        &app,
        &database,
        &task_id,
        &task_name,
        &task_prompt,
        &schedule_expr,
        &task_kind,
        crew_id_ref,
        crew_snapshot_json_ref,
        model_config_json_ref,
      );
    }

    std::thread::sleep(Duration::from_secs(30));
  });
}

#[tauri::command]
fn scheduler_upsert_task(
  state: tauri::State<'_, Arc<Database>>,
  request: ScheduledTaskUpsertRequest,
) -> Result<ScheduledTaskRow, String> {
  let database = state.inner().clone();
  let now = chrono::Utc::now();
  let now_text = now.to_rfc3339();
  let existing_task = database
    .list_scheduled_tasks()
    .map_err(|err: rusqlite::Error| err.to_string())?
    .into_iter()
    .find(|row| row.0 == request.id);

  let next_run_at = if request.active {
    Some(
      scheduler::next_run_from_expression(&request.schedule_expr, now)
        .map_err(|err| err.to_string())?
        .to_rfc3339(),
    )
  } else {
    None
  };

  let last_run_at = existing_task.and_then(|row| row.11);
  let task_kind = request.task_kind.unwrap_or_else(|| "prompt".to_string());
  let priority = request.priority.unwrap_or(100).clamp(1, 1000);
  let depends_on_task_ids_json = serde_json::to_string(&request.depends_on_task_ids.unwrap_or_default())
    .map_err(|err| err.to_string())?;

  database
    .upsert_scheduled_task(
      &request.id,
      &request.name,
      &request.prompt,
      &request.schedule_expr,
      &task_kind,
      request.crew_id.as_deref(),
      request.crew_snapshot_json.as_deref(),
      request.model_config_json.as_deref(),
      priority,
      &depends_on_task_ids_json,
      request.active,
      last_run_at.as_deref(),
      next_run_at.as_deref(),
      &now_text,
    )
    .map_err(|err: rusqlite::Error| err.to_string())?;

  database
    .list_scheduled_tasks()
    .map_err(|err: rusqlite::Error| err.to_string())?
    .into_iter()
    .find(|row| row.0 == request.id)
    .map(map_scheduled_task_row)
    .ok_or_else(|| "scheduled task not found after upsert".to_string())
}

#[tauri::command]
fn scheduler_list_tasks(state: tauri::State<'_, Arc<Database>>) -> Result<Vec<ScheduledTaskRow>, String> {
  let database = state.inner().clone();
  database
    .list_scheduled_tasks()
    .map_err(|err: rusqlite::Error| err.to_string())
    .map(|rows| rows.into_iter().map(map_scheduled_task_row).collect::<Vec<_>>())
}

#[tauri::command]
fn scheduler_delete_task(state: tauri::State<'_, Arc<Database>>, id: String) -> Result<(), String> {
  let database = state.inner().clone();
  database.delete_scheduled_task(&id).map_err(|err: rusqlite::Error| err.to_string())
}

#[tauri::command]
fn scheduler_set_task_active(
  state: tauri::State<'_, Arc<Database>>,
  request: ScheduledTaskToggleRequest,
) -> Result<(), String> {
  let database = state.inner().clone();
  let task_row = database
    .list_scheduled_tasks()
    .map_err(|err: rusqlite::Error| err.to_string())?
    .into_iter()
    .find(|row| row.0 == request.id)
    .ok_or_else(|| "scheduled task not found".to_string())?;

  let next_run_at = if request.active {
    Some(
      scheduler::next_run_from_expression(&task_row.3, chrono::Utc::now())
        .map_err(|err| err.to_string())?
        .to_rfc3339(),
    )
  } else {
    None
  };

  database
    .set_scheduled_task_active(&request.id, request.active, next_run_at.as_deref())
    .map_err(|err: rusqlite::Error| err.to_string())
}

#[tauri::command]
fn scheduler_run_task_now(
  app: tauri::AppHandle,
  state: tauri::State<'_, Arc<Database>>,
  id: String,
) -> Result<(), String> {
  let database = state.inner().clone();
  let task_row = database
    .list_scheduled_tasks()
    .map_err(|err: rusqlite::Error| err.to_string())?
    .into_iter()
    .find(|row| row.0 == id)
    .ok_or_else(|| "scheduled task not found".to_string())?;
  run_scheduled_task_once(
    &app,
    &database,
    &task_row.0,
    &task_row.1,
    &task_row.2,
    &task_row.3,
    &task_row.4,
    task_row.5.as_deref(),
    task_row.6.as_deref(),
    task_row.7.as_deref(),
  );
  Ok(())
}

#[tauri::command]
fn scheduler_list_runs(
  state: tauri::State<'_, Arc<Database>>,
  limit: Option<u32>,
) -> Result<Vec<ScheduledRunRow>, String> {
  let bounded_limit = limit.unwrap_or(30).clamp(1, 200) as i64;
  let database = state.inner().clone();
  database
    .list_scheduled_runs(bounded_limit)
    .map_err(|err: rusqlite::Error| err.to_string())
    .map(|rows| {
      rows
        .into_iter()
        .map(|(id, task_id, status, started_at, finished_at, result, error)| ScheduledRunRow {
          id,
          task_id,
          status,
          started_at,
          finished_at,
          result,
          error,
        })
        .collect::<Vec<_>>()
    })
}

#[tauri::command]
fn export_save_text_file(
  app: tauri::AppHandle,
  path: String,
  content: String,
) -> Result<(), String> {
  let target_path = PathBuf::from(&path);
  if let Some(parent) = target_path.parent() {
    fs::create_dir_all(parent).map_err(|err| err.to_string())?;
  }
  fs::write(&target_path, content.as_bytes()).map_err(|err| err.to_string())?;

  if let Ok(app_data_dir) = app.path().app_data_dir() {
    let details = serde_json::json!({
      "path": path,
      "bytes": content.len(),
    });
    let _ = audit::append_audit_event(app_data_dir, "export", "save_text_file", Some(details));
  }

  Ok(())
}

async fn probe_connector_method(
  client: &reqwest::Client,
  url: &str,
  method: Method,
  api_key: Option<&str>,
) -> Result<StatusCode, String> {
  let mut request = client
    .request(method, url)
    .header("User-Agent", "Open-Cowork/1.0");

  if let Some(key) = api_key.filter(|value| !value.trim().is_empty()) {
    request = request.header("Authorization", format!("Bearer {}", key.trim()));
  }

  request
    .send()
    .await
    .map(|response| response.status())
    .map_err(|error| error.to_string())
}

fn interpret_connector_status(status: StatusCode) -> (bool, String) {
  if status.is_success() {
    return (true, format!("Endpoint antwortet erfolgreich ({})", status));
  }

  match status.as_u16() {
    401 | 403 => (true, format!("Endpoint erreichbar, aber Authentifizierung erforderlich ({})", status)),
    405 => (true, format!("Endpoint erreichbar, verlangt aber eine andere HTTP-Methode ({})", status)),
    404 => (false, format!("Endpoint nicht gefunden ({})", status)),
    _ if status.is_server_error() => (false, format!("Endpoint antwortet mit Serverfehler ({})", status)),
    _ => (true, format!("Endpoint erreichbar, antwortet mit Status {}", status)),
  }
}

fn build_provider_probe_url(provider_kind: &str, base_url: &str) -> Result<String, String> {
  let trimmed = base_url.trim().trim_end_matches('/');
  if trimmed.is_empty() {
    return Err("baseUrl ist erforderlich".to_string());
  }

  let url = match provider_kind {
    "openai-compatible" | "openrouter" => {
      if trimmed.ends_with("/chat/completions") {
        format!("{}{}", trimmed.trim_end_matches("/chat/completions"), "/models")
      } else if trimmed.ends_with("/models") {
        trimmed.to_string()
      } else {
        format!("{}/models", trimmed)
      }
    }
    _ => trimmed.to_string(),
  };

  Url::parse(&url)
    .map(|parsed| parsed.to_string())
    .map_err(|error| format!("ungueltige URL: {}", error))
}

#[tauri::command]
async fn crew_provider_health_check(
  request: CrewProviderHealthCheckRequest,
) -> Result<CrewProviderHealthCheckResponse, String> {
  let checked_at = chrono::Utc::now().to_rfc3339();

  if request.provider_kind.eq_ignore_ascii_case("ollama") {
    let health = check_health(Some(OllamaConfig {
      base_url: request.base_url.trim().to_string(),
      model: String::new(),
      timeout_ms: 12_000,
    }))
    .await
    .map_err(map_ollama_error)?;

    return Ok(CrewProviderHealthCheckResponse {
      reachable: health.ok,
      status: None,
      endpoint: health.endpoint,
      message: health.error.unwrap_or_else(|| format!("Ollama erreichbar, {} Modell(e) gefunden", health.models.len())),
      checked_at,
    });
  }

  let probe_url = build_provider_probe_url(&request.provider_kind, &request.base_url)?;
  let client = reqwest::Client::builder()
    .timeout(Duration::from_secs(12))
    .build()
    .map_err(|error| error.to_string())?;

  let api_key = request.api_key.as_deref();
  let status = if request.provider_kind.eq_ignore_ascii_case("openrouter") {
    let mut request_builder = client
      .get(&probe_url)
      .header("User-Agent", "Open-Cowork/1.0")
      .header("HTTP-Referer", "https://open-cowork.local")
      .header("X-Title", "Open Cowork");
    if let Some(key) = api_key.filter(|value| !value.trim().is_empty()) {
      request_builder = request_builder.header("Authorization", format!("Bearer {}", key.trim()));
    }
    request_builder
      .send()
      .await
      .map(|response| response.status())
      .map_err(|error| error.to_string())?
  } else {
    match probe_connector_method(&client, &probe_url, Method::HEAD, api_key).await {
      Ok(status) => status,
      Err(_) => probe_connector_method(&client, &probe_url, Method::GET, api_key).await?,
    }
  };

  let (reachable, message) = interpret_connector_status(status);

  Ok(CrewProviderHealthCheckResponse {
    reachable,
    status: Some(status.as_u16()),
    endpoint: probe_url,
    message,
    checked_at,
  })
}

#[tauri::command]
async fn crew_provider_models_list(
  request: CrewProviderModelsRequest,
) -> Result<CrewProviderModelsResponse, String> {
  let endpoint = build_provider_probe_url(&request.provider_kind, &request.base_url)?;
  let client = reqwest::Client::builder()
    .timeout(Duration::from_secs(20))
    .build()
    .map_err(|error| error.to_string())?;

  let mut call = client.get(&endpoint).header("User-Agent", "Open-Cowork/1.0");
  if let Some(api_key) = request.api_key.filter(|value| !value.trim().is_empty()) {
    call = call.header("Authorization", format!("Bearer {}", api_key.trim()));
  }
  if request.provider_kind.eq_ignore_ascii_case("openrouter") {
    call = call
      .header("HTTP-Referer", "https://open-cowork.local")
      .header("X-Title", "Open Cowork");
  }

  let response = call.send().await.map_err(|error| error.to_string())?;
  let status = response.status();
  if !status.is_success() {
    let body = response.text().await.unwrap_or_default();
    let excerpt: String = body.chars().take(400).collect();
    return Err(format!("Provider antwortete mit {}: {}", status, excerpt));
  }

  let payload: OpenAiModelsResponse = response.json().await.map_err(|error| error.to_string())?;
  let mut models = payload
    .data
    .into_iter()
    .filter_map(|entry| {
      entry
        .id
        .or(entry.name)
        .map(|value| value.trim().to_string())
    })
    .filter(|entry| !entry.trim().is_empty())
    .collect::<Vec<_>>();
  models.sort();
  models.dedup();

  Ok(CrewProviderModelsResponse {
    endpoint,
    models,
  })
}

#[tauri::command]
async fn connector_test_reachability(
  app: tauri::AppHandle,
  request: ConnectorReachabilityRequest,
) -> Result<ConnectorReachabilityResponse, String> {
  let url = request
    .webhook_url
    .clone()
    .filter(|value| !value.trim().is_empty())
    .ok_or_else(|| "webhookUrl ist erforderlich".to_string())?;

  let parsed_url = Url::parse(url.trim()).map_err(|error| format!("ungueltige URL: {}", error))?;
  let client = reqwest::Client::builder()
    .timeout(Duration::from_secs(12))
    .build()
    .map_err(|error| error.to_string())?;

  let api_key = request.api_key.as_deref();
  let status = match probe_connector_method(&client, parsed_url.as_str(), Method::HEAD, api_key).await {
    Ok(status) => status,
    Err(_) => probe_connector_method(&client, parsed_url.as_str(), Method::GET, api_key).await?,
  };

  let (reachable, message) = interpret_connector_status(status);
  let checked_at = chrono::Utc::now().to_rfc3339();

  if let Ok(app_data_dir) = app.path().app_data_dir() {
    let details = serde_json::json!({
      "key": request.key,
      "label": request.label,
      "url": parsed_url.as_str(),
      "status": status.as_u16(),
      "reachable": reachable,
    });
    let _ = audit::append_audit_event(app_data_dir, "connector", "reachability_test", Some(details));
  }

  Ok(ConnectorReachabilityResponse {
    reachable,
    status: Some(status.as_u16()),
    message,
    checked_at,
  })
}

#[tauri::command]
fn policy_get(state: tauri::State<'_, Arc<Database>>) -> Result<PolicyStatePayload, String> {
  load_policy_state(&state)
}

#[tauri::command]
fn policy_set(
  state: tauri::State<'_, Arc<Database>>,
  request: PolicySetRequest,
) -> Result<PolicyStatePayload, String> {
  state
    .set_policy_flag(POLICY_FLAG_STRICT, request.flags.strict_policy_enforcement)
    .map_err(|err| err.to_string())?;
  state
    .set_policy_flag(POLICY_FLAG_TOOL_DISPATCHER, request.flags.allow_tool_dispatcher)
    .map_err(|err| err.to_string())?;
  state
    .set_policy_flag(POLICY_FLAG_MCP, request.flags.allow_mcp_tool_calls)
    .map_err(|err| err.to_string())?;
  state
    .set_policy_flag(POLICY_FLAG_WEB_FETCH, request.flags.allow_web_fetch)
    .map_err(|err| err.to_string())?;
  state
    .set_policy_flag(POLICY_FLAG_FILE_READ, request.flags.allow_file_read_extraction)
    .map_err(|err| err.to_string())?;
  state
    .set_policy_flag(POLICY_FLAG_AUTO_COMPACT, request.flags.auto_compact_long_context)
    .map_err(|err| err.to_string())?;
  state
    .set_policy_flag(POLICY_FLAG_SHELL_EXECUTION, request.flags.allow_shell_execution)
    .map_err(|err| err.to_string())?;
  state
    .set_policy_flag(POLICY_FLAG_WEB_SEARCH, request.flags.allow_web_search)
    .map_err(|err| err.to_string())?;

  state
    .replace_policy_deny_rules(&request.deny_rules)
    .map_err(|err| err.to_string())?;

  state
    .replace_policy_tool_states(&build_policy_tool_states(&normalize_policy_enabled_tool_ids(&request.enabled_tool_ids)))
    .map_err(|err| err.to_string())?;

  load_policy_state(&state)
}

#[tauri::command]
fn policy_evaluate(
  state: tauri::State<'_, Arc<Database>>,
  request: PolicyEvaluateRequest,
) -> Result<PolicyEvaluateResponse, String> {
  let policy = load_policy_state(&state)?;
  let flag_allowed = match request.requested_flag.as_deref() {
    Some(POLICY_FLAG_TOOL_DISPATCHER) => policy.flags.allow_tool_dispatcher,
    Some(POLICY_FLAG_MCP) => policy.flags.allow_mcp_tool_calls,
    Some(POLICY_FLAG_WEB_FETCH) => policy.flags.allow_web_fetch,
    Some(POLICY_FLAG_FILE_READ) => policy.flags.allow_file_read_extraction,
    Some(POLICY_FLAG_AUTO_COMPACT) => policy.flags.auto_compact_long_context,
    Some(POLICY_FLAG_SHELL_EXECUTION) => policy.flags.allow_shell_execution,
    Some(POLICY_FLAG_WEB_SEARCH) => policy.flags.allow_web_search,
    _ => true,
  };

  match enforce_tool_policy(&policy, &request.tool, &request.target, flag_allowed) {
    Ok(_) => Ok(PolicyEvaluateResponse {
      allowed: true,
      reason: "allowed".to_string(),
    }),
    Err(err) => Ok(PolicyEvaluateResponse {
      allowed: false,
      reason: err,
    }),
  }
}

#[tauri::command]
fn engine_run_create(
  state: tauri::State<'_, Arc<Database>>,
  request: EngineRunCreateRequest,
) -> Result<(), String> {
  state
    .insert_engine_run(
      &request.id,
      request.parent_run_id.as_deref(),
      request.thread_id.as_deref(),
      request.session_id.as_deref(),
      &request.title,
      request.input_summary.as_deref(),
      request.status.as_deref().unwrap_or("pending"),
      request.phase.as_deref().unwrap_or("queued"),
      request.cwd.as_deref(),
      request.model.as_deref(),
      request.provider.as_deref(),
      request.retry_count.unwrap_or(0),
      request.resumed_from_run_id.as_deref(),
      request.checkpoint_json.as_deref(),
      request.metadata_json.as_deref(),
    )
    .map_err(|err| err.to_string())
}

#[tauri::command]
fn engine_run_update(
  state: tauri::State<'_, Arc<Database>>,
  request: EngineRunUpdateRequest,
) -> Result<(), String> {
  state
    .update_engine_run(
      &request.id,
      request.status.as_deref(),
      request.phase.as_deref(),
      request.checkpoint_json.as_deref(),
      request.result_summary.as_deref(),
      request.error.as_deref(),
      request.metadata_json.as_deref(),
    )
    .map_err(|err| err.to_string())
}

#[tauri::command]
fn engine_run_get(
  state: tauri::State<'_, Arc<Database>>,
  id: String,
) -> Result<Option<db::EngineRunRow>, String> {
  state.get_engine_run(&id).map_err(|err| err.to_string())
}

#[tauri::command]
fn engine_run_list(
  state: tauri::State<'_, Arc<Database>>,
  limit: Option<i64>,
  status: Option<String>,
) -> Result<Vec<db::EngineRunRow>, String> {
  state
    .list_engine_runs(limit.unwrap_or(100).clamp(1, 500), status.as_deref())
    .map_err(|err| err.to_string())
}

#[tauri::command]
fn engine_run_cancel(
  state: tauri::State<'_, Arc<Database>>,
  id: String,
) -> Result<(), String> {
  if let Some(sandbox) = state
    .get_worker_sandbox_by_run(&id)
    .map_err(|err| err.to_string())?
  {
    let _ = state.update_worker_sandbox(&sandbox.id, Some("canceled"), None);
  }
  state
    .update_engine_run(&id, Some("canceled"), Some("canceled"), None, None, None, None)
    .map_err(|err| err.to_string())
}

#[tauri::command]
fn engine_run_resume(
  state: tauri::State<'_, Arc<Database>>,
  id: String,
) -> Result<(), String> {
  let existing = state
    .get_engine_run(&id)
    .map_err(|err| err.to_string())?
    .ok_or_else(|| "run not found".to_string())?;

  if existing.checkpoint_json.is_none() {
    return Err("run hat keinen checkpoint".to_string());
  }

  state
    .update_engine_run(
      &id,
      Some("running"),
      Some("resumed"),
      existing.checkpoint_json.as_deref(),
      None,
      None,
      None,
    )
    .map_err(|err| err.to_string())
}

#[tauri::command]
fn engine_run_retry(
  state: tauri::State<'_, Arc<Database>>,
  id: String,
) -> Result<String, String> {
  let existing = state
    .get_engine_run(&id)
    .map_err(|err| err.to_string())?
    .ok_or_else(|| "run not found".to_string())?;
  let new_id = uuid::Uuid::new_v4().to_string();

  state
    .insert_engine_run(
      &new_id,
      existing.parent_run_id.as_deref(),
      existing.thread_id.as_deref(),
      existing.session_id.as_deref(),
      &existing.title,
      existing.input_summary.as_deref(),
      "pending",
      "retry_queued",
      existing.cwd.as_deref(),
      existing.model.as_deref(),
      existing.provider.as_deref(),
      existing.retry_count + 1,
      Some(&id),
      existing.checkpoint_json.as_deref(),
      existing.metadata_json.as_deref(),
    )
    .map_err(|err| err.to_string())?;

  Ok(new_id)
}

#[tauri::command]
fn engine_run_checkpoint_add(
  state: tauri::State<'_, Arc<Database>>,
  request: EngineRunCheckpointRequest,
) -> Result<(), String> {
  state
    .insert_engine_run_checkpoint(
      &uuid::Uuid::new_v4().to_string(),
      &request.run_id,
      &request.label,
      &request.snapshot_json,
    )
    .map_err(|err| err.to_string())
}

#[tauri::command]
fn engine_run_checkpoint_list(
  state: tauri::State<'_, Arc<Database>>,
  run_id: String,
  limit: Option<i64>,
) -> Result<Vec<db::EngineRunCheckpointRow>, String> {
  state
    .list_engine_run_checkpoints(&run_id, limit.unwrap_or(20).clamp(1, 200))
    .map_err(|err| err.to_string())
}

#[tauri::command]
fn runtime_instruction_upsert(
  state: tauri::State<'_, Arc<Database>>,
  request: RuntimeInstructionUpsertRequest,
) -> Result<(), String> {
  state
    .upsert_runtime_instruction(
      &request.id,
      &request.scope_type,
      request.scope_ref.as_deref(),
      &request.title,
      &request.content,
      request.enabled.unwrap_or(true),
      request.priority.unwrap_or(100),
    )
    .map_err(|err| err.to_string())
}

#[tauri::command]
fn runtime_instruction_delete(
  state: tauri::State<'_, Arc<Database>>,
  id: String,
) -> Result<(), String> {
  state.delete_runtime_instruction(&id).map_err(|err| err.to_string())
}

#[tauri::command]
fn runtime_instruction_list(
  state: tauri::State<'_, Arc<Database>>,
  scope_type: Option<String>,
  enabled_only: Option<bool>,
) -> Result<Vec<db::RuntimeInstructionRow>, String> {
  state
    .list_runtime_instructions(scope_type.as_deref(), enabled_only.unwrap_or(true))
    .map_err(|err| err.to_string())
}

#[tauri::command]
fn runtime_instruction_effective(
  state: tauri::State<'_, Arc<Database>>,
  cwd: String,
) -> Result<Vec<db::RuntimeInstructionRow>, String> {
  let rows = state
    .list_runtime_instructions(None, true)
    .map_err(|err| err.to_string())?;
  Ok(filter_runtime_instructions_for_cwd(rows, &cwd))
}

#[tauri::command]
fn worker_sandbox_create(
  app: tauri::AppHandle,
  state: tauri::State<'_, Arc<Database>>,
  request: WorkerSandboxCreateRequest,
) -> Result<db::WorkerSandboxRow, String> {
  let mode = request
    .mode
    .as_deref()
    .unwrap_or("workspace_copy")
    .trim()
    .to_lowercase();
  if mode != "workspace_copy" && mode != "native" && mode != "wsl" {
    return Err(format!(
      "sandbox mode '{}' wird nicht unterstuetzt (erlaubt: workspace_copy, native, wsl)",
      mode
    ));
  }
  if mode == "wsl" && !cfg!(target_os = "windows") {
    return Err("sandbox mode 'wsl' ist nur unter Windows verfuegbar".to_string());
  }

  let source_cwd = PathBuf::from(&request.source_cwd)
    .canonicalize()
    .map_err(|err| err.to_string())?;
  if !source_cwd.is_dir() {
    return Err("source_cwd muss ein verzeichnis sein".to_string());
  }

  let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
  let backend = if let Some(backend_id) = request.backend_id.as_deref() {
    state
      .list_terminal_backends()
      .map_err(|err| err.to_string())?
      .into_iter()
      .find(|item| item.id == backend_id)
      .ok_or_else(|| format!("backend '{}' nicht gefunden", backend_id))?
  } else {
    terminal_backends::ensure_default_local_backend(&state)?
  };

  let workspace = if mode == "native" {
    let sandbox_root = worker_sandbox::sandbox_root(&app_data_dir, &request.id);
    fs::create_dir_all(&sandbox_root).map_err(|err| err.to_string())?;
    worker_sandbox::WorkspacePrepareResult {
      sandbox_root: sandbox_root.display().to_string(),
      workspace_root: source_cwd.display().to_string(),
      copied_files: 0,
      skipped_files: 0,
      skipped_dirs: Vec::new(),
    }
  } else {
    worker_sandbox::prepare_workspace_snapshot(&app_data_dir, &request.id, &source_cwd)?
  };
  let allowed_roots_json = serde_json::to_string(&vec![workspace.workspace_root.clone()])
    .map_err(|err| err.to_string())?;
  let read_only_roots_json = if request.allow_file_write.unwrap_or(true) {
    None
  } else {
    Some(allowed_roots_json.clone())
  };

  let metadata_json = serde_json::json!({
    "copiedFiles": workspace.copied_files,
    "skippedFiles": workspace.skipped_files,
    "skippedDirs": workspace.skipped_dirs,
    "mode": mode,
    "workspaceStrategy": if mode == "native" { "in_place" } else { "snapshot_copy" },
    "sourceCwd": source_cwd.display().to_string(),
    "sandboxRoot": workspace.sandbox_root,
    "requestedMetadata": request.metadata_json,
  })
  .to_string();

  state
    .insert_worker_sandbox(
      &request.id,
      &request.run_id,
      request.parent_run_id.as_deref(),
      Some(&backend.id),
      "active",
      &mode,
      &source_cwd.display().to_string(),
      &workspace.workspace_root,
      &allowed_roots_json,
      read_only_roots_json.as_deref(),
      request.allow_file_read.unwrap_or(true),
      request.allow_file_write.unwrap_or(true),
      request.allow_shell_execution.unwrap_or(true),
      request.allow_web_fetch.unwrap_or(false),
      request.allow_web_search.unwrap_or(false),
      request.allow_mcp.unwrap_or(false),
      request.env_json.as_deref(),
      Some(&metadata_json),
    )
    .map_err(|err| err.to_string())?;

  let event_payload = serde_json::json!({
    "sandboxId": request.id,
    "workspaceRoot": workspace.workspace_root,
    "backendId": backend.id,
    "copiedFiles": workspace.copied_files,
    "skippedFiles": workspace.skipped_files,
  })
  .to_string();
  let _ = state.insert_engine_run_event(
    &uuid::Uuid::new_v4().to_string(),
    &request.run_id,
    "worker_sandbox_created",
    Some(&event_payload),
  );

  state
    .get_worker_sandbox(&request.id)
    .map_err(|err| err.to_string())?
    .ok_or_else(|| "sandbox konnte nicht geladen werden".to_string())
}

#[tauri::command]
fn worker_sandbox_get(
  state: tauri::State<'_, Arc<Database>>,
  id: String,
) -> Result<Option<db::WorkerSandboxRow>, String> {
  state.get_worker_sandbox(&id).map_err(|err| err.to_string())
}

#[tauri::command]
fn worker_sandbox_get_for_run(
  state: tauri::State<'_, Arc<Database>>,
  run_id: String,
) -> Result<Option<db::WorkerSandboxRow>, String> {
  state.get_worker_sandbox_by_run(&run_id).map_err(|err| err.to_string())
}

#[tauri::command]
fn worker_sandbox_list(
  state: tauri::State<'_, Arc<Database>>,
  limit: Option<i64>,
  status: Option<String>,
) -> Result<Vec<db::WorkerSandboxRow>, String> {
  state
    .list_worker_sandboxes(limit.unwrap_or(100).clamp(1, 500), status.as_deref())
    .map_err(|err| err.to_string())
}

#[tauri::command]
fn worker_sandbox_update(
  state: tauri::State<'_, Arc<Database>>,
  request: WorkerSandboxUpdateRequest,
) -> Result<(), String> {
  state
    .update_worker_sandbox(
      &request.id,
      request.status.as_deref(),
      request.metadata_json.as_deref(),
    )
    .map_err(|err| err.to_string())
}

#[tauri::command]
fn worker_sandbox_destroy(
  app: tauri::AppHandle,
  state: tauri::State<'_, Arc<Database>>,
  id: String,
  remove_files: Option<bool>,
) -> Result<(), String> {
  if remove_files.unwrap_or(true) {
    let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
    worker_sandbox::destroy_workspace_snapshot(&app_data_dir, &id)?;
  }
  state
    .update_worker_sandbox(&id, Some("destroyed"), None)
    .map_err(|err| err.to_string())
}

// -- Helpers ----------------------------------------------------------------

fn default_true() -> bool {
  true
}

fn default_policy_flags() -> PolicyFlagsPayload {
  PolicyFlagsPayload {
    strict_policy_enforcement: true,
    allow_tool_dispatcher: true,
    allow_mcp_tool_calls: true,
    allow_web_fetch: true,
    allow_file_read_extraction: true,
    auto_compact_long_context: true,
    allow_shell_execution: true,
    allow_web_search: true,
  }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScheduledPromptRuntimeConfig {
  #[serde(flatten)]
  config: OllamaConfig,
  #[serde(default)]
  cwd: Option<String>,
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
  if pattern == "*" {
    return true;
  }

  if !pattern.contains('*') {
    return pattern.eq_ignore_ascii_case(text);
  }

  let mut remainder = text.to_lowercase();
  let pattern_lower = pattern.to_lowercase();
  let parts: Vec<&str> = pattern_lower.split('*').collect();
  let anchored_start = !pattern_lower.starts_with('*');
  let anchored_end = !pattern_lower.ends_with('*');

  if anchored_start {
    let first = parts.first().copied().unwrap_or("");
    if !remainder.starts_with(first) {
      return false;
    }
    remainder = remainder[first.len()..].to_string();
  }

  let mut idx = if anchored_start { 1 } else { 0 };
  let mut end_guard = parts.len();
  if anchored_end && !parts.is_empty() {
    end_guard -= 1;
  }

  while idx < end_guard {
    let part = parts[idx];
    if part.is_empty() {
      idx += 1;
      continue;
    }
    if let Some(found_at) = remainder.find(part) {
      remainder = remainder[found_at + part.len()..].to_string();
      idx += 1;
      continue;
    }
    return false;
  }

  if anchored_end {
    let last = parts.last().copied().unwrap_or("");
    return remainder.ends_with(last);
  }

  true
}

fn matches_deny_rule(rule: &str, tool: &str, target: &str) -> bool {
  let trimmed = rule.trim();
  if trimmed.is_empty() {
    return false;
  }

  let (rule_tool, rule_target) = if let Some(split_idx) = trimmed.find(':') {
    (&trimmed[..split_idx], &trimmed[split_idx + 1..])
  } else {
    (trimmed, "*")
  };

  wildcard_match(rule_tool, tool) && wildcard_match(rule_target, target)
}

fn canonical_policy_tool_id(tool: &str) -> String {
  let trimmed = tool.trim();
  let normalized = trimmed.to_ascii_lowercase();

  match normalized.as_str() {
    "delegate" | "delegatetask" | "delegate_task" | "delegation" | "handoff" => "delegate_task".to_string(),
    "shell" | "bash" | "bashtool" => "bash".to_string(),
    "read" | "read_file" | "filereadtool" => "read_file".to_string(),
    "edit" | "edit_file" | "fileedittool" | "write" | "multiedit" | "append" => "edit_file".to_string(),
    "createdirectory" | "create_directory" | "mkdir" | "make_dir" => "create_directory".to_string(),
    "movepath" | "move_path" | "rename_path" => "move_path".to_string(),
    "copypath" | "copy_path" => "copy_path".to_string(),
    "glob" => "glob".to_string(),
    "grep" | "search" => "grep".to_string(),
    "webfetch" | "web_fetch" => "web_fetch".to_string(),
    "websearch" | "web_search" => "web_search".to_string(),
    "mcptool" | "mcp" | "mcp_call" => "mcp".to_string(),
    "askuser" | "ask_user" => "ask_user".to_string(),
    "taskcreate" | "tasklist" | "taskupdate" | "todo" => "todo".to_string(),
    _ if normalized.is_empty() => trimmed.to_string(),
    _ => normalized,
  }
}

fn default_policy_enabled_tool_ids_vec() -> Vec<String> {
  DEFAULT_POLICY_ENABLED_TOOL_IDS
    .iter()
    .map(|tool_id| (*tool_id).to_string())
    .collect()
}

fn normalize_policy_enabled_tool_ids(enabled_tool_ids: &[String]) -> Vec<String> {
  let requested = enabled_tool_ids
    .iter()
    .map(|tool_id| canonical_policy_tool_id(tool_id))
    .filter(|tool_id| DEFAULT_POLICY_ENABLED_TOOL_IDS.contains(&tool_id.as_str()))
    .collect::<HashSet<_>>();

  DEFAULT_POLICY_ENABLED_TOOL_IDS
    .iter()
    .filter(|tool_id| requested.contains(**tool_id))
    .map(|tool_id| (*tool_id).to_string())
    .collect()
}

fn build_policy_tool_states(enabled_tool_ids: &[String]) -> Vec<(String, bool)> {
  let enabled = enabled_tool_ids.iter().cloned().collect::<HashSet<_>>();

  DEFAULT_POLICY_ENABLED_TOOL_IDS
    .iter()
    .map(|tool_id| ((*tool_id).to_string(), enabled.contains(*tool_id)))
    .collect()
}

fn enforce_tool_policy(
  policy: &PolicyStatePayload,
  tool: &str,
  target: &str,
  tool_allowed_by_flag: bool,
) -> Result<(), String> {
  let canonical_tool = canonical_policy_tool_id(tool);

  if !tool_allowed_by_flag {
    return Err(format!("policy blockiert {}", canonical_tool));
  }

  if !policy.flags.strict_policy_enforcement {
    return Ok(());
  }

  if !policy.enabled_tool_ids.iter().any(|enabled_tool_id| enabled_tool_id == &canonical_tool) {
    return Err(format!("tool {} ist im Profil deaktiviert", canonical_tool));
  }

  if policy
    .deny_rules
    .iter()
    .any(|rule| matches_deny_rule(rule, canonical_tool.as_str(), target))
  {
    return Err(format!("deny rule blockiert {}:{}", canonical_tool, target));
  }

  Ok(())
}

fn load_run_sandbox(
  state: &Arc<Database>,
  run_id: Option<&str>,
) -> Result<Option<db::WorkerSandboxRow>, String> {
  let Some(active_run_id) = run_id else {
    return Ok(None);
  };
  state
    .get_worker_sandbox_by_run(active_run_id)
    .map_err(|err| err.to_string())
}

fn parse_json_string_array(input: &str) -> Result<Vec<String>, String> {
  serde_json::from_str::<Vec<String>>(input).map_err(|err| err.to_string())
}

fn enforce_worker_sandbox_flag(
  sandbox: &db::WorkerSandboxRow,
  allowed: bool,
  capability: &str,
) -> Result<(), String> {
  if sandbox.status != "active" {
    return Err(format!("sandbox {} ist nicht aktiv", sandbox.id));
  }
  if !allowed {
    return Err(format!("sandbox {} blockiert {}", sandbox.id, capability));
  }
  Ok(())
}

fn resolve_allowed_folders_for_run(
  state: &Arc<Database>,
  run_id: Option<&str>,
) -> Result<Vec<String>, String> {
  if let Some(sandbox) = load_run_sandbox(state, run_id)? {
    enforce_worker_sandbox_flag(&sandbox, sandbox.allow_file_read, "dateizugriff")?;
    return parse_json_string_array(&sandbox.allowed_roots_json);
  }

  state.list_allowed_folders().map_err(|err| err.to_string())
}

fn ensure_run_file_access(
  state: &Arc<Database>,
  run_id: Option<&str>,
  path: &str,
  write_access: bool,
) -> Result<PathBuf, String> {
  if let Some(sandbox) = load_run_sandbox(state, run_id)? {
    enforce_worker_sandbox_flag(&sandbox, sandbox.allow_file_read, "dateilesen")?;
    if write_access {
      enforce_worker_sandbox_flag(&sandbox, sandbox.allow_file_write, "dateischreiben")?;
    }
    let allowed_roots = parse_json_string_array(&sandbox.allowed_roots_json)?;
    let canonical_target = file_safety::ensure_path_allowed(PathBuf::from(path).as_path(), &allowed_roots)?;
    if write_access {
      if let Some(read_only_roots_json) = sandbox.read_only_roots_json.as_deref() {
        let read_only_roots = parse_json_string_array(read_only_roots_json)?;
        if !read_only_roots.is_empty()
          && file_safety::ensure_path_allowed(canonical_target.as_path(), &read_only_roots).is_ok()
        {
          return Err(format!("sandbox {} erlaubt nur lesen fuer {}", sandbox.id, path));
        }
      }
    }
    return Ok(canonical_target);
  }

  let allowed_folders = state.list_allowed_folders().map_err(|err| err.to_string())?;
  file_safety::ensure_path_allowed(PathBuf::from(path).as_path(), &allowed_folders)
}

fn ensure_run_cwd(
  state: &Arc<Database>,
  run_id: Option<&str>,
  requested_cwd: Option<&str>,
) -> Result<Option<String>, String> {
  let Some(sandbox) = load_run_sandbox(state, run_id)? else {
    return Ok(requested_cwd.map(|value| value.to_string()));
  };

  let allowed_roots = parse_json_string_array(&sandbox.allowed_roots_json)?;
  let base = requested_cwd.unwrap_or(sandbox.workspace_root.as_str());
  let canonical = file_safety::ensure_path_allowed(PathBuf::from(base).as_path(), &allowed_roots)?;
  Ok(Some(canonical.display().to_string()))
}

fn resolve_shell_allowed_roots(
  state: &Arc<Database>,
  run_id: Option<&str>,
) -> Result<Vec<String>, String> {
  if let Some(sandbox) = load_run_sandbox(state, run_id)? {
    return parse_json_string_array(&sandbox.allowed_roots_json);
  }
  state.list_allowed_folders().map_err(|err| err.to_string())
}

fn split_shell_tokens(command: &str) -> Vec<String> {
  let mut tokens = Vec::new();
  let mut current = String::new();
  let mut quote: Option<char> = None;

  for ch in command.chars() {
    if let Some(active_quote) = quote {
      current.push(ch);
      if ch == active_quote {
        quote = None;
      }
      continue;
    }

    if ch == '"' || ch == '\'' {
      quote = Some(ch);
      current.push(ch);
      continue;
    }

    if ch.is_whitespace() || ch == ';' || ch == '|' || ch == '&' {
      if !current.trim().is_empty() {
        tokens.push(current.clone());
      }
      current.clear();
      continue;
    }

    current.push(ch);
  }

  if !current.trim().is_empty() {
    tokens.push(current);
  }

  tokens
}

fn is_windows_drive_path(value: &str) -> bool {
  let bytes = value.as_bytes();
  bytes.len() >= 3
    && bytes[0].is_ascii_alphabetic()
    && bytes[1] == b':'
    && (bytes[2] == b'\\' || bytes[2] == b'/')
}

fn is_absolute_path_candidate(value: &str) -> bool {
  value.starts_with("\\\\") || value.starts_with('/') || is_windows_drive_path(value)
}

fn normalize_path_token(token: &str) -> String {
  token
    .trim()
    .trim_matches('"')
    .trim_matches('\'')
    .trim_matches('`')
    .trim_matches('(')
    .trim_matches(')')
    .trim_matches('{')
    .trim_matches('}')
    .trim_matches('[')
    .trim_matches(']')
    .trim_matches(',')
    .to_string()
}

fn extract_absolute_path_candidates(command: &str) -> Vec<String> {
  let mut paths = Vec::new();

  for token in split_shell_tokens(command) {
    let mut candidate = normalize_path_token(&token);
    if let Some(eq_idx) = candidate.find('=') {
      let rhs = normalize_path_token(&candidate[eq_idx + 1..]);
      if !rhs.is_empty() {
        candidate = rhs;
      }
    }

    if candidate.is_empty() || !is_absolute_path_candidate(&candidate) {
      continue;
    }

    if !paths.iter().any(|existing| existing == &candidate) {
      paths.push(candidate);
    }
  }

  paths
}

fn command_contains_path_traversal(command: &str) -> bool {
  if command.contains("../") || command.contains("..\\") {
    return true;
  }

  for token in split_shell_tokens(command) {
    let normalized = normalize_path_token(&token).replace('\\', "/");
    if normalized == ".."
      || normalized.starts_with("../")
      || normalized.ends_with("/..")
      || normalized.contains("/../")
    {
      return true;
    }
  }

  false
}

fn detect_dangerous_shell_pattern(command: &str) -> Option<&'static str> {
  let lower = command.to_lowercase();
  let compact = lower.replace('\n', " ");

  if (compact.contains("curl") || compact.contains("wget"))
    && compact.contains('|')
    && (compact.contains("| bash")
      || compact.contains("|sh")
      || compact.contains("| sh")
      || compact.contains("| pwsh")
      || compact.contains("| powershell"))
  {
    return Some("remote script piping ist blockiert");
  }

  if compact.contains("rm -rf /")
    || compact.contains("rm -fr /")
    || compact.contains("rm -rf ~")
    || compact.contains("mkfs")
    || compact.contains(" dd if=")
    || compact.starts_with("dd if=")
    || compact.contains("> /dev/")
    || compact.contains("format c:")
    || compact.contains("del /s")
    || compact.contains("rmdir /s")
    || compact.contains("set-executionpolicy")
    || (compact.contains("powershell") && compact.contains("-enc"))
  {
    return Some("potenziell destruktives shell-muster erkannt");
  }

  None
}

fn enforce_shell_command_guard(
  state: &Arc<Database>,
  run_id: Option<&str>,
  command_text: &str,
  effective_cwd: Option<&str>,
) -> Result<(), String> {
  let allowed_roots = resolve_shell_allowed_roots(state, run_id)?;

  if let Some(cwd) = effective_cwd {
    file_safety::ensure_path_allowed(Path::new(cwd), &allowed_roots)
      .map_err(|_| format!("working directory liegt ausserhalb erlaubter roots: {}", cwd))?;
  }

  if command_contains_path_traversal(command_text) {
    return Err("command blockiert: path traversal (..) ist nicht erlaubt".to_string());
  }

  for path_candidate in extract_absolute_path_candidates(command_text) {
    if file_safety::ensure_path_allowed(Path::new(&path_candidate), &allowed_roots).is_err() {
      return Err(format!(
        "command blockiert: absoluter pfad ausserhalb erlaubter roots: {}",
        path_candidate
      ));
    }
  }

  if let Some(reason) = detect_dangerous_shell_pattern(command_text) {
    return Err(format!("command blockiert: {}", reason));
  }

  Ok(())
}

fn parse_env_vars_json(env_json: Option<&str>) -> Result<HashMap<String, String>, String> {
  match env_json {
    Some(text) if !text.trim().is_empty() => {
      serde_json::from_str::<HashMap<String, String>>(text).map_err(|err| err.to_string())
    }
    _ => Ok(HashMap::new()),
  }
}

fn load_policy_state(state: &Arc<Database>) -> Result<PolicyStatePayload, String> {
  let stored_flags = state.list_policy_flags().map_err(|err| err.to_string())?;
  let mut flags = default_policy_flags();

  for (key, value) in stored_flags {
    match key.as_str() {
      POLICY_FLAG_STRICT => flags.strict_policy_enforcement = value,
      POLICY_FLAG_TOOL_DISPATCHER => flags.allow_tool_dispatcher = value,
      POLICY_FLAG_MCP => flags.allow_mcp_tool_calls = value,
      POLICY_FLAG_WEB_FETCH => flags.allow_web_fetch = value,
      POLICY_FLAG_FILE_READ => flags.allow_file_read_extraction = value,
      POLICY_FLAG_AUTO_COMPACT => flags.auto_compact_long_context = value,
      POLICY_FLAG_SHELL_EXECUTION => flags.allow_shell_execution = value,
      POLICY_FLAG_WEB_SEARCH => flags.allow_web_search = value,
      _ => {}
    }
  }

  let deny_rules = state
    .list_policy_deny_rules()
    .map_err(|err| err.to_string())?;

  let stored_tool_states = state
    .list_policy_tool_states()
    .map_err(|err| err.to_string())?;

  let enabled_tool_ids = if stored_tool_states.is_empty() {
    default_policy_enabled_tool_ids_vec()
  } else {
    let stored_tool_states = stored_tool_states.into_iter().collect::<HashMap<_, _>>();
    DEFAULT_POLICY_ENABLED_TOOL_IDS
      .iter()
      .filter(|tool_id| stored_tool_states.get(**tool_id).copied().unwrap_or(false))
      .map(|tool_id| (*tool_id).to_string())
      .collect()
  };

  Ok(PolicyStatePayload { flags, deny_rules, enabled_tool_ids })
}

fn map_ollama_error(err: OllamaError) -> String {
  err.to_string()
}

fn map_mcp_error(err: McpError) -> String {
  err.to_string()
}

fn extract_html_title(input: &str) -> Option<String> {
  let lower = input.to_lowercase();
  let start = lower.find("<title>")? + "<title>".len();
  let end = lower[start..].find("</title>")? + start;
  Some(input[start..end].trim().to_string())
}

fn strip_html_like_content(input: &str) -> String {
  let mut output = String::new();
  let mut inside_tag = false;
  let mut previous_was_space = false;

  for ch in input.chars() {
    match ch {
      '<' => {
        inside_tag = true;
      }
      '>' => {
        inside_tag = false;
      }
      _ if !inside_tag => {
        let normalized = if ch.is_whitespace() { ' ' } else { ch };
        if normalized == ' ' {
          if !previous_was_space {
            output.push(' ');
          }
          previous_was_space = true;
        } else {
          output.push(normalized);
          previous_was_space = false;
        }
      }
      _ => {}
    }
  }

  output
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
  input.chars().take(max_chars).collect()
}

fn decode_html_entities(input: &str) -> String {
  input
    .replace("&amp;", "&")
    .replace("&quot;", "\"")
    .replace("&#x27;", "'")
    .replace("&#39;", "'")
    .replace("&lt;", "<")
    .replace("&gt;", ">")
}

fn extract_anchor_href(fragment: &str) -> Option<String> {
  let href_idx = fragment.find("href=\"")? + 6;
  let href_rest = &fragment[href_idx..];
  let href_end = href_rest.find('"')?;
  Some(decode_html_entities(&href_rest[..href_end]))
}

fn extract_anchor_text(fragment: &str) -> Option<String> {
  let start = fragment.find('>')? + 1;
  let end = fragment[start..].find("</a>")? + start;
  Some(decode_html_entities(fragment[start..end].trim()))
}

fn parse_duckduckgo_results(body: &str, max_results: usize) -> Vec<WebSearchResultItem> {
  let mut results = Vec::new();
  let mut remainder = body;

  while results.len() < max_results {
    let Some(anchor_pos) = remainder.find("result__a") else {
      break;
    };
    remainder = &remainder[anchor_pos..];
    let Some(tag_end) = remainder.find("</a>") else {
      break;
    };
    let anchor = &remainder[..tag_end + 4];
    remainder = &remainder[tag_end + 4..];

    let Some(raw_href) = extract_anchor_href(anchor) else {
      continue;
    };
    let url = if let Some(idx) = raw_href.find("uddg=") {
      let encoded = &raw_href[idx + 5..];
      let candidate = format!("https://dummy.invalid/?uddg={}", encoded);
      url::Url::parse(&candidate)
        .ok()
        .and_then(|parsed| {
          parsed
            .query_pairs()
            .find(|(key, _)| key == "uddg")
            .map(|(_, value)| value.to_string())
        })
        .unwrap_or_else(|| raw_href.clone())
    } else {
      raw_href.clone()
    };
    let title = extract_anchor_text(anchor).unwrap_or_else(|| url.clone());

    let snippet = if let Some(snippet_idx) = remainder.find("result__snippet") {
      let snippet_rest = &remainder[snippet_idx..];
      if let Some(snippet_end) = snippet_rest.find("</a>") {
        strip_html_like_content(&snippet_rest[..snippet_end])
      } else if let Some(snippet_end) = snippet_rest.find("</div>") {
        strip_html_like_content(&snippet_rest[..snippet_end])
      } else {
        String::new()
      }
    } else {
      String::new()
    };

    results.push(WebSearchResultItem {
      title,
      url,
      snippet: snippet.trim().to_string(),
    });
  }

  results
}

fn emit_exec_chunk(app: &tauri::AppHandle, stream_id: Option<&str>, channel: &str, content: &str) {
  if let Some(active_stream_id) = stream_id {
    let payload = serde_json::json!({
      "streamId": active_stream_id,
      "channel": channel,
      "content": content,
    });
    let _ = app.emit("exec-command-chunk", payload);
  }
}

const EXEC_CURRENT_CWD_MARKER: &str = "__OPEN_COWORK_CURRENT_CWD__=";

fn build_exec_command_text(command_text: &str, force_posix_shell: bool) -> String {
  if cfg!(target_os = "windows") && !force_posix_shell {
    format!(
      "{command_text}; $openCoworkExit = if ($null -ne $LASTEXITCODE) {{ $LASTEXITCODE }} elseif ($?) {{ 0 }} else {{ 1 }}; Write-Output ('{marker}' + (Get-Location).Path); exit $openCoworkExit",
      marker = EXEC_CURRENT_CWD_MARKER,
    )
  } else {
    format!(
      "{command_text}; open_cowork_exit=$?; printf '%s%s\\n' '{marker}' \"$PWD\"; exit $open_cowork_exit",
      marker = EXEC_CURRENT_CWD_MARKER,
    )
  }
}

fn windows_path_to_wsl(path: &str) -> Option<String> {
  let normalized = path.replace('\\', "/");
  let bytes = normalized.as_bytes();
  if bytes.len() >= 3
    && bytes[0].is_ascii_alphabetic()
    && bytes[1] == b':'
    && bytes[2] == b'/'
  {
    let drive = normalized[0..1].to_lowercase();
    let remainder = normalized[3..].trim_start_matches('/');
    if remainder.is_empty() {
      return Some(format!("/mnt/{}", drive));
    }
    return Some(format!("/mnt/{}/{}", drive, remainder));
  }

  if normalized.starts_with('/') {
    return Some(normalized);
  }

  None
}

fn escape_bash_single_quotes(input: &str) -> String {
  input.replace('\'', "'\"'\"'")
}

fn extract_current_cwd_from_stdout(stdout: &str) -> (String, Option<String>) {
  let mut cleaned_lines = Vec::new();
  let mut current_cwd: Option<String> = None;

  for line in stdout.lines() {
    if let Some(value) = line.strip_prefix(EXEC_CURRENT_CWD_MARKER) {
      let normalized = value.trim();
      if !normalized.is_empty() {
        current_cwd = Some(normalized.to_string());
      }
      continue;
    }

    cleaned_lines.push(line);
  }

  (cleaned_lines.join("\n"), current_cwd)
}

fn resolve_exec_runtime(
  state: &Arc<Database>,
  backend_id: Option<&str>,
  run_id: Option<&str>,
) -> Result<(Option<String>, HashMap<String, String>, Option<String>), String> {
  let mut shell_override: Option<String> = None;
  let mut env_vars: HashMap<String, String> = HashMap::new();
  let mut runtime_mode: Option<String> = None;

  if let Some(active_run_id) = run_id {
    if let Some(sandbox) = load_run_sandbox(state, Some(active_run_id))? {
      enforce_worker_sandbox_flag(&sandbox, sandbox.allow_shell_execution, "shell-ausfuehrung")?;
      env_vars.extend(parse_env_vars_json(sandbox.env_json.as_deref())?);
      env_vars.insert("OPEN_COWORK_SANDBOX_ID".to_string(), sandbox.id.clone());
      env_vars.insert("OPEN_COWORK_RUN_ID".to_string(), sandbox.run_id.clone());
      runtime_mode = Some(sandbox.mode.clone());
    }
  }

  let selected_backend_id = if let Some(explicit_backend_id) = backend_id {
    Some(explicit_backend_id.to_string())
  } else if let Some(sandbox) = load_run_sandbox(state, run_id)? {
    sandbox.backend_id
  } else {
    None
  };

  if let Some(active_backend_id) = selected_backend_id.as_deref() {
    let backend = state
      .list_terminal_backends()
      .map_err(|err| err.to_string())?
      .into_iter()
      .find(|item| item.id == active_backend_id)
      .ok_or_else(|| format!("backend '{}' nicht gefunden", active_backend_id))?;

    if backend.backend_type != "local" {
      return Err(format!(
        "backend '{}' wird fuer sandboxed exec noch nicht unterstuetzt",
        backend.backend_type
      ));
    }

    let config = serde_json::from_str::<terminal_backends::LocalBackendConfig>(&backend.config_json)
      .map_err(|err| err.to_string())?;
    shell_override = config.shell;
    if let Some(backend_env) = config.env_vars {
      env_vars.extend(backend_env);
    }
  }

  if cfg!(target_os = "windows")
    && runtime_mode
      .as_deref()
      .map(|mode| mode.eq_ignore_ascii_case("wsl"))
      .unwrap_or(false)
    && shell_override.is_none()
  {
    shell_override = Some("wsl".to_string());
  }

  Ok((shell_override, env_vars, runtime_mode))
}

fn run_command_once(
  app: &tauri::AppHandle,
  stream_id: Option<&str>,
  command_text: &str,
  cwd: Option<&str>,
  timeout_ms: u64,
  shell_override: Option<&str>,
  runtime_mode: Option<&str>,
  env_vars: &HashMap<String, String>,
) -> Result<ExecCommandResponse, String> {
  let is_wsl_mode = cfg!(target_os = "windows")
    && (runtime_mode
      .map(|mode| mode.eq_ignore_ascii_case("wsl"))
      .unwrap_or(false)
      || shell_override
        .map(|shell| shell.eq_ignore_ascii_case("wsl") || shell.eq_ignore_ascii_case("wsl.exe"))
        .unwrap_or(false));

  let shell = if is_wsl_mode {
    "wsl"
  } else {
    shell_override.unwrap_or(if cfg!(target_os = "windows") {
      "powershell"
    } else {
      "sh"
    })
  };

  let wrapped_command_text = build_exec_command_text(command_text, is_wsl_mode);

  let mut command = if cfg!(target_os = "windows") {
    if is_wsl_mode {
      let mut cmd = Command::new(shell);
      let wrapped_for_wsl = if let Some(dir) = cwd.and_then(windows_path_to_wsl) {
        format!(
          "cd '{}' && {}",
          escape_bash_single_quotes(&dir),
          wrapped_command_text
        )
      } else {
        wrapped_command_text.clone()
      };
      cmd.args(["-e", "bash", "-lc", wrapped_for_wsl.as_str()]);
      cmd
    } else {
      let mut cmd = Command::new(shell);
      let shell_lower = shell.to_ascii_lowercase();
      if shell_lower.contains("powershell") || shell_lower.ends_with("pwsh") || shell_lower.ends_with("pwsh.exe") {
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", wrapped_command_text.as_str()]);
      } else if shell_lower.ends_with("cmd") || shell_lower.ends_with("cmd.exe") {
        cmd.args(["/C", wrapped_command_text.as_str()]);
      } else {
        cmd.args(["-c", wrapped_command_text.as_str()]);
      }
      cmd
    }
  } else {
    let mut cmd = Command::new(shell);
    cmd.args(["-c", wrapped_command_text.as_str()]);
    cmd
  };

  if !is_wsl_mode {
    if let Some(dir) = cwd {
      command.current_dir(dir);
    }
  }

  for (key, value) in env_vars {
    command.env(key, value);
  }

  command.stdout(Stdio::piped());
  command.stderr(Stdio::piped());

  let mut child = command.spawn().map_err(|err| err.to_string())?;
  let stdout = child.stdout.take().ok_or_else(|| "stdout pipe unavailable".to_string())?;
  let stderr = child.stderr.take().ok_or_else(|| "stderr pipe unavailable".to_string())?;

  let stream_for_stdout = stream_id.map(|value| value.to_string());
  let stream_for_stderr = stream_id.map(|value| value.to_string());
  let app_for_stdout = app.clone();
  let app_for_stderr = app.clone();

  let stdout_handle = thread::spawn(move || {
    let mut buffer = String::new();
    let reader = BufReader::new(stdout);
    for line in reader.lines() {
      if let Ok(text) = line {
        buffer.push_str(&text);
        buffer.push('\n');
        emit_exec_chunk(&app_for_stdout, stream_for_stdout.as_deref(), "stdout", &text);
      }
    }
    buffer
  });

  let stderr_handle = thread::spawn(move || {
    let mut buffer = String::new();
    let reader = BufReader::new(stderr);
    for line in reader.lines() {
      if let Ok(text) = line {
        buffer.push_str(&text);
        buffer.push('\n');
        emit_exec_chunk(&app_for_stderr, stream_for_stderr.as_deref(), "stderr", &text);
      }
    }
    buffer
  });

  let wait_started = Instant::now();
  let mut timed_out = false;
  let exit_status = loop {
    match child.try_wait() {
      Ok(Some(status)) => break Some(status),
      Ok(None) => {
        if wait_started.elapsed().as_millis() as u64 >= timeout_ms {
          timed_out = true;
          let _ = child.kill();
          let _ = child.wait();
          break None;
        }
        thread::sleep(Duration::from_millis(50));
      }
      Err(err) => return Err(err.to_string()),
    }
  };

  let stdout_text = stdout_handle.join().unwrap_or_default();
  let stderr_text = stderr_handle.join().unwrap_or_default();
  let (stdout_text, extracted_cwd) = extract_current_cwd_from_stdout(&stdout_text);
  let exit_code = exit_status.and_then(|status| status.code());
  let normalized_status = if timed_out {
    "timed_out"
  } else if exit_code == Some(0) {
    "success"
  } else if exit_code.is_some() {
    "error"
  } else {
    "terminated"
  };

  emit_exec_chunk(app, stream_id, "done", normalized_status);

  Ok(ExecCommandResponse {
    stdout: stdout_text,
    stderr: stderr_text,
    exit_code,
    current_cwd: extracted_cwd.or_else(|| cwd.map(|value| value.to_string())),
    timed_out,
    duration_ms: wait_started.elapsed().as_millis() as u64,
    attempts: 1,
    normalized_status: normalized_status.to_string(),
  })
}

fn filter_runtime_instructions_for_cwd(
  rows: Vec<db::RuntimeInstructionRow>,
  cwd: &str,
) -> Vec<db::RuntimeInstructionRow> {
  let normalized_cwd = cwd.replace('\\', "/").to_lowercase();

  rows
    .into_iter()
    .filter(|row| {
      if !row.enabled {
        return false;
      }
      match row.scope_type.as_str() {
        "global" => true,
        "workspace" => row.scope_ref.as_deref().map(|scope| normalized_cwd.starts_with(&scope.replace('\\', "/").to_lowercase())).unwrap_or(false),
        "folder" => row.scope_ref.as_deref().map(|scope| normalized_cwd.starts_with(&scope.replace('\\', "/").to_lowercase())).unwrap_or(false),
        _ => false,
      }
    })
    .collect()
}

fn configure_pdfium_search_paths(app: &tauri::AppHandle) {
  let mut candidates = Vec::new();

  if let Ok(resource_dir) = app.path().resource_dir() {
    candidates.push(
      resource_dir
        .join("resources")
        .join("pdfium")
        .join("bin")
        .join("pdfium.dll"),
    );
    candidates.push(
      resource_dir
        .join("pdfium")
        .join("bin")
        .join("pdfium.dll"),
    );
  }

  if let Ok(current_exe) = std::env::current_exe() {
    if let Some(exe_dir) = current_exe.parent() {
      candidates.push(exe_dir.join("pdfium.dll"));
      candidates.push(
        exe_dir
          .join("resources")
          .join("pdfium")
          .join("bin")
          .join("pdfium.dll"),
      );
    }
  }

  artifact_pipeline::set_pdfium_search_paths(candidates);
}

// -- Memory commands --------------------------------------------------------

#[tauri::command]
fn memory_upsert(
  state: tauri::State<'_, Arc<Database>>,
  id: String,
  scope: String,
  category: String,
  key: String,
  content: String,
  source_session_id: Option<String>,
  confidence: Option<f64>,
) -> Result<(), String> {
  memory_engine::validate_scope(&scope)?;
  let conf = confidence.unwrap_or(1.0);
  if memory_engine::is_duplicate_memory(&state, &scope, &category, &key, &content) {
    return Ok(());
  }
  state
    .upsert_memory_entry(&id, &scope, &category, &key, &content, source_session_id.as_deref(), conf)
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn memory_delete(state: tauri::State<'_, Arc<Database>>, id: String) -> Result<(), String> {
  state.delete_memory_entry(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn memory_search(
  state: tauri::State<'_, Arc<Database>>,
  scope: Option<String>,
  category: Option<String>,
  keyword: Option<String>,
  limit: Option<i64>,
) -> Result<Vec<db::MemoryEntryRow>, String> {
  let lim = limit.unwrap_or(100);
  if let Some(ref kw) = keyword {
    state
      .search_memory_entries(kw, lim)
      .map_err(|e| e.to_string())
  } else {
    state
      .list_memory_entries(
        &scope.unwrap_or_else(|| "agent".to_string()),
        category.as_deref(),
        lim,
      )
      .map_err(|e| e.to_string())
  }
}

#[tauri::command]
fn memory_compact(
  state: tauri::State<'_, Arc<Database>>,
  scope: String,
  min_confidence: f64,
) -> Result<memory_engine::MemoryCompactResponse, String> {
  let db_arc = state.inner().clone();
  memory_engine::compact_low_confidence(&db_arc, &scope, min_confidence)
}

#[tauri::command]
fn memory_snapshot(
  state: tauri::State<'_, Arc<Database>>,
) -> Result<memory_engine::FrozenMemorySnapshot, String> {
  let db_arc = state.inner().clone();
  memory_engine::create_memory_snapshot(&db_arc)
}

#[tauri::command]
fn memory_hints(
  state: tauri::State<'_, Arc<Database>>,
) -> Result<Vec<memory_engine::MemoryHint>, String> {
  let db_arc = state.inner().clone();
  Ok(memory_engine::generate_memory_hints(&db_arc))
}

// -- User profile commands --------------------------------------------------

#[tauri::command]
fn user_profile_upsert(
  state: tauri::State<'_, Arc<Database>>,
  id: String,
  key: String,
  value: String,
  source: String,
  confidence: Option<f64>,
) -> Result<(), String> {
  state
    .upsert_user_profile(&id, &key, &value, &source, confidence.unwrap_or(1.0))
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn user_profile_list(state: tauri::State<'_, Arc<Database>>) -> Result<Vec<db::UserProfileRow>, String> {
  state.list_user_profile().map_err(|e| e.to_string())
}

#[tauri::command]
fn user_profile_delete(state: tauri::State<'_, Arc<Database>>, key: String) -> Result<(), String> {
  state.delete_user_profile_entry(&key).map_err(|e| e.to_string())
}

// -- Skill commands ---------------------------------------------------------

#[tauri::command]
fn skill_upsert(
  state: tauri::State<'_, Arc<Database>>,
  id: String,
  name: String,
  description: String,
  prompt_template: String,
  trigger_pattern: Option<String>,
  run_mode: Option<String>,
  auto_generated: Option<bool>,
  parent_skill_id: Option<String>,
  source_task_ids: Option<String>,
) -> Result<(), String> {
  state
    .upsert_skill(
      &id,
      &name,
      &description,
      &prompt_template,
      trigger_pattern.as_deref(),
      &run_mode.unwrap_or_else(|| "execute".to_string()),
      auto_generated.unwrap_or(false),
      parent_skill_id.as_deref(),
      source_task_ids.as_deref(),
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn skill_list(state: tauri::State<'_, Arc<Database>>, limit: Option<i64>) -> Result<Vec<db::SkillRow>, String> {
  state.list_skills(limit.unwrap_or(100)).map_err(|e| e.to_string())
}

#[tauri::command]
fn skill_delete(state: tauri::State<'_, Arc<Database>>, id: String) -> Result<(), String> {
  state.delete_skill(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn skill_record_usage(
  state: tauri::State<'_, Arc<Database>>,
  id: String,
  success: bool,
  quality: Option<f64>,
) -> Result<(), String> {
  state
    .record_skill_usage(&id, success, quality.unwrap_or(0.0))
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn skill_improve(
  state: tauri::State<'_, Arc<Database>>,
  skill_id: String,
  new_prompt_template: String,
  reason: String,
) -> Result<(), String> {
  state
    .improve_skill(&skill_id, &new_prompt_template, &reason)
    .map(|_| ())
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn skill_match(
  state: tauri::State<'_, Arc<Database>>,
  user_input: String,
) -> Result<Option<db::SkillRow>, String> {
  let db_arc = state.inner().clone();
  Ok(skill_engine::match_skill_for_input(&db_arc, &user_input))
}

#[tauri::command]
fn skill_auto_generate(
  state: tauri::State<'_, Arc<Database>>,
  task_title: String,
  task_prompt: String,
  task_steps_summary: String,
  task_outcome: String,
) -> Result<skill_engine::SkillAutoGenResult, String> {
  let db_arc = state.inner().clone();
  Ok(skill_engine::analyze_for_skill_generation(
    &db_arc,
    &task_title,
    &task_prompt,
    &task_steps_summary,
    &task_outcome,
  ))
}

// -- Session commands -------------------------------------------------------

#[tauri::command]
fn session_create(
  state: tauri::State<'_, Arc<Database>>,
  id: String,
  thread_id: Option<String>,
  title: String,
  model_used: Option<String>,
  provider: Option<String>,
  personality: Option<String>,
) -> Result<(), String> {
  state
    .insert_session(
      &id,
      thread_id.as_deref(),
      &title,
      None,
      model_used.as_deref(),
      provider.as_deref(),
      personality.as_deref(),
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn session_end(
  state: tauri::State<'_, Arc<Database>>,
  id: String,
  summary: Option<String>,
  total_messages: Option<i32>,
  total_tokens_est: Option<i64>,
  outcome: Option<String>,
) -> Result<(), String> {
  state
    .end_session(
      &id,
      summary.as_deref(),
      total_messages.unwrap_or(0),
      total_tokens_est.unwrap_or(0),
      outcome.as_deref(),
      None,
      None,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn session_list(state: tauri::State<'_, Arc<Database>>, limit: Option<i64>) -> Result<Vec<db::SessionRow>, String> {
  state.list_sessions(limit.unwrap_or(100)).map_err(|e| e.to_string())
}

#[tauri::command]
fn session_search(state: tauri::State<'_, Arc<Database>>, query: String, limit: Option<i64>) -> Result<Vec<db::SessionSearchResultRow>, String> {
  state
    .fulltext_search_sessions(&query, limit.unwrap_or(50))
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn session_freeze_snapshot(
  state: tauri::State<'_, Arc<Database>>,
  session_id: String,
) -> Result<String, String> {
  let db_arc = state.inner().clone();
  let snapshot = memory_engine::create_memory_snapshot(&db_arc)?;
  let snapshot_json = serde_json::to_string(&snapshot).map_err(|e| e.to_string())?;
  state
    .save_session_snapshot(&session_id, &snapshot_json)
    .map_err(|e| e.to_string())?;
  Ok(snapshot_json)
}

#[tauri::command]
fn session_get(state: tauri::State<'_, Arc<Database>>, id: String) -> Result<Option<db::SessionRow>, String> {
  state.get_session(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn session_delete(state: tauri::State<'_, Arc<Database>>, id: String) -> Result<(), String> {
  state.delete_session(&id).map_err(|e| e.to_string())
}

// -- Learning outcome commands ----------------------------------------------

#[tauri::command]
fn learning_upsert(
  state: tauri::State<'_, Arc<Database>>,
  id: String,
  session_id: Option<String>,
  task_id: Option<String>,
  outcome_type: String,
  description: String,
  learned_pattern: Option<String>,
  confidence: Option<f64>,
) -> Result<(), String> {
  state
    .insert_learning_outcome(
      &id,
      session_id.as_deref(),
      task_id.as_deref(),
      &outcome_type,
      &description,
      learned_pattern.as_deref(),
      confidence.unwrap_or(1.0),
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn learning_list(state: tauri::State<'_, Arc<Database>>, limit: Option<i64>) -> Result<Vec<db::LearningOutcomeRow>, String> {
  state.list_learning_outcomes(limit.unwrap_or(100)).map_err(|e| e.to_string())
}

// -- Terminal backend commands ----------------------------------------------

#[tauri::command]
fn backend_upsert(
  state: tauri::State<'_, Arc<Database>>,
  id: String,
  name: String,
  backend_type: String,
  config_json: String,
) -> Result<(), String> {
  terminal_backends::validate_backend_type(&backend_type)?;
  terminal_backends::validate_backend_config(&backend_type, &config_json)?;
  state
    .upsert_terminal_backend(&id, &name, &backend_type, &config_json)
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn backend_list(state: tauri::State<'_, Arc<Database>>) -> Result<Vec<db::TerminalBackendRow>, String> {
  state.list_terminal_backends().map_err(|e| e.to_string())
}

#[tauri::command]
fn backend_delete(state: tauri::State<'_, Arc<Database>>, id: String) -> Result<(), String> {
  state.delete_terminal_backend(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn backend_exec(
  state: tauri::State<'_, Arc<Database>>,
  backend_id: String,
  command: String,
  working_dir: Option<String>,
  timeout_ms: Option<u64>,
) -> Result<terminal_backends::BackendExecResponse, String> {
  let db_arc = state.inner().clone();
  terminal_backends::dispatch_exec(&db_arc, &backend_id, &command, working_dir.as_deref(), timeout_ms)
}

#[tauri::command]
fn backend_ensure_local(
  state: tauri::State<'_, Arc<Database>>,
) -> Result<db::TerminalBackendRow, String> {
  let db_arc = state.inner().clone();
  terminal_backends::ensure_default_local_backend(&db_arc)
}

// -- Process manager commands -----------------------------------------------

#[tauri::command]
fn process_start(
  state: tauri::State<'_, Arc<Database>>,
  label: String,
  command: String,
  backend_id: Option<String>,
  requires_admin: Option<bool>,
) -> Result<process_manager::ProcessStartResult, String> {
  let db_arc = state.inner().clone();
  let request = process_manager::ProcessStartRequest {
    label,
    command,
    backend_id,
    requires_admin: requires_admin.unwrap_or(false),
  };
  Ok(process_manager::start_process(&db_arc, &request))
}

#[tauri::command]
fn process_stop(state: tauri::State<'_, Arc<Database>>, process_id: String) -> Result<(), String> {
  let db_arc = state.inner().clone();
  process_manager::stop_process(&db_arc, &process_id)
}

#[tauri::command]
fn process_approve(
  state: tauri::State<'_, Arc<Database>>,
  process_id: String,
  approved: bool,
) -> Result<process_manager::ProcessStartResult, String> {
  let db_arc = state.inner().clone();
  Ok(process_manager::approve_and_start(&db_arc, &process_id, approved))
}

#[tauri::command]
fn process_list(state: tauri::State<'_, Arc<Database>>) -> Result<Vec<process_manager::ProcessStatusResult>, String> {
  let db_arc = state.inner().clone();
  process_manager::list_process_statuses(&db_arc)
}

// -- Personality commands ---------------------------------------------------

#[tauri::command]
fn personality_upsert(
  state: tauri::State<'_, Arc<Database>>,
  id: String,
  name: String,
  description: String,
  system_prompt: String,
  temperature: Option<f64>,
  model_override: Option<String>,
  icon: Option<String>,
  is_default: Option<bool>,
) -> Result<(), String> {
  state
    .upsert_personality(
      &id,
      &name,
      &description,
      &system_prompt,
      temperature,
      model_override.as_deref(),
      icon.as_deref(),
      is_default.unwrap_or(false),
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn personality_list(state: tauri::State<'_, Arc<Database>>) -> Result<Vec<db::PersonalityRow>, String> {
  state.list_personalities().map_err(|e| e.to_string())
}

#[tauri::command]
fn personality_delete(state: tauri::State<'_, Arc<Database>>, id: String) -> Result<(), String> {
  state.delete_personality(&id).map_err(|e| e.to_string())
}

// -- Insights commands ------------------------------------------------------

#[tauri::command]
fn insights_record(
  state: tauri::State<'_, Arc<Database>>,
  event_type: String,
  category: String,
  value_num: Option<f64>,
  value_text: Option<String>,
  session_id: Option<String>,
  metadata_json: Option<String>,
) -> Result<String, String> {
  let db_arc = state.inner().clone();
  let req = insights::InsightsEventRequest {
    event_type,
    category,
    value_num,
    value_text,
    session_id,
    metadata_json,
  };
  insights::record_event(&db_arc, &req)
}

#[tauri::command]
fn insights_list(
  state: tauri::State<'_, Arc<Database>>,
  category: Option<String>,
  limit: Option<i64>,
) -> Result<Vec<db::InsightsEventRow>, String> {
  state
    .query_insights(category.as_deref(), None, limit.unwrap_or(100))
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn insights_summary(state: tauri::State<'_, Arc<Database>>) -> Result<insights::InsightsSummary, String> {
  let db_arc = state.inner().clone();
  insights::build_summary(&db_arc)
}

// -- RPC Pipeline commands --------------------------------------------------

#[tauri::command]
fn pipeline_upsert(
  state: tauri::State<'_, Arc<Database>>,
  id: String,
  name: String,
  description: Option<String>,
  steps_json: String,
  zero_context: Option<bool>,
) -> Result<(), String> {
  // Validate steps_json is valid JSON array
  serde_json::from_str::<Vec<serde_json::Value>>(&steps_json)
    .map_err(|e| format!("steps_json muss ein JSON-Array sein: {}", e))?;
  state
    .upsert_rpc_pipeline(&id, &name, description.as_deref(), &steps_json, zero_context.unwrap_or(false))
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn pipeline_list(state: tauri::State<'_, Arc<Database>>) -> Result<Vec<db::RpcPipelineRow>, String> {
  state.list_rpc_pipelines().map_err(|e| e.to_string())
}

#[tauri::command]
fn pipeline_delete(state: tauri::State<'_, Arc<Database>>, id: String) -> Result<(), String> {
  state.delete_rpc_pipeline(&id).map_err(|e| e.to_string())
}

// -- Memory Provider commands -----------------------------------------------

#[tauri::command]
fn memory_provider_upsert(
  state: tauri::State<'_, Arc<Database>>,
  id: String,
  name: String,
  provider_type: String,
  config_json: String,
  enabled: Option<bool>,
) -> Result<(), String> {
  match provider_type.as_str() {
    "mem0" | "honcho" | "supermemory" | "custom" => {}
    other => return Err(format!("Unbekannter Provider-Typ: {}", other)),
  }
  state
    .upsert_memory_provider(&id, &name, &provider_type, &config_json, enabled.unwrap_or(true))
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn memory_provider_list(state: tauri::State<'_, Arc<Database>>) -> Result<Vec<db::MemoryProviderRow>, String> {
  state.list_memory_providers().map_err(|e| e.to_string())
}

#[tauri::command]
fn memory_provider_delete(state: tauri::State<'_, Arc<Database>>, id: String) -> Result<(), String> {
  state.delete_memory_provider(&id).map_err(|e| e.to_string())
}

// -- Tool Gateway commands --------------------------------------------------

#[tauri::command]
fn tool_gateway_upsert(
  state: tauri::State<'_, Arc<Database>>,
  id: String,
  tool_type: String,
  name: String,
  config_json: String,
  enabled: Option<bool>,
) -> Result<(), String> {
  match tool_type.as_str() {
    "web_search" | "image_gen" | "tts" | "browser" | "code_exec" | "custom" => {}
    other => return Err(format!("Unbekannter Tool-Typ: {}", other)),
  }
  state
    .upsert_tool_gateway_entry(&id, &tool_type, &name, &config_json, enabled.unwrap_or(true))
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn tool_gateway_list(state: tauri::State<'_, Arc<Database>>) -> Result<Vec<db::ToolGatewayRow>, String> {
  state.list_tool_gateway_entries().map_err(|e| e.to_string())
}

#[tauri::command]
fn tool_gateway_delete(state: tauri::State<'_, Arc<Database>>, id: String) -> Result<(), String> {
  state.delete_tool_gateway_entry(&id).map_err(|e| e.to_string())
}

// -- App entry --------------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  tauri::Builder::default()
    .plugin(tauri_plugin_dialog::init())
    .plugin(tauri_plugin_notification::init())
    .plugin(tauri_plugin_opener::init())
    .plugin(
      tauri_plugin_log::Builder::default()
        .level(log::LevelFilter::Info)
        .build(),
    )
    .setup(|app| {
      let app_data_dir = app
        .path()
        .app_data_dir()
        .expect("failed to resolve app data dir");
      let panic_log_dir = app_data_dir.clone();
      std::panic::set_hook(Box::new(move |panic_info| {
        let payload = if let Some(message) = panic_info.payload().downcast_ref::<&str>() {
          (*message).to_string()
        } else if let Some(message) = panic_info.payload().downcast_ref::<String>() {
          message.clone()
        } else {
          "unknown panic payload".to_string()
        };

        let location = panic_info
          .location()
          .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()));

        let details = serde_json::json!({
          "payload": payload,
          "location": location,
          "thread": std::thread::current().name().map(|name| name.to_string()),
        });

        let _ = audit::append_audit_event(
          panic_log_dir.clone(),
          "runtime",
          "backend_panic",
          Some(details),
        );
      }));

      let database = Database::open(app_data_dir)
        .expect("failed to open database");
      let shared_database = Arc::new(database);
      start_scheduler_worker(app.handle().clone(), shared_database.clone());
      app.manage(shared_database);
      app.manage(WatchRegistry::default());
      app.manage(CrewExecutionRegistry::default());
      app.manage(CrewPythonBridge::default());
      app.manage(ClaudeCodeBridge::new());
      configure_pdfium_search_paths(app.handle());

      Ok(())
    })
    .invoke_handler(tauri::generate_handler![
      ollama_health_check,
      generate_plan,
      chat_turn,
      chat_turn_stream,
      // Claude Code Bridge
      claude_code_start,
      claude_code_stop,
      claude_code_status,
      claude_code_send,
      claude_code_send_stream,
      claude_code_list_commands,
      claude_code_list_tools,
      desktop_primary_display,
      desktop_capture_primary_screenshot,
      desktop_capture_primary_annotated_screenshot,
      desktop_list_windows,
      desktop_focus_window,
      desktop_launch_app,
      desktop_click,
      desktop_move_mouse,
      desktop_type_text,
      desktop_keypress,
      desktop_scroll,
      mcp_runtime_start,
      mcp_runtime_stop,
      mcp_runtime_restart,
      mcp_runtime_list,
      mcp_probe,
      mcp_call_tool,
      web_fetch_url,
      web_search,
      exec_command,
      db_save_thread,
      db_list_threads,
      db_update_thread_provider_settings,
      db_delete_thread,
      db_save_message,
      db_update_message_content,
      db_delete_messages,
      db_list_messages,
      db_save_task,
      db_update_task_status,
      db_list_tasks,
      db_save_step,
      db_update_step,
      db_list_steps,
      execute_task,
      audit_event,
      fs_list_allowed_folders,
      fs_add_allowed_folder,
      fs_remove_allowed_folder,
      fs_import_attachment,
      fs_collect_attachment_metadata,
      fs_write_text_file,
      fs_create_directory,
      fs_move_path,
      fs_copy_path,
      fs_delete_file,
      fs_list_backups,
      fs_restore_backup,
      fs_watch_list,
      fs_watch_start,
      fs_watch_stop,
      fs_parse_artifact,
      fs_extract_text,
      fs_extract_text_limited,
      fs_save_artifact_version,
      fs_list_artifact_versions,
      fs_export_artifact_version,
      fs_list_artifact_exports,
      task_run_sub_agents,
      fs_generate_pro_outputs,
      fs_generate_office_workflow,
      scheduler_upsert_task,
      scheduler_list_tasks,
      scheduler_delete_task,
      scheduler_set_task_active,
      scheduler_run_task_now,
      scheduler_list_runs,
      crew_definition_upsert,
      crew_definition_list,
      crew_definition_versions_list,
      crew_role_binding_upsert,
      crew_role_binding_list,
      crew_approval_create,
      crew_approval_resolve,
      crew_approval_list,
      crew_runs_list,
      crew_run_events_list,
      crew_run_logs_list,
      crew_run_snapshot_get,
      crew_run_replay,
      export_save_text_file,
      policy_get,
      policy_set,
      policy_evaluate,
      engine_run_create,
      engine_run_update,
      engine_run_get,
      engine_run_list,
      engine_run_cancel,
      engine_run_resume,
      engine_run_retry,
      engine_run_checkpoint_add,
      engine_run_checkpoint_list,
      runtime_instruction_upsert,
      runtime_instruction_delete,
      runtime_instruction_list,
      runtime_instruction_effective,
      worker_sandbox_create,
      worker_sandbox_get,
      worker_sandbox_get_for_run,
      worker_sandbox_list,
      worker_sandbox_update,
      worker_sandbox_destroy,
      // Memory
      memory_upsert,
      memory_delete,
      memory_search,
      memory_compact,
      memory_snapshot,
      memory_hints,
      // User profile
      user_profile_upsert,
      user_profile_list,
      user_profile_delete,
      // Skills
      skill_upsert,
      skill_list,
      skill_delete,
      skill_record_usage,
      skill_improve,
      skill_match,
      skill_auto_generate,
      // Sessions
      session_create,
      session_end,
      session_list,
      session_search,
      session_freeze_snapshot,
      session_get,
      session_delete,
      // Learning
      learning_upsert,
      learning_list,
      // Terminal backends
      backend_upsert,
      backend_list,
      backend_delete,
      backend_exec,
      backend_ensure_local,
      // Process manager
      process_start,
      process_stop,
      process_approve,
      process_list,
      // Personalities
      personality_upsert,
      personality_list,
      personality_delete,
      // Insights
      insights_record,
      insights_list,
      insights_summary,
      // RPC Pipelines
      pipeline_upsert,
      pipeline_list,
      pipeline_delete,
      pipeline_execute,
      crew_execute,
      crew_stop,
      crew_runtime_status,
      crew_runtime_bootstrap,
      crew_runtime_validate_definition,
      // Memory providers
      memory_provider_upsert,
      memory_provider_list,
      memory_provider_delete,
      // Tool gateway
      tool_gateway_upsert,
      tool_gateway_list,
      tool_gateway_delete,
      connector_test_reachability,
      crew_provider_health_check,
      crew_provider_models_list,
    ])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
