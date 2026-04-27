mod artifact_pipeline;
mod audit;
mod claude_code_bridge;
mod cowork_features;
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
use db::Database;
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
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PolicyStatePayload {
  flags: PolicyFlagsPayload,
  deny_rules: Vec<String>,
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadRow {
  id: String,
  title: String,
  created_at: String,
  updated_at: String,
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

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct CrewExecuteAgentRequest {
  id: String,
  name: String,
  role: String,
  goal: String,
  backstory: String,
  personality_id: Option<String>,
  model_override: Option<String>,
  tools: Vec<String>,
  allow_delegation: bool,
  verbose: bool,
  max_iterations: i32,
}

#[derive(Debug, Deserialize, Clone)]
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

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct CrewExecuteRequest {
  id: String,
  name: String,
  description: String,
  process: String,
  manager_agent_id: Option<String>,
  verbose: bool,
  max_rpm: i32,
  agents: Vec<CrewExecuteAgentRequest>,
  tasks: Vec<CrewExecuteTaskRequest>,
  config: Option<OllamaConfig>,
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

fn build_crew_system_prompt(request: &CrewExecuteRequest, agent: &CrewExecuteAgentRequest) -> String {
  let manager_hint = request.manager_agent_id.as_ref().map_or(String::new(), |manager_id| {
    if manager_id == &agent.id {
      "Du bist zusaetzlich der koordinierende Manager-Agent dieser Crew.".to_string()
    } else {
      format!("Der koordinierende Manager-Agent hat die ID {}.", manager_id)
    }
  });

  format!(
    "Du arbeitest als Agent in der Crew \"{}\".\nBeschreibung: {}\nProzess: {}\n{}\n\nAgent:\n- Name: {}\n- Rolle: {}\n- Ziel: {}\n- Hintergrund: {}\n- Tools: {}\n- Delegation erlaubt: {}\n- Verbose: {}\n- Max Iterationen: {}\n- Max RPM der Crew: {}\n\nLiefere eine direkte, umsetzbare Antwort auf Deutsch.",
    request.name,
    request.description,
    request.process,
    manager_hint,
    agent.name,
    agent.role,
    agent.goal,
    agent.backstory,
    agent.tools.join(", "),
    agent.allow_delegation,
    agent.verbose,
    agent.max_iterations,
    request.max_rpm,
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

#[tauri::command]
async fn crew_execute(
  registry: tauri::State<'_, CrewExecutionRegistry>,
  request: CrewExecuteRequest,
) -> Result<CrewExecutionResponse, String> {
  if request.tasks.is_empty() {
    return Err("Crew enthaelt keine Tasks".to_string());
  }

  if let Ok(mut canceled) = registry.canceled.lock() {
    canceled.remove(&request.id);
  }

  let mut task_outputs: HashMap<String, String> = HashMap::new();
  let mut task_results = Vec::with_capacity(request.tasks.len());
  let mut logs = Vec::new();
  let mut overall_status = "completed".to_string();
  let mut overall_error: Option<String> = None;

  for task in &request.tasks {
    if is_crew_canceled(&registry, &request.id) {
      overall_status = "canceled".to_string();
      overall_error = Some("Crew-Ausfuehrung abgebrochen".to_string());
      break;
    }

    let Some(agent) = request.agents.iter().find(|entry| entry.id == task.agent_id) else {
      let error = format!("Agent {} fuer Task {} nicht gefunden", task.agent_id, task.id);
      task_results.push(CrewTaskExecutionRow {
        task_id: task.id.clone(),
        agent_id: task.agent_id.clone(),
        status: "failed".to_string(),
        output: Some(error.clone()),
      });
      overall_status = "failed".to_string();
      overall_error = Some(error);
      continue;
    };

    logs.push(CrewExecutionLogRow {
      id: uuid::Uuid::new_v4().to_string(),
      crew_id: request.id.clone(),
      agent_id: agent.id.clone(),
      task_id: task.id.clone(),
      action: format!("Task gestartet: {}", task.description.chars().take(80).collect::<String>()),
      result: format!("Agent: {}", agent.name),
      timestamp: chrono::Utc::now().timestamp_millis(),
    });

    let mut history = vec![ChatMessage {
      role: "system".to_string(),
      content: build_crew_system_prompt(&request, agent),
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
          content: format!("Ergebnis aus {}:\n{}", dependency, output),
        });
      }
    }

    let prompt = format!(
      "Crew: {}\nTask-ID: {}\nBeschreibung: {}\nErwartetes Ergebnis: {}\nAsynchrone Ausfuehrung erlaubt: {}\n\nLiefere das Task-Ergebnis direkt.",
      request.name,
      task.id,
      task.description,
      if task.expected_output.trim().is_empty() { "(nicht angegeben)" } else { &task.expected_output },
      task.async_execution,
    );

    let mut task_config = request.config.clone();
    if let Some(model_override) = agent.model_override.clone() {
      let mut config = task_config.unwrap_or_default();
      config.model = model_override;
      task_config = Some(config);
    }

    match chat_turn_internal(task_config, prompt, history, vec![]).await {
      Ok(response) => {
        let output = response.assistant_message;
        task_outputs.insert(task.id.clone(), output.clone());
        task_results.push(CrewTaskExecutionRow {
          task_id: task.id.clone(),
          agent_id: agent.id.clone(),
          status: "completed".to_string(),
          output: Some(output.clone()),
        });
        logs.push(CrewExecutionLogRow {
          id: uuid::Uuid::new_v4().to_string(),
          crew_id: request.id.clone(),
          agent_id: agent.id.clone(),
          task_id: task.id.clone(),
          action: "Task abgeschlossen".to_string(),
          result: output.chars().take(200).collect(),
          timestamp: chrono::Utc::now().timestamp_millis(),
        });
      }
      Err(error) => {
        let error_text = map_ollama_error(error);
        task_results.push(CrewTaskExecutionRow {
          task_id: task.id.clone(),
          agent_id: agent.id.clone(),
          status: "failed".to_string(),
          output: Some(error_text.clone()),
        });
        logs.push(CrewExecutionLogRow {
          id: uuid::Uuid::new_v4().to_string(),
          crew_id: request.id.clone(),
          agent_id: agent.id.clone(),
          task_id: task.id.clone(),
          action: "Task fehlgeschlagen".to_string(),
          result: error_text.chars().take(200).collect(),
          timestamp: chrono::Utc::now().timestamp_millis(),
        });
        overall_status = "failed".to_string();
        overall_error = Some(error_text);
      }
    }
  }

  if let Ok(mut canceled) = registry.canceled.lock() {
    canceled.remove(&request.id);
  }

  Ok(CrewExecutionResponse {
    crew_id: request.id,
    status: overall_status,
    task_results,
    logs,
    error: overall_error,
  })
}

#[tauri::command]
fn crew_stop(
  registry: tauri::State<'_, CrewExecutionRegistry>,
  request: CrewStopRequest,
) -> Result<(), String> {
  let mut canceled = registry.canceled.lock().map_err(|_| "Crew-Registry gesperrt".to_string())?;
  canceled.insert(request.crew_id);
  Ok(())
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
fn db_save_thread(state: tauri::State<'_, Arc<Database>>, id: String, title: String, created_at: String) -> Result<(), String> {
  state.insert_thread(&id, &title, &created_at).map_err(|e| e.to_string())
}

#[tauri::command]
fn db_list_threads(state: tauri::State<'_, Arc<Database>>) -> Result<Vec<ThreadRow>, String> {
  state.list_threads().map_err(|e| e.to_string()).map(|rows| {
    rows.into_iter().map(|(id, title, ca, ua)| ThreadRow { id, title, created_at: ca, updated_at: ua }).collect()
  })
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
  row: (String, String, String, String, bool, Option<String>, Option<String>, String, String),
) -> ScheduledTaskRow {
  let (id, name, prompt, schedule_expr, active, last_run_at, next_run_at, created_at, updated_at) = row;
  ScheduledTaskRow {
    id,
    name,
    prompt,
    schedule_expr,
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
) {
  let started_at = chrono::Utc::now().to_rfc3339();
  let run_id = uuid::Uuid::new_v4().to_string();
  let plan_result = tauri::async_runtime::block_on(generate_plan_internal(None, task_prompt.to_string()));
  let finished_at = chrono::Utc::now().to_rfc3339();

  let next_run_at = scheduler::next_run_from_expression(schedule_expr, chrono::Utc::now())
    .ok()
    .map(|next| next.to_rfc3339());

  match plan_result {
    Ok(plan) => {
      let result_json = serde_json::to_string(&plan).unwrap_or_else(|_| String::new());
      let _ = database.insert_scheduled_run(
        &run_id,
        task_id,
        "succeeded",
        &started_at,
        Some(&finished_at),
        Some(&result_json),
        None,
      );
      let _ = database.update_scheduled_task_runtime(task_id, Some(&finished_at), next_run_at.as_deref());

      if let Ok(app_data_dir) = app.path().app_data_dir() {
        let details = serde_json::json!({
          "taskId": task_id,
          "taskName": task_name,
          "runId": run_id,
          "status": "succeeded",
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
          "status": "failed",
          "error": error_text,
        });
        let _ = audit::append_audit_event(app_data_dir, "scheduler", "task_run_completed", Some(details));
      }
    }
  }
}

fn start_scheduler_worker(app: tauri::AppHandle, database: Arc<Database>) {
  std::thread::spawn(move || loop {
    let now = chrono::Utc::now().to_rfc3339();
    if let Ok(due_tasks) = database.list_due_scheduled_tasks(&now) {
      for (task_id, task_name, task_prompt, schedule_expr, _) in due_tasks {
        run_scheduled_task_once(&app, &database, &task_id, &task_name, &task_prompt, &schedule_expr);
      }
    }

    std::thread::sleep(Duration::from_secs(30));
  });
}

#[tauri::command]
fn scheduler_upsert_task(
  state: tauri::State<'_, Arc<Database>>,
  request: ScheduledTaskUpsertRequest,
) -> Result<ScheduledTaskRow, String> {
  let now = chrono::Utc::now();
  let now_text = now.to_rfc3339();
  let existing_task = state
    .list_scheduled_tasks()
    .map_err(|err| err.to_string())?
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

  let last_run_at = existing_task.and_then(|row| row.5);

  state
    .upsert_scheduled_task(
      &request.id,
      &request.name,
      &request.prompt,
      &request.schedule_expr,
      request.active,
      last_run_at.as_deref(),
      next_run_at.as_deref(),
      &now_text,
    )
    .map_err(|err| err.to_string())?;

  state
    .list_scheduled_tasks()
    .map_err(|err| err.to_string())?
    .into_iter()
    .find(|row| row.0 == request.id)
    .map(map_scheduled_task_row)
    .ok_or_else(|| "scheduled task not found after upsert".to_string())
}

#[tauri::command]
fn scheduler_list_tasks(state: tauri::State<'_, Arc<Database>>) -> Result<Vec<ScheduledTaskRow>, String> {
  state
    .list_scheduled_tasks()
    .map_err(|err| err.to_string())
    .map(|rows| rows.into_iter().map(map_scheduled_task_row).collect())
}

#[tauri::command]
fn scheduler_delete_task(state: tauri::State<'_, Arc<Database>>, id: String) -> Result<(), String> {
  state.delete_scheduled_task(&id).map_err(|err| err.to_string())
}

#[tauri::command]
fn scheduler_set_task_active(
  state: tauri::State<'_, Arc<Database>>,
  request: ScheduledTaskToggleRequest,
) -> Result<(), String> {
  let task_row = state
    .list_scheduled_tasks()
    .map_err(|err| err.to_string())?
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

  state
    .set_scheduled_task_active(&request.id, request.active, next_run_at.as_deref())
    .map_err(|err| err.to_string())
}

#[tauri::command]
fn scheduler_run_task_now(
  app: tauri::AppHandle,
  state: tauri::State<'_, Arc<Database>>,
  id: String,
) -> Result<(), String> {
  let task_row = state
    .list_scheduled_tasks()
    .map_err(|err| err.to_string())?
    .into_iter()
    .find(|row| row.0 == id)
    .ok_or_else(|| "scheduled task not found".to_string())?;

  let database = state.inner().clone();
  run_scheduled_task_once(&app, &database, &task_row.0, &task_row.1, &task_row.2, &task_row.3);
  Ok(())
}

#[tauri::command]
fn scheduler_list_runs(
  state: tauri::State<'_, Arc<Database>>,
  limit: Option<u32>,
) -> Result<Vec<ScheduledRunRow>, String> {
  let bounded_limit = limit.unwrap_or(30).clamp(1, 200) as i64;
  state
    .list_scheduled_runs(bounded_limit)
    .map_err(|err| err.to_string())
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
        .collect()
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

fn enforce_tool_policy(
  policy: &PolicyStatePayload,
  tool: &str,
  target: &str,
  tool_allowed_by_flag: bool,
) -> Result<(), String> {
  if !policy.flags.strict_policy_enforcement {
    return Ok(());
  }

  if !tool_allowed_by_flag {
    return Err(format!("policy blockiert {}", tool));
  }

  if policy
    .deny_rules
    .iter()
    .any(|rule| matches_deny_rule(rule, tool, target))
  {
    return Err(format!("deny rule blockiert {}:{}", tool, target));
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

  Ok(PolicyStatePayload { flags, deny_rules })
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
      // Memory providers
      memory_provider_upsert,
      memory_provider_list,
      memory_provider_delete,
      // Tool gateway
      tool_gateway_upsert,
      tool_gateway_list,
      tool_gateway_delete,
      connector_test_reachability,
    ])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
