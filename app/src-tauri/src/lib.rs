#![allow(clippy::too_many_arguments, clippy::type_complexity)]

mod artifact_pipeline;
mod audit;
mod audit_service;
mod audit_sink;
mod capability_model;
mod claude_code_bridge;
mod context;
mod cowork_features;
mod credential_store;
mod crew_python_bridge;
mod db;
mod event_sink;
mod file_safety;
mod file_watch;
mod insights;
mod mcp;
mod memory_engine;
mod network_safety;
mod office_integration;
mod ollama;
mod process_control;
mod process_manager;
mod scheduler;
mod secure_config;
mod sensitive_data;
mod service_error;
mod skill_engine;
mod support_bundle;
mod terminal_backends;
mod terminal_sessions;
#[cfg(test)]
mod test_fixtures;
mod worker_sandbox;

use claude_code_bridge::ClaudeCodeBridge;
use crew_python_bridge::{
    crew_runtime_bootstrap, crew_runtime_execute_request, crew_runtime_status,
    crew_runtime_validate_definition, CrewPythonBridge, CrewRuntimeExecutionLog,
};
use db::Database;
use mcp::{
    call_tool, probe_server, runtime_call_tool, runtime_has_server, runtime_list_servers,
    runtime_probe_server, runtime_restart_server, runtime_start_server, runtime_stop_server,
    McpCallRequest, McpError, McpRuntimeServerStatus, McpServerRequest,
};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use ollama::{
    chat_turn as chat_turn_internal, chat_turn_stream as chat_turn_stream_internal, check_health,
    generate_plan as generate_plan_internal, ChatMessage, ChatStreamChunkPayload, ChatToolDef,
    OllamaConfig, OllamaError,
};
use reqwest::{Method, StatusCode};
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
use std::time::Duration;
use std::time::Instant;
use tauri::{Emitter, Manager};
use terminal_sessions::TerminalSessionRegistry;
use url::Url;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[cfg(target_os = "windows")]
fn suppress_command_window(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
fn suppress_command_window(_command: &mut Command) {}

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
const POLICY_SETTING_ACTIVE_TOOLSET: &str = "activeToolsetPolicyId";
const CUSTOM_TOOLSET_POLICY_ID: &str = "custom";
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
    "office_workflow",
    "todo",
    "delegate_task",
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

#[derive(Default)]
struct ChatStreamRegistry {
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
    #[serde(default)]
    active_toolset_policy_id: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ToolsetPolicyPayload {
    id: String,
    label: String,
    description: String,
    risk_level: String,
    tool_ids: Vec<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PolicyStatePayload {
    flags: PolicyFlagsPayload,
    deny_rules: Vec<String>,
    enabled_tool_ids: Vec<String>,
    active_toolset_policy_id: String,
    toolset_policies: Vec<ToolsetPolicyPayload>,
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
    source: Option<String>,
    status: Option<String>,
    phase: Option<String>,
    cwd: Option<String>,
    workspace_path: Option<String>,
    model: Option<String>,
    provider: Option<String>,
    provider_profile_id: Option<String>,
    runtime_mode: Option<String>,
    toolset_policy_id: Option<String>,
    channel_kind: Option<String>,
    channel_ref: Option<String>,
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
struct EngineRunEventAppendRequest {
    run_id: String,
    event_type: String,
    summary: Option<String>,
    payload_json: Option<String>,
    redaction_level: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EngineRunArtifactAddRequest {
    run_id: String,
    kind: String,
    path: String,
    title: Option<String>,
    summary: Option<String>,
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
    #[serde(default)]
    model: Option<String>,
    #[serde(default = "default_true")]
    verify_tls_certificates: bool,
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
    #[serde(default)]
    model: Option<String>,
    #[serde(default = "default_true")]
    verify_tls_certificates: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenAiCompatibleChatCompletionRequest {
    endpoint: String,
    headers: HashMap<String, String>,
    body: String,
    timeout_ms: Option<u64>,
    #[serde(default = "default_true")]
    verify_tls_certificates: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenAiCompatibleChatCompletionResponse {
    status: u16,
    body: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CrewProviderModelsResponse {
    endpoint: String,
    models: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct GatewayHealthRequest {
    #[serde(default)]
    include_provider_probe: bool,
    #[serde(default)]
    provider_kind: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default = "default_true")]
    verify_tls_certificates: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GatewayProbeRequest {
    subsystem: String,
    #[serde(default)]
    provider: Option<GatewayHealthRequest>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct GatewaySubsystemPayload {
    id: String,
    label: String,
    category: String,
    status: String,
    message: String,
    checked_at: String,
    detail_json: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct GatewayHealthPayload {
    status: String,
    checked_at: String,
    subsystems: Vec<GatewaySubsystemPayload>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeProviderMappingRequest {
    base_url: String,
    #[serde(default)]
    runtime_mode: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RuntimeProviderMappingResponse {
    input_url: String,
    mapped_url: String,
    runtime_mode: String,
    changed: bool,
    reason: String,
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

fn normalize_openai_model_rows(rows: Vec<OpenAiModelRow>) -> Vec<String> {
    let mut models = rows
        .into_iter()
        .filter_map(|entry| {
            let id = entry
                .id
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            let name = entry
                .name
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            id.or(name)
        })
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    models
}

fn parse_openai_models_response(body: &str) -> Result<Vec<String>, String> {
    serde_json::from_str::<OpenAiModelsResponse>(body)
        .map(|payload| normalize_openai_model_rows(payload.data))
        .map_err(|error| format!("Model list could not be read: {}", error))
}

fn model_name_suffix(value: &str) -> &str {
    let trimmed = value.trim();
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

fn find_model_suggestion<'a>(models: &'a [String], configured_model: &str) -> Option<&'a str> {
    let configured = configured_model.trim();
    if configured.is_empty() {
        return None;
    }

    let lower_configured = configured.to_lowercase();
    models
        .iter()
        .find(|model| model.to_lowercase() == lower_configured)
        .or_else(|| {
            models
                .iter()
                .find(|model| model_name_suffix(model).to_lowercase() == lower_configured)
        })
        .or_else(|| {
            models
                .iter()
                .find(|model| models.len() == 1 && !model.is_empty())
        })
        .map(|model| model.as_str())
}

fn format_model_sample(models: &[String]) -> String {
    models
        .iter()
        .take(5)
        .map(|model| format!("'{}'", model))
        .collect::<Vec<_>>()
        .join(", ")
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
struct ProjectResourceRecord {
    id: String,
    project_id: String,
    kind: String,
    path: String,
    label: Option<String>,
    enabled: bool,
    added_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectRecord {
    id: String,
    title: String,
    instructions: String,
    resources: Vec<ProjectResourceRecord>,
    thread_ids: Vec<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectUpsertRequest {
    id: String,
    title: String,
    instructions: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectResourceUpsertRequest {
    id: String,
    project_id: String,
    kind: String,
    path: String,
    label: Option<String>,
    enabled: Option<bool>,
    added_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectDeleteResponse {
    deleted_thread_ids: Vec<String>,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkTaskUpsertRequest {
    id: String,
    title: String,
    prompt: String,
    expected_output: Option<String>,
    work_dir: Option<String>,
    thread_id: Option<String>,
    runner: String,
    crew_id: Option<String>,
    model: Option<String>,
    schedule_expr: Option<String>,
    schedule_enabled: Option<bool>,
    status: Option<String>,
    output: Option<String>,
    error: Option<String>,
    last_run_at: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkTaskStatusUpdateRequest {
    id: String,
    status: String,
    output: Option<String>,
    error: Option<String>,
    last_run_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkTaskRecord {
    id: String,
    title: String,
    prompt: String,
    expected_output: String,
    work_dir: String,
    thread_id: Option<String>,
    runner: String,
    crew_id: Option<String>,
    model: String,
    schedule_expr: String,
    schedule_enabled: bool,
    status: String,
    output: Option<String>,
    error: Option<String>,
    last_run_at: Option<String>,
    created_at: String,
    updated_at: String,
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
    execution_subject: Option<String>,
    #[serde(default)]
    execution_guidelines: String,
    #[serde(default)]
    knowledge_focus: String,
    #[serde(default = "default_crew_governance_mode")]
    governance_mode: String,
    #[serde(default)]
    governance_resume_approval_id: Option<String>,
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
    #[serde(default)]
    stream_id: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct CrewGovernanceAgentAccessPayload {
    agent_id: String,
    allowed_tools: Vec<String>,
    blocked_tools: Vec<String>,
    allowed_mcp_server_names: Vec<String>,
    blocked_mcp_server_names: Vec<String>,
    delegation_allowed: bool,
    gateway_hints: Vec<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct CrewGovernancePayload {
    subject: String,
    subject_roles: Vec<String>,
    policy_strict: bool,
    pending_approval_types: Vec<String>,
    agent_access: Vec<CrewGovernanceAgentAccessPayload>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct QueuedCrewApprovalPayload {
    request: CrewExecuteRequest,
    reason: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct CrewMemoryEntryPayload {
    id: String,
    scope: String,
    category: String,
    key: String,
    content: String,
    confidence: f64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct CrewUserProfilePayload {
    key: String,
    value: String,
    confidence: f64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct CrewMemoryPayload {
    query: String,
    summary: String,
    entries: Vec<CrewMemoryEntryPayload>,
    user_profile: Vec<CrewUserProfilePayload>,
    hints: Vec<String>,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CrewStopRequest {
    crew_id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct CrewExecutionLogRow {
    id: String,
    crew_id: String,
    agent_id: String,
    task_id: String,
    action: String,
    result: String,
    timestamp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    severity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_reasoning: Option<String>,
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

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct CrewExecutionLogEventPayload {
    stream_id: Option<String>,
    run_id: Option<String>,
    log: CrewExecutionLogRow,
}

fn runtime_log_to_row(entry: CrewRuntimeExecutionLog) -> CrewExecutionLogRow {
    CrewExecutionLogRow {
        id: entry.id,
        crew_id: entry.crew_id,
        agent_id: entry.agent_id,
        task_id: entry.task_id,
        action: entry.action,
        result: entry.result,
        timestamp: entry.timestamp,
        agent_name: entry.agent_name,
        source_agent: entry.source_agent,
        target_agent: entry.target_agent,
        provider: entry.provider,
        model: entry.model,
        task_title: entry.task_title,
        phase: entry.phase,
        summary: entry.summary,
        detail: entry.detail,
        severity: entry.severity,
        provider_reasoning: entry.provider_reasoning,
    }
}

fn emit_crew_execution_log_event(
    app: &tauri::AppHandle,
    stream_id: Option<String>,
    run_id: Option<String>,
    log: CrewExecutionLogRow,
) {
    let payload = CrewExecutionLogEventPayload {
        stream_id,
        run_id,
        log,
    };
    let _ = app.emit("crew-execution-log", payload);
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

fn default_crew_governance_mode() -> String {
    "allow-all".to_string()
}

fn default_manager_review_enabled() -> bool {
    true
}

fn default_share_all_task_outputs() -> bool {
    true
}

fn default_crew_execution_subject() -> String {
    "workspace-user".to_string()
}

fn crew_agent_can_delegate(request: &CrewExecuteRequest, agent: &CrewExecuteAgentRequest) -> bool {
    agent.allow_delegation
        || (request.process.eq_ignore_ascii_case("hierarchical")
            && request.manager_agent_id.as_deref() == Some(agent.id.as_str()))
}

fn build_effective_crew_tool_ids(
    request: &CrewExecuteRequest,
    agent: &CrewExecuteAgentRequest,
) -> Vec<String> {
    let mut tool_ids = normalize_policy_enabled_tool_ids(&agent.tools);
    if crew_agent_can_delegate(request, agent)
        && !tool_ids.iter().any(|tool_id| tool_id == "delegate_task")
    {
        tool_ids.push("delegate_task".to_string());
    }
    tool_ids
}

fn truncate_chars_with_ellipsis(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{}...", truncated)
    } else {
        truncated
    }
}

fn normalize_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn dedupe_strings(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();

    for value in values {
        let normalized = value.trim().to_string();
        if normalized.is_empty() {
            continue;
        }
        if seen.insert(normalized.clone()) {
            ordered.push(normalized);
        }
    }

    ordered
}

fn normalize_crew_governance_mode(mode: &str) -> &'static str {
    match mode.trim().to_ascii_lowercase().as_str() {
        "ask-all" | "always-ask" | "always_ask" | "alwaysask" => "ask-all",
        "ask-risky" | "ask_risky" | "risky" | "risky-only" | "risky_only" => "ask-risky",
        "read-only" | "read_only" | "readonly" => "read-only",
        _ => "allow-all",
    }
}

fn crew_tool_is_read_only(tool_id: &str) -> bool {
    matches!(
        canonical_policy_tool_id(tool_id).as_str(),
        "read_file" | "glob" | "grep" | "web_fetch" | "web_search" | "todo"
    )
}

fn queue_crew_governance_approval(
    database: &Arc<Database>,
    request: &CrewExecuteRequest,
    approval_type: &str,
    reason: &str,
) -> Result<String, String> {
    let pending_approval = database
        .list_crew_approvals(Some("pending"), Some(&request.id))
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|approval| approval.approval_type.eq_ignore_ascii_case(approval_type));

    if let Some(approval) = pending_approval {
        return Ok(format!(
            "Crew is waiting for approval: {} ({})",
            reason, approval.id,
        ));
    }

    let approval_id = uuid::Uuid::new_v4().to_string();
    let payload_json = serde_json::to_string(&QueuedCrewApprovalPayload {
        request: request.clone(),
        reason: reason.to_string(),
    })
    .map_err(|error| error.to_string())?;

    database
        .insert_crew_approval(
            &approval_id,
            Some(&request.id),
            None,
            approval_type,
            Some(&request.id),
            "pending",
            Some("governance-auto"),
            Some(&payload_json),
        )
        .map_err(|error| error.to_string())?;

    Ok(format!(
        "Crew is waiting for approval: {} ({})",
        reason, approval_id,
    ))
}

fn resolve_crew_execution_subject(request: &CrewExecuteRequest) -> String {
    request
        .execution_subject
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
        .unwrap_or_else(default_crew_execution_subject)
}

fn subject_matches_role_binding(subject: &str, binding_subject: &str) -> bool {
    let normalized_subject = subject.trim();
    let normalized_binding = binding_subject.trim();

    !normalized_subject.is_empty()
        && !normalized_binding.is_empty()
        && (normalized_subject.eq_ignore_ascii_case(normalized_binding)
            || normalized_binding == "*"
            || normalized_binding.eq_ignore_ascii_case("everyone"))
}

fn crew_role_allows_execution(role: &str) -> bool {
    matches!(
        role.trim().to_ascii_lowercase().as_str(),
        "owner/admin"
            | "owner"
            | "admin"
            | "editor/designer"
            | "editor"
            | "designer"
            | "operator/runner"
            | "operator"
            | "runner"
    )
}

fn crew_role_allows_tool_operations(role: &str) -> bool {
    matches!(
        role.trim().to_ascii_lowercase().as_str(),
        "owner/admin" | "owner" | "admin" | "operator/runner" | "operator" | "runner"
    )
}

fn build_crew_memory_query(request: &CrewExecuteRequest) -> String {
    let mut parts = Vec::new();

    for candidate in [
        request.knowledge_focus.as_str(),
        request.name.as_str(),
        request.description.as_str(),
        request.execution_guidelines.as_str(),
    ] {
        let normalized = normalize_whitespace(candidate);
        if !normalized.is_empty() {
            parts.push(truncate_chars_with_ellipsis(&normalized, 220));
        }
    }

    for task in request.tasks.iter().take(3) {
        let normalized = normalize_whitespace(&task.description);
        if !normalized.is_empty() {
            parts.push(truncate_chars_with_ellipsis(&normalized, 180));
        }
    }

    dedupe_strings(parts).join(" | ")
}

fn collect_crew_memory_payload(
    database: &Arc<Database>,
    request: &CrewExecuteRequest,
) -> CrewMemoryPayload {
    let query = build_crew_memory_query(request);
    let mut entries = if query.is_empty() {
        Vec::new()
    } else {
        database
            .search_memory_entries(&query, None, None, 8)
            .unwrap_or_default()
    };

    if entries.is_empty() {
        entries = database
            .list_memory_entries("shared", None, 4)
            .or_else(|_| database.list_memory_entries("agent", None, 4))
            .unwrap_or_default();
    }

    for entry in &entries {
        let _ = database.touch_memory_entry(&entry.id);
    }

    let snapshot = memory_engine::create_memory_snapshot(database).ok();
    let user_profile = snapshot
        .as_ref()
        .map(|value| {
            value
                .user_profile
                .iter()
                .take(8)
                .map(|entry| CrewUserProfilePayload {
                    key: entry.key.clone(),
                    value: truncate_chars_with_ellipsis(&normalize_whitespace(&entry.value), 220),
                    confidence: entry.confidence,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let hints = memory_engine::generate_memory_hints(database)
        .into_iter()
        .map(|hint| truncate_chars_with_ellipsis(&normalize_whitespace(&hint.message), 180))
        .take(4)
        .collect::<Vec<_>>();

    let summary = if entries.is_empty() && user_profile.is_empty() {
        "No saved crew knowledge found. Work conservatively and mark assumptions explicitly."
            .to_string()
    } else {
        format!(
            "{} memory entries and {} profile notes are available as crew context. Use them as working hypotheses and verify disputed points.",
            entries.len(),
            user_profile.len(),
        )
    };

    CrewMemoryPayload {
        query,
        summary,
        entries: entries
            .into_iter()
            .map(|entry| CrewMemoryEntryPayload {
                id: entry.id,
                scope: entry.scope,
                category: entry.category,
                key: entry.key,
                content: truncate_chars_with_ellipsis(&normalize_whitespace(&entry.content), 420),
                confidence: entry.confidence,
            })
            .collect(),
        user_profile,
        hints,
    }
}

fn collect_crew_governance_payload(
    database: &Arc<Database>,
    request: &CrewExecuteRequest,
    enabled_agents: &HashMap<String, CrewExecuteAgentRequest>,
) -> Result<CrewGovernancePayload, String> {
    ensure_crew_run_is_approved(database, &request.id)?;
    let governance_mode = normalize_crew_governance_mode(&request.governance_mode);
    let resume_approval_active = request
        .governance_resume_approval_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some();

    let subject = resolve_crew_execution_subject(request);
    let role_bindings = database
        .list_crew_role_bindings(Some("crew"), None)
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|binding| {
            binding.scope_ref.as_deref().is_none()
                || binding.scope_ref.as_deref() == Some(request.id.as_str())
        })
        .collect::<Vec<_>>();
    let subject_roles = dedupe_strings(
        role_bindings
            .iter()
            .filter(|binding| subject_matches_role_binding(&subject, &binding.subject))
            .map(|binding| binding.role.clone()),
    );

    if !role_bindings.is_empty()
        && !subject_roles
            .iter()
            .any(|role| crew_role_allows_execution(role))
    {
        return Err(format!(
            "Crew start for subject '{}' blocked: no matching runner role is stored for crew {} hinterlegt.",
            subject, request.id,
        ));
    }

    let pending_approvals = database
        .list_crew_approvals(Some("pending"), Some(&request.id))
        .map_err(|error| error.to_string())?;
    let pending_approval_types = dedupe_strings(
        pending_approvals
            .iter()
            .map(|approval| approval.approval_type.clone()),
    );
    let policy = load_policy_state(database)?;
    let gateways = database.list_tool_gateway_entries().unwrap_or_default();
    let mcp_gateways_configured = gateways
        .iter()
        .any(|entry| entry.enabled && canonical_policy_tool_id(&entry.tool_type) == "mcp");

    let mut requested_live_tools = false;
    let mut requested_delegation = false;
    let mut requested_risky_actions = false;
    let mut capability_errors = Vec::new();
    let mut agent_access = Vec::new();

    for agent in enabled_agents.values() {
        let requested_tools = build_effective_crew_tool_ids(request, agent);
        let requested_delegation_for_agent = crew_agent_can_delegate(request, agent);
        if !agent.mcp_server_names.is_empty() {
            requested_risky_actions = true;
        }
        let mut allowed_tools = Vec::new();
        let mut blocked_tools = Vec::new();

        for tool_id in &requested_tools {
            let allowed = if tool_id == "delegate_task" {
                requested_delegation_for_agent
                    && (!policy.flags.strict_policy_enforcement
                        || policy
                            .enabled_tool_ids
                            .iter()
                            .any(|enabled| enabled == "delegate_task"))
            } else {
                crew_tool_allowed_by_flags(&policy, tool_id)
                    && (!policy.flags.strict_policy_enforcement
                        || policy
                            .enabled_tool_ids
                            .iter()
                            .any(|enabled| enabled == tool_id))
            };

            if tool_id != "delegate_task" {
                requested_live_tools = true;
            }

            if tool_id == "delegate_task" || !crew_tool_is_read_only(tool_id) {
                requested_risky_actions = true;
            }

            if allowed {
                allowed_tools.push(tool_id.clone());
            } else {
                blocked_tools.push(tool_id.clone());
            }
        }

        if requested_delegation_for_agent {
            requested_delegation = true;
        }

        let mcp_calls_allowed = crew_tool_allowed_by_flags(&policy, "mcp")
            && (!policy.flags.strict_policy_enforcement
                || policy
                    .enabled_tool_ids
                    .iter()
                    .any(|enabled| enabled == "mcp"));
        let mut allowed_mcp_server_names = Vec::new();
        let mut blocked_mcp_server_names = Vec::new();

        for server_name in &agent.mcp_server_names {
            let gateway_matches = gateways.iter().any(|entry| {
                entry.enabled
                    && canonical_policy_tool_id(&entry.tool_type) == "mcp"
                    && (entry.name.eq_ignore_ascii_case(server_name)
                        || entry.name.trim().is_empty()
                        || canonical_policy_tool_id(&entry.name) == "mcp")
            });
            let allowed = mcp_calls_allowed && (!mcp_gateways_configured || gateway_matches);
            if allowed {
                allowed_mcp_server_names.push(server_name.clone());
            } else {
                blocked_mcp_server_names.push(server_name.clone());
            }
        }

        let gateway_hints = dedupe_strings(
            requested_tools
                .iter()
                .chain(agent.mcp_server_names.iter())
                .filter_map(|tool_name| find_gateway_context(tool_name, &gateways)),
        );
        let delegation_allowed = requested_delegation_for_agent
            && !blocked_tools.iter().any(|tool| tool == "delegate_task");

        if !blocked_tools.is_empty() || !blocked_mcp_server_names.is_empty() {
            capability_errors.push(format!(
                "Agent '{}' hat blockierte Capabilities. Tools: [{}]; MCP: [{}].",
                agent.name,
                blocked_tools.join(", "),
                blocked_mcp_server_names.join(", "),
            ));
        }

        agent_access.push(CrewGovernanceAgentAccessPayload {
            agent_id: agent.id.clone(),
            allowed_tools,
            blocked_tools,
            allowed_mcp_server_names,
            blocked_mcp_server_names,
            delegation_allowed,
            gateway_hints,
        });
    }

    if (!subject_roles.is_empty() || !role_bindings.is_empty())
        && (requested_live_tools || requested_delegation)
        && !subject_roles
            .iter()
            .any(|role| crew_role_allows_tool_operations(role))
    {
        return Err(format!(
            "Crew start for subject '{}' blocked: live tools and delegation require owner/admin or operator/runner.",
            subject,
        ));
    }

    if governance_mode == "read-only" && requested_risky_actions {
        return Err(
      "Crew governance blocked: mode 'Read only' allows only read access (read_file, grep, glob, web_fetch, web_search, todo).".to_string()
    );
    }

    if !resume_approval_active {
        if governance_mode == "ask-all" {
            return Err(queue_crew_governance_approval(
                database,
                request,
                "run_gate",
                "Mode 'always ask before actions' requires approval before start.",
            )?);
        }

        if governance_mode == "ask-risky" && requested_risky_actions {
            return Err(queue_crew_governance_approval(
                database,
                request,
                "run_gate",
                "risky actions were detected and must be confirmed before start.",
            )?);
        }
    }

    if pending_approval_types
        .iter()
        .any(|entry| entry.eq_ignore_ascii_case("tool_gate"))
        && requested_live_tools
    {
        return Err(format!(
            "Crew start blocked: open tool_gate approvals for Crew {}.",
            request.id,
        ));
    }

    if pending_approval_types
        .iter()
        .any(|entry| entry.eq_ignore_ascii_case("delegation_gate"))
        && requested_delegation
    {
        return Err(format!(
            "Crew start blocked: open delegation_gate approvals for Crew {}.",
            request.id,
        ));
    }

    if !capability_errors.is_empty() {
        return Err(capability_errors.join("\n"));
    }

    Ok(CrewGovernancePayload {
        subject,
        subject_roles,
        policy_strict: policy.flags.strict_policy_enforcement,
        pending_approval_types,
        agent_access,
    })
}

fn persist_crew_run_memory_summary(
    database: &Arc<Database>,
    request: &CrewExecuteRequest,
    run_id: &str,
    response: &CrewExecutionResponse,
) {
    let task_lines = response
        .task_results
        .iter()
        .take(6)
        .map(|task| {
            let output = task
                .output
                .as_deref()
                .map(|value| truncate_chars_with_ellipsis(&normalize_whitespace(value), 220))
                .unwrap_or_else(|| "no output".to_string());
            format!("- {} [{}]: {}", task.task_id, task.status, output)
        })
        .collect::<Vec<_>>();

    let mut summary_lines = vec![
        format!("Crew: {} ({})", request.name, request.id),
        format!("Status: {}", response.status),
    ];

    if let Some(error) = response.error.as_deref() {
        summary_lines.push(format!(
            "Error: {}",
            truncate_chars_with_ellipsis(&normalize_whitespace(error), 280)
        ));
    }
    if !request.knowledge_focus.trim().is_empty() {
        summary_lines.push(format!(
            "Knowledge focus: {}",
            truncate_chars_with_ellipsis(&normalize_whitespace(&request.knowledge_focus), 180)
        ));
    }
    if !task_lines.is_empty() {
        summary_lines.push("Task outcomes:".to_string());
        summary_lines.extend(task_lines);
    }

    let summary = summary_lines.join("\n");
    let confidence = if response.status.eq_ignore_ascii_case("completed") {
        0.82
    } else if response.status.eq_ignore_ascii_case("failed") {
        0.45
    } else {
        0.6
    };

    for key in [
        format!("crew::{}::latest", request.id),
        format!("crew::{}::run::{}", request.id, run_id),
    ] {
        let _ = database.upsert_memory_entry(
            &uuid::Uuid::new_v4().to_string(),
            "shared",
            "crew-run",
            &key,
            &summary,
            Some(run_id),
            confidence,
        );
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
                && (entry.name.eq_ignore_ascii_case(tool_name)
                    || entry.tool_type.eq_ignore_ascii_case(tool_name))
        })
        .map(|entry| format!("Tool gateway: {} ({})", entry.name, entry.tool_type))
}

async fn execute_pipeline_web_fetch(url: &str) -> Result<String, String> {
    let requested_url = url.trim();
    if requested_url.is_empty() {
        return Err("web_fetch benoetigt eine URL".to_string());
    }

    let response =
        network_safety::fetch_public_text(requested_url, network_safety::MAX_TEXT_RESPONSE_BYTES)
            .await?;
    let title = extract_html_title(&response.body).unwrap_or_else(|| "(no title)".to_string());
    let stripped = strip_html_like_content(&response.body);
    let content: String = stripped.trim().chars().take(4_000).collect();

    Ok(format!(
        "URL: {}\nStatus: {}\nTitle: {}\n\n{}",
        response.final_url,
        response.status.as_u16(),
        title,
        content
    ))
}

async fn execute_pipeline_web_search(query: &str) -> Result<String, String> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Err("web_search requires a search query".to_string());
    }

    let encoded_query =
        url::form_urlencoded::byte_serialize(trimmed.as_bytes()).collect::<String>();
    let search_url = format!("https://html.duckduckgo.com/html/?q={}", encoded_query);
    let response =
        network_safety::fetch_public_text(&search_url, network_safety::MAX_TEXT_RESPONSE_BYTES)
            .await?;
    if !response.status.is_success() {
        return Err(format!(
            "web search returned HTTP {}",
            response.status.as_u16()
        ));
    }
    let results = parse_duckduckgo_results(&response.body, 5);

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
            Some(format!("Previous pipeline context:\n{}", previous_context))
        },
        gateway_context,
        Some(format!("Tool: {}\nTask:\n{}", tool_name, prompt)),
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
                return Err(format!(
                    "Manager agent {} is not active or does not exist",
                    manager_id
                ));
            }
        } else {
            return Err("Hierarchical crew requires an active manager agent".to_string());
        }
    }

    for task in &request.tasks {
        if !enabled_agents.contains_key(&task.agent_id) {
            return Err(format!(
                "Task {} references an inactive or missing agent {}",
                task.id, task.agent_id
            ));
        }

        if !task_ids.insert(task.id.clone()) {
            return Err(format!("Task ID {} appears multiple times", task.id));
        }

        if task
            .dependencies
            .iter()
            .any(|dependency| dependency == &task.id)
        {
            return Err(format!("Task {} must not depend on itself", task.id));
        }

        dependency_graph.insert(task.id.clone(), task.dependencies.clone());
    }

    for task in &request.tasks {
        for dependency in &task.dependencies {
            if !task_ids.contains(dependency) {
                return Err(format!(
                    "Task {} references unknown dependency {}",
                    task.id, dependency
                ));
            }
        }
    }

    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    for task in &request.tasks {
        if detect_crew_cycle(&task.id, &dependency_graph, &mut visiting, &mut visited) {
            return Err(format!(
                "Zyklische Task-Abhaengigkeit erkannt, ausgehend von {}",
                task.id
            ));
        }
    }

    Ok(())
}

fn sanitize_crew_request_snapshot(request: &CrewExecuteRequest) -> CrewExecuteRequest {
    let mut snapshot = request.clone();
    snapshot.governance_resume_approval_id = None;
    snapshot.stream_id = None;

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

fn replay_provider_config_needs_restore(
    config: Option<&CrewExternalProviderConfigRequest>,
) -> bool {
    config
        .map(|entry| entry.api_key.trim().is_empty() || entry.api_key.trim() == "***redacted***")
        .unwrap_or(true)
}

fn extract_replay_provider_config(
    value: Option<&Value>,
) -> Option<CrewExternalProviderConfigRequest> {
    let profile = value?;
    if !profile
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }

    Some(CrewExternalProviderConfigRequest {
        base_url: profile
            .get("baseUrl")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        model: profile
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        api_key: profile
            .get("apiKey")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        timeout_ms: profile
            .get("timeoutMs")
            .and_then(Value::as_u64)
            .unwrap_or(600000),
    })
}

fn restore_replay_provider_configs(database: &Arc<Database>, request: &mut CrewExecuteRequest) {
    let needs_openai =
        replay_provider_config_needs_restore(request.provider_configs.open_ai_compatible.as_ref());
    let needs_openrouter =
        replay_provider_config_needs_restore(request.provider_configs.open_router.as_ref());

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
        if let Some(config) = extract_replay_provider_config(
            provider_profiles.and_then(|profiles| profiles.get("openAICompatible")),
        ) {
            request.provider_configs.open_ai_compatible = Some(config);
        }
    }

    if needs_openrouter {
        if let Some(config) = extract_replay_provider_config(
            provider_profiles.and_then(|profiles| profiles.get("openRouter")),
        ) {
            request.provider_configs.open_router = Some(config);
        }
    }
}

fn ensure_crew_run_is_approved(database: &Arc<Database>, crew_id: &str) -> Result<(), String> {
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
        "Crew start blocked: open run_gate approvals for Crew {} ({}).",
        crew_id, approval_ids,
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
          "agentName": log.agent_name,
          "sourceAgent": log.source_agent,
          "targetAgent": log.target_agent,
          "provider": log.provider,
          "model": log.model,
          "taskTitle": log.task_title,
          "phase": log.phase,
          "summary": log.summary,
          "detail": log.detail,
          "severity": log.severity,
          "providerReasoning": log.provider_reasoning,
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

    persist_crew_run_memory_summary(database, request, run_id, response);
}

async fn execute_crew_request(
    app: &tauri::AppHandle,
    database: &Arc<Database>,
    registry: &CrewExecutionRegistry,
    bridge: &CrewPythonBridge,
    request: CrewExecuteRequest,
) -> Result<CrewExecutionResponse, String> {
    if request.tasks.is_empty() {
        return Err("Crew contains no tasks".to_string());
    }

    let enabled_agents: HashMap<String, CrewExecuteAgentRequest> = request
        .agents
        .iter()
        .filter(|agent| agent.enabled)
        .cloned()
        .map(|agent| (agent.id.clone(), agent))
        .collect();

    if enabled_agents.is_empty() {
        return Err("Crew contains no active agents".to_string());
    }

    validate_crew_request(&request, &enabled_agents)?;
    let governance = collect_crew_governance_payload(database, &request, &enabled_agents)?;
    let memory_context = collect_crew_memory_payload(database, &request);
    let started_at = chrono::Utc::now().to_rfc3339();
    let run_id = uuid::Uuid::new_v4().to_string();
    let request_snapshot_json = serde_json::to_string(&sanitize_crew_request_snapshot(&request))
        .unwrap_or_else(|_| "{}".to_string());
    database
        .insert_crew_run(
            &run_id,
            &request.id,
            &request.name,
            &request.process,
            "running",
            request.manager_agent_id.as_deref(),
            None,
            &request_snapshot_json,
            &started_at,
            None,
        )
        .map_err(|error| format!("crew run could not be persisted: {error}"))?;
    let mut runtime_payload = serde_json::to_value(&request).map_err(|error| error.to_string())?;
    if let Value::Object(payload) = &mut runtime_payload {
        payload.insert(
            "governance".to_string(),
            serde_json::to_value(&governance).map_err(|error| error.to_string())?,
        );
        payload.insert(
            "memoryContext".to_string(),
            serde_json::to_value(&memory_context).map_err(|error| error.to_string())?,
        );
        payload.insert("runId".to_string(), Value::String(run_id.clone()));
    }

    let _ = database.insert_crew_run_event(
        &uuid::Uuid::new_v4().to_string(),
        &run_id,
        &request.id,
        "run_started",
        Some(&request_snapshot_json),
    );
    let runtime_context_payload = serde_json::json!({
      "subject": governance.subject,
      "subjectRoles": governance.subject_roles,
      "pendingApprovalTypes": governance.pending_approval_types,
      "memoryQuery": memory_context.query,
      "memoryEntryCount": memory_context.entries.len(),
      "userProfileCount": memory_context.user_profile.len(),
    });
    let runtime_context_json = serde_json::to_string(&runtime_context_payload).ok();
    let _ = database.insert_crew_run_event(
        &uuid::Uuid::new_v4().to_string(),
        &run_id,
        &request.id,
        "runtime_context",
        runtime_context_json.as_deref(),
    );

    if let Ok(mut canceled) = registry.canceled.lock() {
        canceled.remove(&request.id);
    }

    emit_crew_execution_log_event(
        app,
        request.stream_id.clone(),
        Some(run_id.clone()),
        CrewExecutionLogRow {
            id: uuid::Uuid::new_v4().to_string(),
            crew_id: request.id.clone(),
            agent_id: request
                .manager_agent_id
                .clone()
                .unwrap_or_else(|| "crew-runtime".to_string()),
            task_id: request
                .tasks
                .first()
                .map(|task| task.id.clone())
                .unwrap_or_else(|| "runtime".to_string()),
            action: "run_started".to_string(),
            result: format!(
                "Crew '{}' starts with {} task(s), process {}.",
                request.name,
                request.tasks.len(),
                request.process
            ),
            timestamp: chrono::Utc::now().timestamp_millis(),
            agent_name: Some("Runtime".to_string()),
            source_agent: None,
            target_agent: None,
            provider: None,
            model: request.config.as_ref().map(|config| config.model.clone()),
            task_title: request.tasks.first().map(|task| task.description.clone()),
            phase: Some("status".to_string()),
            summary: Some(format!("Crew '{}' startet", request.name)),
            detail: None,
            severity: Some("info".to_string()),
            provider_reasoning: None,
        },
    );

    let app_for_runtime_logs = app.clone();
    let stream_id_for_runtime_logs = request.stream_id.clone();
    let run_id_for_runtime_logs = run_id.clone();
    let runtime_result =
        crew_runtime_execute_request(app, bridge, &runtime_payload, move |event| {
            let stream_id = event
                .stream_id
                .or_else(|| stream_id_for_runtime_logs.clone());
            let run_id = event
                .run_id
                .or_else(|| Some(run_id_for_runtime_logs.clone()));
            emit_crew_execution_log_event(
                &app_for_runtime_logs,
                stream_id,
                run_id,
                runtime_log_to_row(event.log),
            );
        });

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
                    .map(runtime_log_to_row)
                    .collect(),
                error: runtime_response.error,
            };

            persist_crew_execution_response(
                database,
                &request,
                &run_id,
                &started_at,
                &request_snapshot_json,
                &response,
            );
            Ok(response)
        }
        Err(error) => {
            if error.contains("Crew runtime is not prepared")
                || error.contains("Crew runtime Skript fehlt")
            {
                return Err(error);
            }

            let response = CrewExecutionResponse {
                crew_id: request.id.clone(),
                status: "failed".to_string(),
                task_results: Vec::new(),
                logs: vec![CrewExecutionLogRow {
                    id: uuid::Uuid::new_v4().to_string(),
                    crew_id: request.id.clone(),
                    agent_id: request
                        .manager_agent_id
                        .clone()
                        .unwrap_or_else(|| "python-runtime".to_string()),
                    task_id: request
                        .tasks
                        .first()
                        .map(|task| task.id.clone())
                        .unwrap_or_else(|| "runtime".to_string()),
                    action: "Runtime-Fehler".to_string(),
                    result: error.clone(),
                    timestamp: chrono::Utc::now().timestamp_millis(),
                    agent_name: Some("Runtime".to_string()),
                    source_agent: None,
                    target_agent: None,
                    provider: None,
                    model: request.config.as_ref().map(|config| config.model.clone()),
                    task_title: request.tasks.first().map(|task| task.description.clone()),
                    phase: Some("error".to_string()),
                    summary: Some("Runtime-Fehler".to_string()),
                    detail: Some(error.clone()),
                    severity: Some("error".to_string()),
                    provider_reasoning: None,
                }],
                error: Some(error.clone()),
            };

            persist_crew_execution_response(
                database,
                &request,
                &run_id,
                &started_at,
                &request_snapshot_json,
                &response,
            );
            Ok(response)
        }
    }
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
        .ok_or_else(|| format!("Pipeline {} not found", request.id))?;

    let steps: Vec<PipelineStepDefinition> = serde_json::from_str(&pipeline.steps_json)
        .map_err(|err| format!("Ungueltige Steps-JSON: {}", err))?;
    let gateways = state.list_tool_gateway_entries().unwrap_or_default();
    let mut step_results = Vec::with_capacity(steps.len());
    let mut previous_context = String::new();

    for (index, step) in steps.iter().enumerate() {
        let tool_name = step.tool.clone().unwrap_or_else(|| "ollama".to_string());
        let args_text = step
            .args
            .as_ref()
            .map(value_to_step_text)
            .unwrap_or_default();
        let prompt = step
            .prompt
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| {
                if args_text.trim().is_empty() {
                    format!("Pipeline step {} has no input text", index + 1)
                } else {
                    args_text.clone()
                }
            });

        let execution = match tool_name.as_str() {
            "web_fetch" => {
                execute_pipeline_web_fetch(if args_text.trim().is_empty() {
                    &prompt
                } else {
                    &args_text
                })
                .await
            }
            "web_search" => {
                execute_pipeline_web_search(if args_text.trim().is_empty() {
                    &prompt
                } else {
                    &args_text
                })
                .await
            }
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
    let mut canceled = registry
        .canceled
        .lock()
        .map_err(|_| "Crew-Registry gesperrt".to_string())?;
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
            rows.into_iter()
                .map(
                    |(
                        id,
                        crew_id,
                        crew_name,
                        process,
                        status,
                        manager_agent_id,
                        error,
                        started_at,
                        finished_at,
                    )| CrewRunHistoryRow {
                        id,
                        crew_id,
                        crew_name,
                        process,
                        status,
                        manager_agent_id,
                        error,
                        started_at,
                        finished_at,
                    },
                )
                .collect()
        })
}

#[tauri::command]
fn crew_run_logs_list(
    state: tauri::State<'_, Arc<Database>>,
    run_id: String,
) -> Result<Vec<CrewExecutionLogRow>, String> {
    state
        .list_crew_run_logs(&run_id)
        .map_err(|err| err.to_string())
}

#[tauri::command]
fn crew_run_snapshot_get(
    state: tauri::State<'_, Arc<Database>>,
    run_id: String,
) -> Result<Option<CrewExecuteRequest>, String> {
    let snapshot = state
        .get_crew_run_snapshot(&run_id)
        .map_err(|err| err.to_string())?;
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
        .ok_or_else(|| format!("No crew snapshot exists for run {}", run_id))?;

    let mut request = serde_json::from_str::<CrewExecuteRequest>(&snapshot_json)
        .map_err(|error| error.to_string())?;
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
    state
        .list_crew_definitions()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn crew_definition_versions_list(
    state: tauri::State<'_, Arc<Database>>,
    crew_id: String,
) -> Result<Vec<db::CrewDefinitionVersionRow>, String> {
    state
        .list_crew_definition_versions(&crew_id)
        .map_err(|error| error.to_string())
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
async fn crew_approval_resolve(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Database>>,
    request: CrewApprovalResolveRequest,
) -> Result<db::CrewApprovalRow, String> {
    let approval = state
        .get_crew_approval(&request.id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Crew approval {} not found", request.id))?;

    let resolved = state
        .resolve_crew_approval(
            &request.id,
            &request.status,
            request.resolved_by.as_deref(),
            request.resolution_note.as_deref(),
        )
        .map_err(|error| error.to_string())?;

    if request.status.eq_ignore_ascii_case("approved") {
        if let Some(payload_json) = approval.payload_json.as_deref() {
            if let Ok(payload) = serde_json::from_str::<QueuedCrewApprovalPayload>(payload_json) {
                let mut queued_request = payload.request;
                queued_request.governance_resume_approval_id = Some(resolved.id.clone());
                let app_handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    let database = app_handle.state::<Arc<Database>>();
                    let registry = app_handle.state::<CrewExecutionRegistry>();
                    let bridge = app_handle.state::<CrewPythonBridge>();
                    let _ = execute_crew_request(
                        &app_handle,
                        database.inner(),
                        &registry,
                        bridge.inner(),
                        queued_request,
                    )
                    .await;
                });
            }
        }
    }

    Ok(resolved)
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
        .list_crew_run_events(
            run_id.as_deref(),
            crew_id.as_deref(),
            limit.unwrap_or(100).clamp(1, 500) as i64,
        )
        .map_err(|error| error.to_string())
}

// -- Ollama commands --------------------------------------------------------

#[tauri::command]
async fn ollama_health_check(
    config: Option<OllamaConfig>,
) -> Result<ollama::OllamaHealthResponse, String> {
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
async fn chat_turn_stream(
    app: tauri::AppHandle,
    registry: tauri::State<'_, ChatStreamRegistry>,
    request: ChatTurnRequest,
) -> Result<ollama::ChatTurnResponse, String> {
    let stream_id = request
        .stream_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let app_for_emit = app.clone();
    if let Ok(mut canceled) = registry.canceled.lock() {
        canceled.remove(&stream_id);
    }
    let stream_id_for_check = stream_id.clone();

    let result = chat_turn_stream_internal(
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
        || {
            registry
                .canceled
                .lock()
                .map(|canceled| canceled.contains(&stream_id_for_check))
                .unwrap_or(false)
        },
    )
    .await;

    if let Ok(mut canceled) = registry.canceled.lock() {
        canceled.remove(&stream_id_for_check);
    }

    result.map_err(map_ollama_error)
}

#[tauri::command]
fn chat_turn_stream_cancel(
    registry: tauri::State<'_, ChatStreamRegistry>,
    stream_id: String,
) -> Result<bool, String> {
    let mut canceled = registry
        .canceled
        .lock()
        .map_err(|_| "Chat-Stream-Registry gesperrt".to_string())?;
    Ok(canceled.insert(stream_id))
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
fn claude_code_status(
    bridge: tauri::State<'_, ClaudeCodeBridge>,
) -> claude_code_bridge::ClaudeCodeStatus {
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
        ClaudeCodeBridge::send_prompt_streaming(&config, &prompt, &sid, move |chunk| {
            let _ = app_for_emit.emit("claude-code-chunk", &chunk);
        })
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
        let mut command = Command::new("powershell");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-STA",
            "-ExecutionPolicy",
            policy,
            "-Command",
            script,
        ]);
        suppress_command_window(&mut command);
        let output = command
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

fn desktop_capture_primary_display_with_overlay(
    coordinate_overlay: bool,
) -> Result<DesktopScreenshotResponse, String> {
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
        coordinate_overlay = if coordinate_overlay {
            "$true"
        } else {
            "$false"
        },
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
    let process_name =
        escape_powershell_single_quoted(request.process_name.as_deref().unwrap_or(""));
    let process_id = request
        .process_id
        .map(|value| value.to_string())
        .unwrap_or_else(|| "$null".to_string());
    let exact_match = if request.exact_match.unwrap_or(false) {
        "$true"
    } else {
        "$false"
    };
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

fn parse_bool_tool_arg(
    tool_args: &HashMap<String, Value>,
    camel: &str,
    snake: &str,
) -> Option<bool> {
    tool_args
        .get(camel)
        .and_then(|value| value.as_bool())
        .or_else(|| tool_args.get(snake).and_then(|value| value.as_bool()))
}

fn parse_string_tool_arg<'a>(
    tool_args: &'a HashMap<String, Value>,
    camel: &str,
    snake: &str,
) -> Option<&'a str> {
    tool_args
        .get(camel)
        .and_then(|value| value.as_str())
        .or_else(|| tool_args.get(snake).and_then(|value| value.as_str()))
}

fn parse_screenshot_region(
    tool_args: &HashMap<String, Value>,
) -> Result<Option<ScreenshotDisplayRegion>, String> {
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
        "A screenshot was captured recently. Keep using this image unless a refresh is explicitly required."
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
    let screenshot_path = output_dir.join(format!(
        "screenshot-display-{}-{}.png",
        display_index, timestamp
    ));
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
            let output_dir = if let Some(dir) = request
                .tool_args
                .get("outputDir")
                .and_then(|value| value.as_str())
            {
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
            let display_index =
                parse_i64_tool_arg(&request.tool_args, "displayIndex", "display_index")
                    .unwrap_or(0);
            let reason = parse_string_tool_arg(&request.tool_args, "reason", "reason");
            // User selected aggressive auto-refresh: default to true unless explicitly disabled.
            let force_refresh =
                parse_bool_tool_arg(&request.tool_args, "forceRefresh", "force_refresh")
                    .unwrap_or(true);
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
async fn desktop_capture_primary_annotated_screenshot() -> Result<DesktopScreenshotResponse, String>
{
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
async fn desktop_launch_app(
    request: DesktopLaunchRequest,
) -> Result<DesktopLaunchResponse, String> {
    ensure_windows_desktop_support()?;
    let args = request.args.unwrap_or_default();
    let mut command = Command::new(&request.path);
    if !args.is_empty() {
        command.args(&args);
    }
    if let Some(cwd) = request
        .cwd
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    {
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
    let button = request
        .button
        .unwrap_or_else(|| "left".to_string())
        .to_lowercase();
    let (down_flag, up_flag) = match button.as_str() {
        "right" => ("0x0008", "0x0010"),
        _ => ("0x0002", "0x0004"),
    };
    let iterations = if request.double_click.unwrap_or(false) {
        2
    } else {
        1
    };
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
async fn desktop_move_mouse(
    request: DesktopMoveMouseRequest,
) -> Result<DesktopActionResponse, String> {
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
async fn desktop_keypress(
    request: DesktopKeypressRequest,
) -> Result<DesktopActionResponse, String> {
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
        let folders = state
            .list_allowed_folders()
            .map_err(|err| err.to_string())?;
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

    let allowed_folders = state
        .list_allowed_folders()
        .map_err(|err| err.to_string())?;
    let canonical_target =
        file_safety::ensure_path_allowed(PathBuf::from(path).as_path(), &allowed_folders)?;
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
        return Err("url must not be empty".to_string());
    }

    let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
    let requested_origin = network_safety::origin_for_audit(requested_url);
    let policy = load_policy_state(&state)?;
    if let Err(error) = enforce_tool_policy(
        &policy,
        "web_fetch",
        requested_url,
        policy.flags.allow_web_fetch,
    ) {
        let details = serde_json::json!({
          "requestedOrigin": requested_origin,
          "runId": run_id.as_deref(),
          "policyId": policy.active_toolset_policy_id,
          "outcome": "denied",
          "stage": "tool_policy",
          "reasonCode": "tool_policy_denied",
        });
        let _ = audit::append_audit_event(app_data_dir.clone(), "web", "fetch_url", Some(details));
        return Err(error);
    }
    if let Some(sandbox) = load_run_sandbox(&state, run_id.as_deref())? {
        if let Err(error) =
            enforce_worker_sandbox_flag(&sandbox, sandbox.allow_web_fetch, "web-fetch")
        {
            let details = serde_json::json!({
              "requestedOrigin": requested_origin,
              "runId": run_id.as_deref(),
              "policyId": policy.active_toolset_policy_id,
              "outcome": "denied",
              "stage": "sandbox_policy",
              "reasonCode": "sandbox_policy_denied",
            });
            let _ =
                audit::append_audit_event(app_data_dir.clone(), "web", "fetch_url", Some(details));
            return Err(error);
        }
    }

    let max_chars = request.max_chars.unwrap_or(4_000).clamp(500, 30_000);
    let response = match network_safety::fetch_public_text(
        requested_url,
        network_safety::MAX_TEXT_RESPONSE_BYTES,
    )
    .await
    {
        Ok(response) => response,
        Err(error) => {
            let details = serde_json::json!({
              "requestedOrigin": requested_origin,
              "runId": run_id.as_deref(),
              "policyId": policy.active_toolset_policy_id,
              "outcome": "blocked",
              "stage": "network_policy",
              "reasonCode": "network_policy_blocked",
            });
            let _ =
                audit::append_audit_event(app_data_dir.clone(), "web", "fetch_url", Some(details));
            return Err(error);
        }
    };
    let status = response.status;

    let title = extract_html_title(&response.body);
    let text = strip_html_like_content(&response.body);
    let trimmed = text.trim().to_string();
    let content: String = trimmed.chars().take(max_chars).collect();
    let truncated = response.truncated || trimmed.chars().count() > max_chars;

    let details = serde_json::json!({
      "requestedOrigin": requested_origin,
      "finalOrigin": network_safety::origin_for_audit(&response.final_url),
      "runId": run_id.as_deref(),
      "policyId": policy.active_toolset_policy_id,
      "outcome": if status.is_success() { "success" } else { "http_error" },
      "status": status.as_u16(),
      "contentType": response.content_type,
      "maxChars": max_chars,
      "truncated": truncated,
      "contentChars": content.chars().count(),
    });
    let _ = audit::append_audit_event(app_data_dir, "web", "fetch_url", Some(details));

    Ok(WebFetchResponse {
        url: response.final_url,
        status: status.as_u16(),
        ok: status.is_success(),
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
        return Err("query must not be empty".to_string());
    }

    let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
    let query_chars = query.chars().count();
    let policy = load_policy_state(&state)?;
    if let Err(error) =
        enforce_tool_policy(&policy, "web_search", query, policy.flags.allow_web_search)
    {
        let details = serde_json::json!({
          "queryChars": query_chars,
          "runId": run_id.as_deref(),
          "policyId": policy.active_toolset_policy_id,
          "outcome": "denied",
          "stage": "tool_policy",
          "reasonCode": "tool_policy_denied",
        });
        let _ = audit::append_audit_event(app_data_dir.clone(), "web", "search", Some(details));
        return Err(error);
    }
    if let Some(sandbox) = load_run_sandbox(&state, run_id.as_deref())? {
        if let Err(error) =
            enforce_worker_sandbox_flag(&sandbox, sandbox.allow_web_search, "web-search")
        {
            let details = serde_json::json!({
              "queryChars": query_chars,
              "runId": run_id.as_deref(),
              "policyId": policy.active_toolset_policy_id,
              "outcome": "denied",
              "stage": "sandbox_policy",
              "reasonCode": "sandbox_policy_denied",
            });
            let _ = audit::append_audit_event(app_data_dir.clone(), "web", "search", Some(details));
            return Err(error);
        }
    }

    let max_results = request.max_results.unwrap_or(5).clamp(1, 10);
    let encoded_query = url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>();
    let search_url = format!("https://html.duckduckgo.com/html/?q={}", encoded_query);
    let response = match network_safety::fetch_public_text(
        &search_url,
        network_safety::MAX_TEXT_RESPONSE_BYTES,
    )
    .await
    {
        Ok(response) => response,
        Err(error) => {
            let details = serde_json::json!({
              "queryChars": query_chars,
              "runId": run_id.as_deref(),
              "policyId": policy.active_toolset_policy_id,
              "outcome": "blocked",
              "stage": "network_policy",
              "reasonCode": "network_policy_blocked",
            });
            let _ = audit::append_audit_event(app_data_dir.clone(), "web", "search", Some(details));
            return Err(error);
        }
    };
    if !response.status.is_success() {
        let error = format!("web search returned HTTP {}", response.status.as_u16());
        let details = serde_json::json!({
          "queryChars": query_chars,
          "runId": run_id.as_deref(),
          "policyId": policy.active_toolset_policy_id,
          "outcome": "http_error",
          "status": response.status.as_u16(),
        });
        let _ = audit::append_audit_event(app_data_dir, "web", "search", Some(details));
        return Err(error);
    }

    let results = parse_duckduckgo_results(&response.body, max_results);

    let details = serde_json::json!({
      "queryChars": query_chars,
      "runId": run_id.as_deref(),
      "policyId": policy.active_toolset_policy_id,
      "outcome": "success",
      "resultCount": results.len(),
    });
    let _ = audit::append_audit_event(app_data_dir, "web", "search", Some(details));

    Ok(WebSearchResponse {
        query: query.to_string(),
        results,
    })
}

fn validate_shell_execution_request(
    state: &Arc<Database>,
    command_text: &str,
    requested_cwd: Option<&str>,
    run_id: Option<&str>,
) -> Result<Option<String>, String> {
    let command_text = command_text.trim();
    if command_text.is_empty() {
        return Err("command must not be empty".to_string());
    }

    let policy = load_policy_state(state)?;
    enforce_tool_policy(
        &policy,
        "shell",
        command_text,
        policy.flags.allow_shell_execution,
    )?;

    if let Some(sandbox) = load_run_sandbox(state, run_id)? {
        enforce_worker_sandbox_flag(&sandbox, sandbox.allow_shell_execution, "shell-ausfuehrung")?;
        if process_manager::detect_admin_requirement(command_text) {
            return Err(
                "sandbox blockiert shell-kommandos mit admin/elevation-anforderung".to_string(),
            );
        }
    }

    let effective_cwd = ensure_run_cwd(state, run_id, requested_cwd)?;
    enforce_shell_command_guard(state, run_id, command_text, effective_cwd.as_deref())?;
    Ok(effective_cwd)
}

#[tauri::command]
fn shell_command_validate(
    state: tauri::State<'_, Arc<Database>>,
    command: String,
    cwd: Option<String>,
    run_id: Option<String>,
) -> Result<(), String> {
    validate_shell_execution_request(&state, &command, cwd.as_deref(), run_id.as_deref())?;
    Ok(())
}

#[tauri::command]
fn exec_command(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Database>>,
    credential_state: tauri::State<'_, Arc<credential_store::CredentialStore>>,
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
    let effective_cwd = validate_shell_execution_request(
        &state,
        command_text,
        request.cwd.as_deref(),
        request.run_id.as_deref(),
    )?;
    let timeout_ms = request.timeout_ms.unwrap_or(30_000).clamp(1_000, 600_000);
    let retry_count = request.retry_count.unwrap_or(0).min(3);
    let retry_backoff_ms = request.retry_backoff_ms.unwrap_or(1_000).clamp(100, 30_000);
    let start = Instant::now();
    let (shell_override, env_vars, runtime_mode) = resolve_exec_runtime(
        &state,
        &credential_state,
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

                thread::sleep(Duration::from_millis(
                    retry_backoff_ms * (attempt as u64 + 1),
                ));
            }
            Err(err) => {
                last_error = Some(err.clone());
                last_response.stderr = err;
                last_response.duration_ms = start.elapsed().as_millis() as u64;
                last_response.normalized_status = "spawn_error".to_string();

                if attempt == retry_count {
                    break;
                }

                thread::sleep(Duration::from_millis(
                    retry_backoff_ms * (attempt as u64 + 1),
                ));
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
fn project_list(state: tauri::State<'_, Arc<Database>>) -> Result<Vec<ProjectRecord>, String> {
    let projects = state.list_projects().map_err(|e| e.to_string())?;
    let resources = state.list_project_resources().map_err(|e| e.to_string())?;
    let threads = state.list_project_threads().map_err(|e| e.to_string())?;

    let mut resources_by_project: HashMap<String, Vec<ProjectResourceRecord>> = HashMap::new();
    for resource in resources {
        resources_by_project
            .entry(resource.project_id.clone())
            .or_default()
            .push(ProjectResourceRecord {
                id: resource.id,
                project_id: resource.project_id,
                kind: resource.kind,
                path: resource.path,
                label: resource.label,
                enabled: resource.enabled,
                added_at: resource.added_at,
            });
    }

    let mut threads_by_project: HashMap<String, Vec<String>> = HashMap::new();
    for (project_id, thread_id) in threads {
        threads_by_project
            .entry(project_id)
            .or_default()
            .push(thread_id);
    }

    Ok(projects
        .into_iter()
        .map(|project| ProjectRecord {
            id: project.id.clone(),
            title: project.title,
            instructions: project.instructions,
            resources: resources_by_project.remove(&project.id).unwrap_or_default(),
            thread_ids: threads_by_project.remove(&project.id).unwrap_or_default(),
            created_at: project.created_at,
            updated_at: project.updated_at,
        })
        .collect())
}

#[tauri::command]
fn project_upsert(
    state: tauri::State<'_, Arc<Database>>,
    request: ProjectUpsertRequest,
) -> Result<(), String> {
    let title = request.title.trim();
    if title.is_empty() {
        return Err("Project name must not be empty".to_string());
    }

    let now = chrono::Utc::now().to_rfc3339();
    let created_at = request.created_at.as_deref().unwrap_or(&now);
    let updated_at = request.updated_at.as_deref().unwrap_or(&now);
    state
        .upsert_project(
            request.id.trim(),
            title,
            request.instructions.as_deref().unwrap_or("").trim(),
            created_at,
            updated_at,
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn project_delete(
    state: tauri::State<'_, Arc<Database>>,
    project_id: String,
    delete_threads: Option<bool>,
) -> Result<ProjectDeleteResponse, String> {
    let deleted_thread_ids = state
        .delete_project(project_id.trim(), delete_threads.unwrap_or(false))
        .map_err(|e| e.to_string())?;
    Ok(ProjectDeleteResponse { deleted_thread_ids })
}

#[tauri::command]
fn project_resource_upsert(
    state: tauri::State<'_, Arc<Database>>,
    request: ProjectResourceUpsertRequest,
) -> Result<(), String> {
    let path = request.path.trim();
    if path.is_empty() {
        return Err("Source must not be empty".to_string());
    }

    let now = chrono::Utc::now().to_rfc3339();
    state
        .upsert_project_resource(
            request.id.trim(),
            request.project_id.trim(),
            request.kind.trim(),
            path,
            request
                .label
                .as_deref()
                .map(str::trim)
                .filter(|label| !label.is_empty()),
            request.enabled.unwrap_or(true),
            request.added_at.as_deref().unwrap_or(&now),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn project_resource_delete(
    state: tauri::State<'_, Arc<Database>>,
    resource_id: String,
) -> Result<(), String> {
    state
        .delete_project_resource(resource_id.trim())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn project_resource_set_enabled(
    state: tauri::State<'_, Arc<Database>>,
    resource_id: String,
    enabled: bool,
) -> Result<(), String> {
    state
        .set_project_resource_enabled(resource_id.trim(), enabled)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn project_attach_thread(
    state: tauri::State<'_, Arc<Database>>,
    project_id: String,
    thread_id: String,
) -> Result<(), String> {
    state
        .attach_project_thread(project_id.trim(), thread_id.trim())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn project_detach_thread(
    state: tauri::State<'_, Arc<Database>>,
    project_id: String,
    thread_id: String,
) -> Result<(), String> {
    state
        .detach_project_thread(project_id.trim(), thread_id.trim())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn db_save_thread(
    state: tauri::State<'_, Arc<Database>>,
    id: String,
    title: String,
    created_at: String,
    provider_settings_json: Option<String>,
    permission_config_json: Option<String>,
) -> Result<(), String> {
    state
        .insert_thread(
            &id,
            &title,
            &created_at,
            provider_settings_json.as_deref(),
            permission_config_json.as_deref(),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn db_list_threads(state: tauri::State<'_, Arc<Database>>) -> Result<Vec<ThreadRow>, String> {
    state.list_threads().map_err(|e| e.to_string()).map(|rows| {
        rows.into_iter()
            .map(
                |(id, title, ca, ua, provider_settings_json, permission_config_json)| ThreadRow {
                    id,
                    title,
                    created_at: ca,
                    updated_at: ua,
                    provider_settings_json,
                    permission_config_json,
                },
            )
            .collect()
    })
}

#[tauri::command]
fn db_update_thread_provider_settings(
    state: tauri::State<'_, Arc<Database>>,
    id: String,
    provider_settings_json: Option<String>,
) -> Result<(), String> {
    state
        .update_thread_provider_settings(&id, provider_settings_json.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn db_update_thread_permission_config(
    state: tauri::State<'_, Arc<Database>>,
    id: String,
    permission_config_json: Option<String>,
) -> Result<(), String> {
    state
        .update_thread_permission_config(&id, permission_config_json.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn db_delete_thread(state: tauri::State<'_, Arc<Database>>, id: String) -> Result<(), String> {
    state.delete_thread(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn db_save_message(
    state: tauri::State<'_, Arc<Database>>,
    id: String,
    thread_id: String,
    role: String,
    content: String,
    timestamp: i64,
) -> Result<(), String> {
    state
        .insert_message(&id, &thread_id, &role, &content, timestamp)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn db_update_message_content(
    state: tauri::State<'_, Arc<Database>>,
    id: String,
    content: String,
) -> Result<(), String> {
    state
        .update_message_content(&id, &content)
        .map_err(|e| e.to_string())
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
fn db_list_messages(
    state: tauri::State<'_, Arc<Database>>,
    thread_id: String,
) -> Result<Vec<MessageRow>, String> {
    state
        .list_messages(&thread_id)
        .map_err(|e| e.to_string())
        .map(|rows| {
            rows.into_iter()
                .map(|(id, role, content, ts)| MessageRow {
                    id,
                    role,
                    content,
                    timestamp: ts,
                })
                .collect()
        })
}

#[tauri::command]
fn db_save_task(
    state: tauri::State<'_, Arc<Database>>,
    id: String,
    title: String,
    prompt: String,
    status: String,
    thread_id: Option<String>,
    created_at: String,
) -> Result<(), String> {
    state
        .insert_task(
            &id,
            &title,
            &prompt,
            &status,
            thread_id.as_deref(),
            &created_at,
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn db_update_task_status(
    state: tauri::State<'_, Arc<Database>>,
    id: String,
    status: String,
) -> Result<(), String> {
    state
        .update_task_status(&id, &status)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn db_list_tasks(state: tauri::State<'_, Arc<Database>>) -> Result<Vec<TaskRow>, String> {
    state.list_tasks().map_err(|e| e.to_string()).map(|rows| {
        rows.into_iter()
            .map(
                |(id, title, prompt, status, thread_id, ca, ua, error)| TaskRow {
                    id,
                    title,
                    prompt,
                    status,
                    thread_id,
                    created_at: ca,
                    updated_at: ua,
                    error,
                },
            )
            .collect()
    })
}

fn normalize_work_task_runner(runner: &str) -> Result<String, String> {
    let normalized = runner.trim();
    if normalized == "crew" || normalized == "model" {
        Ok(normalized.to_string())
    } else {
        Err("WorkTask runner must be 'crew' or 'model'.".to_string())
    }
}

fn normalize_work_task_status(status: Option<&str>) -> Result<String, String> {
    let normalized = status.unwrap_or("idle").trim();
    match normalized {
        "idle" | "waiting_approval" | "running" | "completed" | "failed" | "canceled" => {
            Ok(normalized.to_string())
        }
        _ => Err("WorkTask status is invalid.".to_string()),
    }
}

fn map_work_task_record(row: db::WorkTaskRow) -> WorkTaskRecord {
    WorkTaskRecord {
        id: row.id,
        title: row.title,
        prompt: row.prompt,
        expected_output: row.expected_output,
        work_dir: row.work_dir,
        thread_id: row.thread_id,
        runner: row.runner,
        crew_id: row.crew_id,
        model: row.model,
        schedule_expr: row.schedule_expr,
        schedule_enabled: row.schedule_enabled,
        status: row.status,
        output: row.output,
        error: row.error,
        last_run_at: row.last_run_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

#[tauri::command]
fn work_task_list(state: tauri::State<'_, Arc<Database>>) -> Result<Vec<WorkTaskRecord>, String> {
    state
        .list_work_tasks()
        .map_err(|err| err.to_string())
        .map(|rows| rows.into_iter().map(map_work_task_record).collect())
}

#[tauri::command]
fn work_task_upsert(
    state: tauri::State<'_, Arc<Database>>,
    request: WorkTaskUpsertRequest,
) -> Result<WorkTaskRecord, String> {
    let id = request.id.trim();
    if id.is_empty() {
        return Err("WorkTask id must not be empty.".to_string());
    }

    let prompt = request.prompt.trim();
    let runner = normalize_work_task_runner(&request.runner)?;
    let status = normalize_work_task_status(request.status.as_deref())?;
    let schedule_expr = request.schedule_expr.unwrap_or_default().trim().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let created_at = request.created_at.unwrap_or_else(|| now.clone());
    let updated_at = request.updated_at.unwrap_or_else(|| now.clone());

    let row = db::WorkTaskRow {
        id: id.to_string(),
        title: request.title.trim().to_string(),
        prompt: prompt.to_string(),
        expected_output: request
            .expected_output
            .unwrap_or_default()
            .trim()
            .to_string(),
        work_dir: request.work_dir.unwrap_or_default().trim().to_string(),
        thread_id: request.thread_id.and_then(|value| {
            let trimmed = value.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        }),
        runner: runner.clone(),
        crew_id: if runner == "crew" {
            request.crew_id.and_then(|value| {
                let trimmed = value.trim().to_string();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            })
        } else {
            None
        },
        model: if runner == "model" {
            request.model.unwrap_or_default().trim().to_string()
        } else {
            String::new()
        },
        schedule_enabled: request.schedule_enabled.unwrap_or(false) && !schedule_expr.is_empty(),
        schedule_expr,
        status,
        output: request.output,
        error: request.error,
        last_run_at: request.last_run_at,
        created_at,
        updated_at,
    };

    state
        .upsert_work_task(&row)
        .map_err(|err| err.to_string())?;
    state
        .get_work_task(&row.id)
        .map_err(|err| err.to_string())?
        .map(map_work_task_record)
        .ok_or_else(|| "WorkTask not found after upsert.".to_string())
}

#[tauri::command]
fn work_task_delete(state: tauri::State<'_, Arc<Database>>, id: String) -> Result<(), String> {
    state
        .delete_work_task(id.trim())
        .map_err(|err| err.to_string())
}

#[tauri::command]
fn work_task_update_status(
    state: tauri::State<'_, Arc<Database>>,
    request: WorkTaskStatusUpdateRequest,
) -> Result<WorkTaskRecord, String> {
    let id = request.id.trim();
    if id.is_empty() {
        return Err("WorkTask id must not be empty.".to_string());
    }

    let status = normalize_work_task_status(Some(&request.status))?;
    let updated_at = chrono::Utc::now().to_rfc3339();
    state
        .update_work_task_status(
            id,
            &status,
            request.output.as_deref(),
            request.error.as_deref(),
            request.last_run_at.as_deref(),
            &updated_at,
        )
        .map_err(|err| err.to_string())?;
    state
        .get_work_task(id)
        .map_err(|err| err.to_string())?
        .map(map_work_task_record)
        .ok_or_else(|| "WorkTask not found after status update.".to_string())
}

#[tauri::command]
fn db_save_step(
    state: tauri::State<'_, Arc<Database>>,
    id: String,
    task_id: String,
    idx: i32,
    title: String,
    state_val: String,
    requires_approval: bool,
    risk_level: String,
) -> Result<(), String> {
    state
        .insert_step(
            &id,
            &task_id,
            idx,
            &title,
            &state_val,
            requires_approval,
            &risk_level,
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn db_update_step(
    state: tauri::State<'_, Arc<Database>>,
    id: String,
    state_val: String,
    output: Option<String>,
) -> Result<(), String> {
    state
        .update_step_state(&id, &state_val, output.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn db_list_steps(
    state: tauri::State<'_, Arc<Database>>,
    task_id: String,
) -> Result<Vec<StepRow>, String> {
    state
        .list_steps(&task_id)
        .map_err(|e| e.to_string())
        .map(|rows| {
            rows.into_iter()
                .map(|(id, idx, title, st, ra, rl, output)| StepRow {
                    id,
                    idx,
                    title,
                    state: st,
                    requires_approval: ra,
                    risk_level: rl,
                    output,
                })
                .collect()
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
        state
            .set_task_error(&task_id, "task has no steps")
            .map_err(|e| e.to_string())?;
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
                    .update_step_state(&step_id, "skipped", Some("Task was cancelled"))
                    .map_err(|e| e.to_string())?;
                return Ok(());
            }

            state
                .update_step_state(&step_id, "running", None)
                .map_err(|e| e.to_string())?;
            thread::sleep(Duration::from_millis(50));

            let current_status = state
                .list_tasks()
                .map_err(|e| e.to_string())?
                .into_iter()
                .find(|(id, _, _, _, _, _, _, _)| id == &task_id)
                .map(|(_, _, _, status, _, _, _, _)| status)
                .unwrap_or_else(|| "failed".to_string());
            if current_status == "cancelled" {
                state
                    .update_step_state(&step_id, "skipped", Some("Task was cancelled"))
                    .map_err(|e| e.to_string())?;
                return Ok(());
            }

            let output = format!("Automatically executed: {}", title);
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
    let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;

    audit::append_audit_event(app_data_dir, &area, &action, details)
}

#[tauri::command]
async fn credential_set(
    state: tauri::State<'_, Arc<credential_store::CredentialStore>>,
    request: credential_store::CredentialSetRequest,
) -> Result<(), String> {
    credential_store::validate_frontend_access(&request.locator)
        .map_err(|error| error.to_string())?;
    let store = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        store
            .set(&request.locator, &request.value)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|_| "credential storage worker failed".to_string())?
}

#[tauri::command]
async fn credential_get(
    state: tauri::State<'_, Arc<credential_store::CredentialStore>>,
    request: credential_store::CredentialLocator,
) -> Result<credential_store::CredentialReadResponse, String> {
    credential_store::validate_frontend_access(&request).map_err(|error| error.to_string())?;
    let store = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        store
            .get(&request)
            .map(|value| credential_store::CredentialReadResponse { value })
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|_| "credential storage worker failed".to_string())?
}

#[tauri::command]
async fn credential_delete(
    state: tauri::State<'_, Arc<credential_store::CredentialStore>>,
    request: credential_store::CredentialLocator,
) -> Result<(), String> {
    credential_store::validate_frontend_access(&request).map_err(|error| error.to_string())?;
    let store = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        store.delete(&request).map_err(|error| error.to_string())
    })
    .await
    .map_err(|_| "credential storage worker failed".to_string())?
}

#[tauri::command]
fn fs_list_allowed_folders(state: tauri::State<'_, Arc<Database>>) -> Result<Vec<String>, String> {
    state.list_allowed_folders().map_err(|err| err.to_string())
}

#[tauri::command]
fn fs_add_allowed_folder(
    state: tauri::State<'_, Arc<Database>>,
    path: String,
) -> Result<(), String> {
    let canonical = PathBuf::from(path)
        .canonicalize()
        .map_err(|err| err.to_string())?;
    state
        .add_allowed_folder(&canonical.display().to_string())
        .map_err(|err| err.to_string())
}

#[tauri::command]
fn fs_remove_allowed_folder(
    state: tauri::State<'_, Arc<Database>>,
    path: String,
) -> Result<(), String> {
    state
        .remove_allowed_folder(&path)
        .map_err(|err| err.to_string())
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
    let _ = audit::append_audit_event(
        app_data_dir,
        "file_safety",
        "import_attachment",
        Some(details),
    );

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

fn push_metadata_entry(
    path: &Path,
    metadata: &fs::Metadata,
    files: &mut Vec<FsAttachmentMetadataEntry>,
) {
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
    let canonical_target =
        file_safety::ensure_path_allowed(PathBuf::from(&path).as_path(), &allowed_folders)?;
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
    enforce_file_tool_policy(&state, "edit_file", &path)?;
    let canonical_target = ensure_run_file_access(&state, run_id.as_deref(), &path, true)?;

    let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
    let response =
        file_safety::write_text_file(&app_data_dir, &canonical_target, &content, create_backup)?;

    let details = file_safety::write_file_audit_details(
        &response.path,
        response.backup_path.as_deref(),
        response.bytes_written,
    );
    let _ = audit::append_audit_event(
        app_data_dir,
        "file_safety",
        "write_text_file",
        Some(details),
    );

    Ok(response)
}

#[tauri::command]
fn fs_create_directory(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Database>>,
    path: String,
    run_id: Option<String>,
) -> Result<file_safety::DirectoryCreateResponse, String> {
    enforce_file_tool_policy(&state, "create_directory", &path)?;
    let canonical_target = ensure_run_file_access(&state, run_id.as_deref(), &path, true)?;
    let response = file_safety::create_directory(&canonical_target)?;

    let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
    let details = file_safety::create_directory_audit_details(&response.path, response.created);
    let _ = audit::append_audit_event(
        app_data_dir,
        "file_safety",
        "create_directory",
        Some(details),
    );

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
    enforce_file_tool_policy(&state, "move_path", &source_path)?;
    enforce_file_tool_policy(&state, "move_path", &destination_path)?;
    let canonical_source = ensure_run_file_access(&state, run_id.as_deref(), &source_path, true)?;
    let canonical_destination =
        ensure_run_file_access(&state, run_id.as_deref(), &destination_path, true)?;
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
    enforce_file_tool_policy(&state, "copy_path", &source_path)?;
    enforce_file_tool_policy(&state, "copy_path", &destination_path)?;
    let canonical_source = ensure_run_file_access(&state, run_id.as_deref(), &source_path, false)?;
    let canonical_destination =
        ensure_run_file_access(&state, run_id.as_deref(), &destination_path, true)?;
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
    run_id: Option<String>,
) -> Result<(), String> {
    enforce_file_tool_policy(&state, "delete_file", &path)?;
    let canonical_target = ensure_run_file_access(&state, run_id.as_deref(), &path, true)?;

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
    let allowed_folders = state
        .list_allowed_folders()
        .map_err(|err| err.to_string())?;
    let canonical_target =
        file_safety::ensure_path_allowed(PathBuf::from(&target_path).as_path(), &allowed_folders)?;

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
    let watchers = watch_registry
        .watchers
        .lock()
        .map_err(|_| "watch registry is poisoned")?;
    Ok(watchers.keys().cloned().collect())
}

#[tauri::command]
fn fs_watch_start(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Database>>,
    watch_registry: tauri::State<'_, WatchRegistry>,
    path: String,
) -> Result<(), String> {
    let allowed_folders = state
        .list_allowed_folders()
        .map_err(|err| err.to_string())?;
    let canonical_target =
        file_safety::ensure_path_allowed(PathBuf::from(&path).as_path(), &allowed_folders)?;
    let watched_path = canonical_target.display().to_string();

    {
        let watchers = watch_registry
            .watchers
            .lock()
            .map_err(|_| "watch registry is poisoned")?;
        if watchers.contains_key(&watched_path) {
            return Ok(());
        }
    }

    let app_handle = app.clone();
    let watched_path_for_callback = watched_path.clone();

    let mut watcher =
        notify::recommended_watcher(move |result: Result<notify::Event, notify::Error>| {
            if let Ok(event) = result {
                let payload = file_watch::to_payload(&watched_path_for_callback, &event);
                let _ = app_handle.emit("file_safety://watch_event", payload.clone());

                if let Ok(app_data_dir) = app_handle.path().app_data_dir() {
                    let details = serde_json::to_value(payload).ok();
                    let _ = audit::append_audit_event(
                        app_data_dir,
                        "file_safety",
                        "watch_event",
                        details,
                    );
                }
            }
        })
        .map_err(|err| err.to_string())?;

    watcher
        .watch(canonical_target.as_path(), RecursiveMode::Recursive)
        .map_err(|err| err.to_string())?;

    let mut watchers = watch_registry
        .watchers
        .lock()
        .map_err(|_| "watch registry is poisoned")?;
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

    let mut watchers = watch_registry
        .watchers
        .lock()
        .map_err(|_| "watch registry is poisoned")?;
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
    let allowed_folders = state
        .list_allowed_folders()
        .map_err(|err| err.to_string())?;
    let canonical_target =
        file_safety::ensure_path_allowed(PathBuf::from(&path).as_path(), &allowed_folders)?;

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
    let _ = audit::append_audit_event(
        app_data_dir,
        "file_safety",
        "extract_text_limited",
        Some(details),
    );

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
    let allowed_folders = state
        .list_allowed_folders()
        .map_err(|err| err.to_string())?;
    let canonical_target =
        file_safety::ensure_path_allowed(PathBuf::from(&path).as_path(), &allowed_folders)?;
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
    let _ = audit::append_audit_event(
        app_data_dir,
        "file_safety",
        "save_artifact_version",
        Some(details),
    );

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
            rows.into_iter()
                .map(
                    |(
                        id,
                        run_id,
                        label,
                        source_path,
                        format,
                        size_bytes,
                        summary,
                        preview,
                        metadata_json,
                        created_at,
                    )| {
                        let metadata: Value = serde_json::from_str(&metadata_json)
                            .unwrap_or_else(|_| serde_json::json!({}));
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
    let allowed_folders = state
        .list_allowed_folders()
        .map_err(|err| err.to_string())?;
    let canonical_dir =
        file_safety::ensure_path_allowed(PathBuf::from(&target_dir).as_path(), &allowed_folders)?;
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
        _ => {
            return Err(
                "unsupported export format (allowed: json, md, txt, pdf, docx, xlsx, pptx)"
                    .to_string(),
            );
        }
    };

    let source_stem = PathBuf::from(&source_path)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("artifact")
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();

    let short_id: String = version_id.chars().take(8).collect();
    let file_name = format!("{}_{}_export.{}", source_stem, short_id, extension);
    let target_path = canonical_dir.join(file_name);

    let metadata =
        serde_json::from_str::<Value>(&metadata_json).unwrap_or_else(|_| serde_json::json!({}));

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
                "# Artifact export\n\n- Artifact version: {}\n- Run-ID: {}\n- Label: {}\n- Source: {}\n- Format: {}\n- Size: {} Bytes\n\n## Summary\n\n{}\n\n## Preview\n\n```\n{}\n```\n",
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
                "Artifact version: {}\nRun-ID: {}\nLabel: {}\nSource: {}\nFormat: {}\nSize: {} Bytes\n\nSummary:\n{}\n\nPreview:\n{}\n",
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
        cowork_features::export_artifact_version_native(
            target_path.as_path(),
            &format,
            &native_input,
        )?;
        fs::metadata(&target_path)
            .map_err(|err| err.to_string())?
            .len() as i64
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
    let _ = audit::append_audit_event(
        app_data_dir,
        "artifact_pipeline",
        "export_artifact_version",
        Some(details),
    );

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
            rows.into_iter()
                .map(
                    |(
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
                    )| {
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
    let allowed_folders = state
        .list_allowed_folders()
        .map_err(|err| err.to_string())?;
    let mut canonical_paths = Vec::new();

    for path in &request.paths {
        let canonical =
            file_safety::ensure_path_allowed(PathBuf::from(path).as_path(), &allowed_folders)?;
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
    let allowed_folders = state
        .list_allowed_folders()
        .map_err(|err| err.to_string())?;
    let csv_path = file_safety::ensure_path_allowed(
        PathBuf::from(&request.csv_path).as_path(),
        &allowed_folders,
    )?;
    let output_dir = file_safety::ensure_path_allowed(
        PathBuf::from(&request.output_dir).as_path(),
        &allowed_folders,
    )?;

    let response =
        cowork_features::generate_pro_outputs(request, csv_path.as_path(), output_dir.as_path())?;
    let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
    let details = serde_json::json!({
      "csvPath": response.csv_path,
      "outputDir": response.output_dir,
      "generatedFiles": response.generated_files,
      "rows": response.rows,
      "columns": response.columns,
      "numericColumns": response.numeric_columns,
    });
    let _ = audit::append_audit_event(
        app_data_dir,
        "artifact_pipeline",
        "generate_pro_outputs",
        Some(details),
    );

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

    enforce_file_tool_policy(&state, "edit_file", &normalized_request.output_path)?;
    let output_path = ensure_run_file_access(
        &state,
        run_id.as_deref(),
        &normalized_request.output_path,
        true,
    )?;
    normalized_request.output_path = output_path.display().to_string();

    if let Some(template_path) = normalized_request.template_path.clone() {
        enforce_file_tool_policy(&state, "read_file", &template_path)?;
        let canonical_template =
            ensure_run_file_access(&state, run_id.as_deref(), template_path.as_str(), false)?;
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
    let _ = audit::append_audit_event(
        app_data_dir,
        "artifact_pipeline",
        "generate_office_workflow",
        Some(details),
    );

    Ok(response)
}

#[tauri::command]
fn office_detect_apps() -> office_integration::OfficeDetectResponse {
    office_integration::detect_office_apps()
}

#[tauri::command]
fn office_open_document(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Database>>,
    request: office_integration::OfficeOpenRequest,
    run_id: Option<String>,
) -> Result<office_integration::OfficeOpenResponse, String> {
    let canonical_target = ensure_run_file_access(&state, run_id.as_deref(), &request.path, false)?;
    let response =
        office_integration::open_document(canonical_target.as_path(), request.app_kind.as_deref())?;

    let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
    let details = serde_json::json!({
      "path": response.path,
      "format": response.format,
      "officeApp": response.office_app,
      "launched": response.launched,
    });
    let _ = audit::append_audit_event(
        app_data_dir,
        "document_workspace",
        "office_open_document",
        Some(details),
    );

    Ok(response)
}

#[tauri::command]
fn document_render_preview(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Database>>,
    request: office_integration::DocumentPreviewRequest,
    run_id: Option<String>,
) -> Result<office_integration::DocumentPreviewResponse, String> {
    let canonical_target = ensure_run_file_access(&state, run_id.as_deref(), &request.path, false)?;
    let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
    let response = office_integration::render_document_preview(
        canonical_target.as_path(),
        app_data_dir.as_path(),
        request.max_pages,
        request.target_width,
    )?;

    let details = serde_json::json!({
      "sourcePath": response.source_path,
      "format": response.format,
      "previewPages": response.pages.len(),
      "exportedPdfPath": response.exported_pdf_path,
      "officeApp": response.office_app,
    });
    let _ = audit::append_audit_event(
        app_data_dir,
        "document_workspace",
        "document_render_preview",
        Some(details),
    );

    Ok(response)
}

#[tauri::command]
fn office_export_preview(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Database>>,
    request: office_integration::DocumentPreviewRequest,
    run_id: Option<String>,
) -> Result<office_integration::DocumentPreviewResponse, String> {
    document_render_preview(app, state, request, run_id)
}

fn map_scheduled_task_row(
    row: (
        String,
        String,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        i64,
        String,
        bool,
        Option<String>,
        Option<String>,
        String,
        String,
    ),
) -> ScheduledTaskRow {
    let (
        id,
        name,
        prompt,
        schedule_expr,
        task_kind,
        crew_id,
        crew_snapshot_json,
        model_config_json,
        priority,
        depends_on_task_ids_json,
        active,
        last_run_at,
        next_run_at,
        created_at,
        updated_at,
    ) = row;
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
        depends_on_task_ids: serde_json::from_str::<Vec<String>>(&depends_on_task_ids_json)
            .unwrap_or_default(),
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
    task_prompt: &str,
    schedule_expr: &str,
    task_kind: &str,
    crew_id: Option<&str>,
    crew_snapshot_json: Option<&str>,
    model_config_json: Option<&str>,
) -> Result<(), String> {
    let started_at = chrono::Utc::now().to_rfc3339();
    let run_id = uuid::Uuid::new_v4().to_string();
    let next_run_at = scheduler::next_run_from_expression(schedule_expr, chrono::Utc::now())
        .ok()
        .map(|next| next.to_rfc3339());
    let claimed = database
        .begin_scheduled_run(&run_id, task_id, &started_at, next_run_at.as_deref())
        .map_err(|error| format!("scheduled run could not be persisted: {error}"))?;
    if !claimed {
        return Err("scheduled task already has a running execution".to_string());
    }
    let execution_result: Result<(String, Option<String>, Option<String>), String> = if task_kind
        .eq_ignore_ascii_case("crew")
    {
        let snapshot = crew_snapshot_json
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "crewSnapshotJson is missing for scheduled crew job".to_string());

        match snapshot {
            Ok(snapshot_json) => match serde_json::from_str::<CrewExecuteRequest>(snapshot_json) {
                Ok(request) => {
                    let registry = app.state::<CrewExecutionRegistry>();
                    let bridge = app.state::<CrewPythonBridge>();
                    match tauri::async_runtime::block_on(execute_crew_request(
                        app,
                        database,
                        &registry,
                        bridge.inner(),
                        request,
                    )) {
                        Ok(response) => {
                            let status = if response.status == "completed" {
                                "succeeded".to_string()
                            } else {
                                response.status.clone()
                            };
                            let result_json =
                                serde_json::to_string(&response).unwrap_or_else(|_| String::new());
                            Ok((status, Some(result_json), response.error))
                        }
                        Err(error) => Err(error),
                    }
                }
                Err(error) => Err(format!("Invalid crew snapshot: {}", error)),
            },
            Err(error) => Err(error),
        }
    } else {
        let runtime_config = match model_config_json.filter(|value| !value.trim().is_empty()) {
            Some(raw_json) => serde_json::from_str::<ScheduledPromptRuntimeConfig>(raw_json)
                .map(Some)
                .map_err(|error| format!("Invalid modelConfigJson: {}", error)),
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
                    .map(|value| {
                        vec![ChatMessage {
                            role: "system".to_string(),
                            content: format!("Working directory: {}", value),
                        }]
                    })
                    .unwrap_or_default();

                tauri::async_runtime::block_on(chat_turn_internal(
                    config,
                    task_prompt.to_string(),
                    history,
                    vec![],
                ))
                .map(|response| {
                    (
                        "succeeded".to_string(),
                        Some(response.assistant_message),
                        None,
                    )
                })
                .map_err(|error| error.to_string())
            }
            Err(error) => Err(error),
        }
    };

    let finished_at = chrono::Utc::now().to_rfc3339();

    match execution_result {
        Ok((status, result_json, error_json)) => {
            database
                .insert_scheduled_run(
                    &run_id,
                    task_id,
                    &status,
                    &started_at,
                    Some(&finished_at),
                    result_json.as_deref(),
                    error_json.as_deref(),
                )
                .map_err(|error| error.to_string())?;
            database
                .update_scheduled_task_runtime(task_id, Some(&finished_at), next_run_at.as_deref())
                .map_err(|error| error.to_string())?;

            if let Ok(app_data_dir) = app.path().app_data_dir() {
                let details = serde_json::json!({
                  "taskId": task_id,
                  "runId": run_id,
                  "taskKind": task_kind,
                  "crewId": crew_id,
                  "status": status,
                });
                let _ = audit::append_audit_event(
                    app_data_dir,
                    "scheduler",
                    "task_run_completed",
                    Some(details),
                );
            }
        }
        Err(err) => {
            let error_text = err.to_string();
            database
                .insert_scheduled_run(
                    &run_id,
                    task_id,
                    "failed",
                    &started_at,
                    Some(&finished_at),
                    None,
                    Some(&error_text),
                )
                .map_err(|error| error.to_string())?;
            database
                .update_scheduled_task_runtime(task_id, Some(&finished_at), next_run_at.as_deref())
                .map_err(|error| error.to_string())?;

            if let Ok(app_data_dir) = app.path().app_data_dir() {
                let details = serde_json::json!({
                  "taskId": task_id,
                  "runId": run_id,
                  "taskKind": task_kind,
                  "crewId": crew_id,
                  "status": "failed",
                  "error": error_text,
                });
                let _ = audit::append_audit_event(
                    app_data_dir,
                    "scheduler",
                    "task_run_completed",
                    Some(details),
                );
            }
        }
    }
    Ok(())
}

fn scheduled_task_dependencies_ready(
    database: &Arc<Database>,
    depends_on_task_ids_json: &str,
    task_last_run_at: Option<&str>,
) -> bool {
    let dependency_ids =
        serde_json::from_str::<Vec<String>>(depends_on_task_ids_json).unwrap_or_default();
    if dependency_ids.is_empty() {
        return true;
    }

    dependency_ids.into_iter().all(|dependency_id| {
        match database.latest_scheduled_run_status(&dependency_id) {
            Ok(Some((status, finished_at))) if status == "succeeded" => {
                if let Some(current_last_run) = task_last_run_at {
                    finished_at
                        .as_deref()
                        .map(|value| value > current_last_run)
                        .unwrap_or(false)
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
        let due_tasks: Vec<(
            String,
            String,
            String,
            String,
            Option<String>,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
            String,
            Option<String>,
        )> = database.list_due_scheduled_tasks(&now).unwrap_or_default();
        for (
            task_id,
            _task_name,
            task_prompt,
            schedule_expr,
            _,
            task_kind,
            crew_id,
            crew_snapshot_json,
            model_config_json,
            _,
            depends_on_task_ids_json,
            task_last_run_at,
        ) in due_tasks
        {
            if !scheduled_task_dependencies_ready(
                &database,
                &depends_on_task_ids_json,
                task_last_run_at.as_deref(),
            ) {
                continue;
            }
            let crew_id_ref: Option<&str> = crew_id.as_deref();
            let crew_snapshot_json_ref: Option<&str> = crew_snapshot_json.as_deref();
            let model_config_json_ref: Option<&str> = model_config_json.as_deref();
            if let Err(error) = run_scheduled_task_once(
                &app,
                &database,
                &task_id,
                &task_prompt,
                &schedule_expr,
                &task_kind,
                crew_id_ref,
                crew_snapshot_json_ref,
                model_config_json_ref,
            ) {
                if let Ok(app_data_dir) = app.path().app_data_dir() {
                    let _ = audit::append_audit_event(
                        app_data_dir,
                        "scheduler",
                        "task_run_start_failed",
                        Some(serde_json::json!({
                            "taskId": task_id,
                            "error": error,
                        })),
                    );
                }
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
    let depends_on_task_ids_json =
        serde_json::to_string(&request.depends_on_task_ids.unwrap_or_default())
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
fn scheduler_list_tasks(
    state: tauri::State<'_, Arc<Database>>,
) -> Result<Vec<ScheduledTaskRow>, String> {
    let database = state.inner().clone();
    database
        .list_scheduled_tasks()
        .map_err(|err: rusqlite::Error| err.to_string())
        .map(|rows| {
            rows.into_iter()
                .map(map_scheduled_task_row)
                .collect::<Vec<_>>()
        })
}

#[tauri::command]
fn scheduler_delete_task(state: tauri::State<'_, Arc<Database>>, id: String) -> Result<(), String> {
    let database = state.inner().clone();
    database
        .delete_scheduled_task(&id)
        .map_err(|err: rusqlite::Error| err.to_string())
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
        &task_row.2,
        &task_row.3,
        &task_row.4,
        task_row.5.as_deref(),
        task_row.6.as_deref(),
        task_row.7.as_deref(),
    )
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
            rows.into_iter()
                .map(
                    |(id, task_id, status, started_at, finished_at, result, error)| {
                        ScheduledRunRow {
                            id,
                            task_id,
                            status,
                            started_at,
                            finished_at,
                            result,
                            error,
                        }
                    },
                )
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
        401 | 403 => (
            true,
            format!(
                "Endpoint erreichbar, aber Authentifizierung erforderlich ({})",
                status
            ),
        ),
        405 => (
            true,
            format!(
                "Endpoint erreichbar, verlangt aber eine andere HTTP-Methode ({})",
                status
            ),
        ),
        404 => (false, format!("Endpoint not found ({})", status)),
        _ if status.is_server_error() => (
            false,
            format!("Endpoint antwortet mit Serverfehler ({})", status),
        ),
        _ => (
            true,
            format!("Endpoint erreichbar, antwortet mit Status {}", status),
        ),
    }
}

fn is_openai_compatible_provider(provider_kind: &str) -> bool {
    provider_kind.eq_ignore_ascii_case("openai-compatible")
        || provider_kind.eq_ignore_ascii_case("openrouter")
}

fn trim_required_url(base_url: &str) -> Result<String, String> {
    let trimmed = base_url.trim().trim_end_matches('/').to_string();
    if trimmed.is_empty() {
        return Err("baseUrl ist erforderlich".to_string());
    }

    Url::parse(&trimmed)
        .map_err(|error| format!("ungueltige URL: {}", error))
        .map(|_| trimmed)
}

fn is_service_root_url(url: &str) -> Result<bool, String> {
    let parsed = Url::parse(url).map_err(|error| format!("ungueltige URL: {}", error))?;
    Ok(parsed.path().trim_end_matches('/').is_empty())
}

fn normalize_provider_urls(candidates: Vec<String>) -> Result<Vec<String>, String> {
    let mut urls = Vec::new();
    for candidate in candidates {
        let parsed = Url::parse(&candidate)
            .map_err(|error| format!("ungueltige URL: {}", error))?
            .to_string();
        if !urls.contains(&parsed) {
            urls.push(parsed);
        }
    }

    Ok(urls)
}

fn build_provider_model_urls(provider_kind: &str, base_url: &str) -> Result<Vec<String>, String> {
    let trimmed = trim_required_url(base_url)?;
    if !is_openai_compatible_provider(provider_kind) {
        return normalize_provider_urls(vec![trimmed]);
    }

    let candidates = if trimmed.ends_with("/chat/completions") {
        vec![format!(
            "{}/models",
            trimmed.trim_end_matches("/chat/completions")
        )]
    } else if trimmed.ends_with("/models") {
        vec![trimmed]
    } else if trimmed.ends_with("/v1") {
        vec![format!("{}/models", trimmed)]
    } else if is_service_root_url(&trimmed)? {
        vec![
            format!("{}/v1/models", trimmed),
            format!("{}/models", trimmed),
        ]
    } else {
        vec![format!("{}/models", trimmed)]
    };

    normalize_provider_urls(candidates)
}

fn build_provider_chat_urls(provider_kind: &str, base_url: &str) -> Result<Vec<String>, String> {
    let trimmed = trim_required_url(base_url)?;
    if !is_openai_compatible_provider(provider_kind) {
        return normalize_provider_urls(vec![trimmed]);
    }

    let candidates = if trimmed.ends_with("/chat/completions") {
        vec![trimmed]
    } else if trimmed.ends_with("/models") {
        let without_models = trimmed.trim_end_matches("/models").trim_end_matches('/');
        if without_models.ends_with("/v1") {
            vec![format!("{}/chat/completions", without_models)]
        } else if is_service_root_url(without_models)? {
            vec![
                format!("{}/v1/chat/completions", without_models),
                format!("{}/chat/completions", without_models),
            ]
        } else {
            vec![format!("{}/chat/completions", without_models)]
        }
    } else if trimmed.ends_with("/v1") {
        vec![format!("{}/chat/completions", trimmed)]
    } else if is_service_root_url(&trimmed)? {
        vec![
            format!("{}/v1/chat/completions", trimmed),
            format!("{}/chat/completions", trimmed),
        ]
    } else {
        vec![format!("{}/chat/completions", trimmed)]
    };

    normalize_provider_urls(candidates)
}

fn apply_provider_headers(
    mut request: reqwest::RequestBuilder,
    provider_kind: &str,
    api_key: Option<&str>,
) -> reqwest::RequestBuilder {
    request = request.header("User-Agent", "Open-Cowork/1.0");
    if let Some(key) = api_key.filter(|value| !value.trim().is_empty()) {
        request = request.header("Authorization", format!("Bearer {}", key.trim()));
    }
    if provider_kind.eq_ignore_ascii_case("openrouter") {
        request = request
            .header("HTTP-Referer", "https://open-cowork.local")
            .header("X-Title", "Open Cowork");
    }
    request
}

async fn provider_get_response_text(
    client: &reqwest::Client,
    provider_kind: &str,
    endpoint: &str,
    api_key: Option<&str>,
) -> Result<(StatusCode, String), String> {
    let response = apply_provider_headers(client.get(endpoint), provider_kind, api_key)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Ok((status, body))
}

async fn provider_post_chat_probe(
    client: &reqwest::Client,
    provider_kind: &str,
    endpoint: &str,
    api_key: Option<&str>,
    model: &str,
) -> Result<(StatusCode, String), String> {
    let body = serde_json::json!({
        "model": model,
        "messages": [{ "role": "user", "content": "ping" }],
        "max_tokens": 1,
        "temperature": 0,
        "stream": false,
    });

    let response = apply_provider_headers(client.post(endpoint), provider_kind, api_key)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Ok((status, body))
}

fn response_excerpt(body: &str) -> String {
    body.chars().take(400).collect::<String>()
}

fn status_message_with_body(status: StatusCode, body: &str) -> String {
    let (reachable, message) = interpret_connector_status(status);
    if reachable && body.trim().is_empty() {
        return message;
    }

    let excerpt = response_excerpt(body);
    if excerpt.trim().is_empty() {
        message
    } else {
        format!("{}: {}", message, excerpt)
    }
}

fn gateway_subsystem(
    id: &str,
    label: &str,
    category: &str,
    status: &str,
    message: impl Into<String>,
    detail: Option<Value>,
) -> GatewaySubsystemPayload {
    GatewaySubsystemPayload {
        id: id.to_string(),
        label: label.to_string(),
        category: category.to_string(),
        status: status.to_string(),
        message: message.into(),
        checked_at: chrono::Utc::now().to_rfc3339(),
        detail_json: detail.map(|value| value.to_string()),
    }
}

fn aggregate_gateway_status(subsystems: &[GatewaySubsystemPayload]) -> String {
    if subsystems.iter().any(|entry| entry.status == "failed") {
        return "failed".to_string();
    }
    if subsystems.iter().any(|entry| {
        entry.status == "degraded" || entry.status == "unavailable" || entry.status == "unknown"
    }) {
        return "degraded".to_string();
    }
    "ok".to_string()
}

fn gateway_payload(subsystems: Vec<GatewaySubsystemPayload>) -> GatewayHealthPayload {
    GatewayHealthPayload {
        status: aggregate_gateway_status(&subsystems),
        checked_at: chrono::Utc::now().to_rfc3339(),
        subsystems,
    }
}

fn check_audit_writable(app: &tauri::AppHandle) -> GatewaySubsystemPayload {
    let Ok(app_data_dir) = app.path().app_data_dir() else {
        return gateway_subsystem(
            "audit",
            "Audit log",
            "storage",
            "failed",
            "Audit storage is unavailable",
            None,
        );
    };
    let path = app_data_dir.join("audit").join("events.jsonl");
    let writable = path
        .parent()
        .is_some_and(|parent| fs::create_dir_all(parent).is_ok())
        && fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .is_ok();
    if !writable {
        return gateway_subsystem(
            "audit",
            "Audit log",
            "storage",
            "failed",
            "Audit storage is unavailable",
            None,
        );
    }

    let report = audit::integrity_report(&app_data_dir);
    let detail = serde_json::to_value(&report).ok();
    match report.status {
        audit::AuditIntegrityStatus::Empty => gateway_subsystem(
            "audit",
            "Audit log",
            "storage",
            "ok",
            "Audit log is writable and integrity verified",
            detail,
        ),
        audit::AuditIntegrityStatus::Ok if report.chain_complete => gateway_subsystem(
            "audit",
            "Audit log",
            "storage",
            "ok",
            "Audit log is writable and integrity verified",
            detail,
        ),
        audit::AuditIntegrityStatus::Ok => gateway_subsystem(
            "audit",
            "Audit log",
            "storage",
            "degraded",
            "Audit log retained window begins at a verified partial anchor",
            detail,
        ),
        audit::AuditIntegrityStatus::Legacy => gateway_subsystem(
            "audit",
            "Audit log",
            "storage",
            "degraded",
            "Audit log contains verified legacy history",
            detail,
        ),
        audit::AuditIntegrityStatus::Tampered => gateway_subsystem(
            "audit",
            "Audit log",
            "storage",
            "failed",
            "Audit log integrity verification failed",
            detail,
        ),
        audit::AuditIntegrityStatus::Unavailable => gateway_subsystem(
            "audit",
            "Audit log",
            "storage",
            "failed",
            "Audit log integrity is unavailable",
            detail,
        ),
    }
}

fn build_local_gateway_subsystems(
    app: &tauri::AppHandle,
    state: &Arc<Database>,
) -> Vec<GatewaySubsystemPayload> {
    let mut rows = Vec::new();

    rows.push(match state.list_engine_runs(1, None) {
        Ok(_) => gateway_subsystem(
            "database",
            "SQLite database",
            "storage",
            "ok",
            "Database is reachable",
            None,
        ),
        Err(error) => gateway_subsystem(
            "database",
            "SQLite database",
            "storage",
            "failed",
            format!("Database check failed: {}", error),
            None,
        ),
    });

    rows.push(match state.list_scheduled_tasks() {
        Ok(tasks) => {
            let active = tasks.iter().filter(|entry| entry.10).count();
            gateway_subsystem(
                "scheduler",
                "Scheduler",
                "runtime",
                "ok",
                format!("{} task(s), {} active", tasks.len(), active),
                Some(serde_json::json!({ "tasks": tasks.len(), "activeTasks": active })),
            )
        }
        Err(error) => gateway_subsystem(
            "scheduler",
            "Scheduler",
            "runtime",
            "failed",
            format!("Scheduler check failed: {}", error),
            None,
        ),
    });

    rows.push(match state.list_terminal_backends() {
        Ok(backends) if backends.is_empty() => gateway_subsystem(
            "terminal_backends",
            "Terminal backends",
            "runtime",
            "unavailable",
            "No terminal backend is configured",
            Some(serde_json::json!({ "backends": 0 })),
        ),
        Ok(backends) => {
            let connected = backends
                .iter()
                .filter(|entry| entry.status == "connected")
                .count();
            gateway_subsystem(
                "terminal_backends",
                "Terminal backends",
                "runtime",
                "ok",
                format!("{} backend(s), {} connected", backends.len(), connected),
                Some(serde_json::json!({ "backends": backends.len(), "connected": connected })),
            )
        }
        Err(error) => gateway_subsystem(
            "terminal_backends",
            "Terminal backends",
            "runtime",
            "failed",
            format!("Terminal backend check failed: {}", error),
            None,
        ),
    });

    rows.push(match state.list_managed_processes() {
        Ok(processes) => {
            let running = processes
                .iter()
                .filter(|entry| entry.status == "running")
                .count();
            gateway_subsystem(
                "process_manager",
                "Process manager",
                "runtime",
                "ok",
                format!(
                    "{} process definition(s), {} running",
                    processes.len(),
                    running
                ),
                Some(serde_json::json!({ "processes": processes.len(), "running": running })),
            )
        }
        Err(error) => gateway_subsystem(
            "process_manager",
            "Process manager",
            "runtime",
            "failed",
            format!("Process manager check failed: {}", error),
            None,
        ),
    });

    rows.push(match runtime_list_servers() {
        Ok(servers) if servers.is_empty() => gateway_subsystem(
            "mcp_runtime",
            "MCP runtime",
            "tools",
            "unavailable",
            "No MCP runtime server is running",
            Some(serde_json::json!({ "servers": 0 })),
        ),
        Ok(servers) => {
            let with_errors = servers
                .iter()
                .filter(|entry| entry.last_error.is_some())
                .count();
            gateway_subsystem(
                "mcp_runtime",
                "MCP runtime",
                "tools",
                if with_errors > 0 { "degraded" } else { "ok" },
                format!(
                    "{} runtime server(s), {} with errors",
                    servers.len(),
                    with_errors
                ),
                Some(serde_json::json!({ "servers": servers.len(), "withErrors": with_errors })),
            )
        }
        Err(error) => gateway_subsystem(
            "mcp_runtime",
            "MCP runtime",
            "tools",
            "failed",
            format!("MCP runtime check failed: {}", map_mcp_error(error)),
            None,
        ),
    });

    rows.push(match state.list_tool_gateway_entries() {
        Ok(entries) => {
            let enabled = entries.iter().filter(|entry| entry.enabled).count();
            gateway_subsystem(
                "tool_gateway",
                "Tool gateway",
                "tools",
                if entries.is_empty() {
                    "unavailable"
                } else {
                    "ok"
                },
                format!("{} gateway entrie(s), {} enabled", entries.len(), enabled),
                Some(serde_json::json!({ "entries": entries.len(), "enabled": enabled })),
            )
        }
        Err(error) => gateway_subsystem(
            "tool_gateway",
            "Tool gateway",
            "tools",
            "failed",
            format!("Tool gateway check failed: {}", error),
            None,
        ),
    });

    rows.push(match state.list_worker_sandboxes(20, None) {
        Ok(sandboxes) => {
            let active = sandboxes
                .iter()
                .filter(|entry| entry.status == "active")
                .count();
            gateway_subsystem(
                "worker_sandbox",
                "Worker sandbox",
                "isolation",
                if active > 0 { "ok" } else { "degraded" },
                format!("{} sandbox record(s), {} active", sandboxes.len(), active),
                Some(serde_json::json!({ "sandboxes": sandboxes.len(), "active": active })),
            )
        }
        Err(error) => gateway_subsystem(
            "worker_sandbox",
            "Worker sandbox",
            "isolation",
            "failed",
            format!("Worker sandbox check failed: {}", error),
            None,
        ),
    });

    rows.push(gateway_subsystem(
        "isolated_runtime",
        "Container runtime",
        "isolation",
        "unavailable",
        "Docker/container execution is not configured in P0; workspace_copy sandbox is the current isolation path",
        Some(serde_json::json!({ "runtimeMode": "workspace_copy", "dockerManaged": false })),
    ));

    rows.push(check_audit_writable(app));

    rows
}

async fn gateway_provider_probe(request: &GatewayHealthRequest) -> GatewaySubsystemPayload {
    let provider_kind = request
        .provider_kind
        .as_deref()
        .map(str::trim)
        .unwrap_or("");
    let base_url = request.base_url.as_deref().map(str::trim).unwrap_or("");

    if provider_kind.is_empty() || base_url.is_empty() {
        return gateway_subsystem(
            "provider",
            "Active provider",
            "provider",
            "unavailable",
            "Provider probe skipped because provider kind or base URL is missing",
            None,
        );
    }

    let health_request = CrewProviderHealthCheckRequest {
        provider_kind: provider_kind.to_string(),
        base_url: base_url.to_string(),
        api_key: request.api_key.clone(),
        model: request.model.clone(),
        verify_tls_certificates: request.verify_tls_certificates,
    };

    match crew_provider_health_check(health_request).await {
        Ok(response) => gateway_subsystem(
            "provider",
            "Active provider",
            "provider",
            if response.reachable { "ok" } else { "failed" },
            response.message,
            Some(serde_json::json!({
                "providerKind": provider_kind,
                "endpoint": response.endpoint,
                "status": response.status,
                "checkedAt": response.checked_at,
            })),
        ),
        Err(error) => gateway_subsystem(
            "provider",
            "Active provider",
            "provider",
            "failed",
            format!("Provider probe failed: {}", error),
            Some(serde_json::json!({ "providerKind": provider_kind, "baseUrl": base_url })),
        ),
    }
}

fn runtime_mode_needs_host_mapping(runtime_mode: &str) -> bool {
    matches!(
        runtime_mode.trim().to_ascii_lowercase().as_str(),
        "container" | "docker" | "isolated" | "sandbox" | "workspace_copy" | "wsl"
    )
}

fn map_provider_url_for_runtime(
    base_url: &str,
    runtime_mode: Option<&str>,
) -> Result<RuntimeProviderMappingResponse, String> {
    let input_url = trim_required_url(base_url)?;
    let mode = runtime_mode
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("host")
        .to_string();

    if !runtime_mode_needs_host_mapping(&mode) {
        return Ok(RuntimeProviderMappingResponse {
            input_url: input_url.clone(),
            mapped_url: input_url,
            runtime_mode: mode,
            changed: false,
            reason: "Host runtime uses the configured URL unchanged".to_string(),
        });
    }

    let mut parsed =
        Url::parse(&input_url).map_err(|error| format!("ungueltige URL: {}", error))?;
    let host = parsed.host_str().unwrap_or("").to_ascii_lowercase();
    let should_map = matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1");

    if should_map {
        parsed
            .set_host(Some("host.docker.internal"))
            .map_err(|_| "provider URL host could not be remapped".to_string())?;
        return Ok(RuntimeProviderMappingResponse {
            input_url,
            mapped_url: parsed.to_string().trim_end_matches('/').to_string(),
            runtime_mode: mode,
            changed: true,
            reason: "Localhost was mapped to host.docker.internal for isolated execution"
                .to_string(),
        });
    }

    Ok(RuntimeProviderMappingResponse {
        input_url: input_url.clone(),
        mapped_url: input_url,
        runtime_mode: mode,
        changed: false,
        reason: "External or already mapped host remains unchanged".to_string(),
    })
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
            message: health.error.unwrap_or_else(|| {
                format!("Ollama reachable, {} model(s) found", health.models.len())
            }),
            checked_at,
        });
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .danger_accept_invalid_certs(!request.verify_tls_certificates)
        .build()
        .map_err(|error| error.to_string())?;

    let api_key = request.api_key.as_deref();
    if is_openai_compatible_provider(&request.provider_kind) {
        let mut last_status = None;
        let mut last_endpoint = request.base_url.trim().to_string();
        for endpoint in build_provider_model_urls(&request.provider_kind, &request.base_url)? {
            let (status, body) =
                provider_get_response_text(&client, &request.provider_kind, &endpoint, api_key)
                    .await?;
            last_status = Some(status);
            last_endpoint = endpoint.clone();

            if status.is_success() {
                if let Some(model) = request
                    .model
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    match parse_openai_models_response(&body) {
                        Ok(models) if models.iter().any(|available| available == model) => {
                            return Ok(CrewProviderHealthCheckResponse {
                                reachable: true,
                                status: Some(status.as_u16()),
                                endpoint,
                                message: format!("Endpoint reachable, model '{}' available", model),
                                checked_at,
                            });
                        }
                        Ok(models) if !models.is_empty() => {
                            let suggestion = find_model_suggestion(&models, model);
                            let message = if let Some(suggestion) = suggestion {
                                format!(
                                    "Configured model '{}' is not exactly in the model list. Did you mean '{}'? Use 'Load models' or enter exactly this value.",
                                    model, suggestion
                                )
                            } else {
                                format!(
                                    "Configured model '{}' is not in the model list. Available: {}",
                                    model,
                                    format_model_sample(&models)
                                )
                            };

                            return Ok(CrewProviderHealthCheckResponse {
                                reachable: false,
                                status: Some(status.as_u16()),
                                endpoint,
                                message,
                                checked_at,
                            });
                        }
                        Ok(_) => {
                            return Ok(CrewProviderHealthCheckResponse {
                                reachable: false,
                                status: Some(status.as_u16()),
                                endpoint,
                                message: "Model list is empty. Enter the model manually; the app will then use the chat endpoint.".to_string(),
                                checked_at,
                            });
                        }
                        Err(error) => {
                            return Ok(CrewProviderHealthCheckResponse {
                                reachable: false,
                                status: Some(status.as_u16()),
                                endpoint,
                                message: error,
                                checked_at,
                            });
                        }
                    }
                }

                let (reachable, message) = interpret_connector_status(status);
                return Ok(CrewProviderHealthCheckResponse {
                    reachable,
                    status: Some(status.as_u16()),
                    endpoint,
                    message,
                    checked_at,
                });
            }

            if matches!(status.as_u16(), 401 | 403 | 405) {
                let (reachable, message) = interpret_connector_status(status);
                return Ok(CrewProviderHealthCheckResponse {
                    reachable,
                    status: Some(status.as_u16()),
                    endpoint,
                    message,
                    checked_at,
                });
            }

            if status.as_u16() != 404 {
                let (reachable, message) = interpret_connector_status(status);
                return Ok(CrewProviderHealthCheckResponse {
                    reachable,
                    status: Some(status.as_u16()),
                    endpoint,
                    message,
                    checked_at,
                });
            }
        }

        if let Some(model) = request
            .model
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            for endpoint in build_provider_chat_urls(&request.provider_kind, &request.base_url)? {
                let (status, body) = provider_post_chat_probe(
                    &client,
                    &request.provider_kind,
                    &endpoint,
                    api_key,
                    model,
                )
                .await?;
                last_status = Some(status);
                last_endpoint = endpoint.clone();

                if status.as_u16() == 404 {
                    continue;
                }

                let (reachable, _) = interpret_connector_status(status);
                return Ok(CrewProviderHealthCheckResponse {
                    reachable,
                    status: Some(status.as_u16()),
                    endpoint,
                    message: status_message_with_body(status, &body),
                    checked_at,
                });
            }
        }

        let status = last_status.unwrap_or(StatusCode::NOT_FOUND);
        return Ok(CrewProviderHealthCheckResponse {
            reachable: false,
            status: Some(status.as_u16()),
            endpoint: last_endpoint,
            message: "Model list is not available. Enter the model manually; the app will then use the chat endpoint.".to_string(),
            checked_at,
        });
    }

    let probe_url = trim_required_url(&request.base_url)?;
    let status = probe_connector_method(&client, &probe_url, Method::GET, api_key).await?;
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
    let endpoints = build_provider_model_urls(&request.provider_kind, &request.base_url)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .danger_accept_invalid_certs(!request.verify_tls_certificates)
        .build()
        .map_err(|error| error.to_string())?;

    let api_key = request.api_key.as_deref();
    let mut last_error = None;
    let mut last_endpoint = request.base_url.trim().to_string();
    for endpoint in endpoints {
        last_endpoint = endpoint.clone();
        let response =
            apply_provider_headers(client.get(&endpoint), &request.provider_kind, api_key)
                .send()
                .await
                .map_err(|error| error.to_string())?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let excerpt = response_excerpt(&body);
            last_error = Some(format!("Provider antwortete mit {}: {}", status, excerpt));
            continue;
        }

        let body = response.text().await.unwrap_or_default();
        let models = parse_openai_models_response(&body)?;

        return Ok(CrewProviderModelsResponse { endpoint, models });
    }

    if let Some(model) = request
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(CrewProviderModelsResponse {
            endpoint: last_endpoint,
            models: vec![model.to_string()],
        });
    }

    Err(last_error
        .unwrap_or_else(|| "Model list is not available. Enter the model manually.".to_string()))
}

#[tauri::command]
async fn openai_compatible_chat_completion(
    request: OpenAiCompatibleChatCompletionRequest,
) -> Result<OpenAiCompatibleChatCompletionResponse, String> {
    let endpoint = Url::parse(request.endpoint.trim())
        .map_err(|error| format!("ungueltige URL: {}", error))?;
    let timeout_ms = request.timeout_ms.unwrap_or(600_000).max(1_000);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .danger_accept_invalid_certs(!request.verify_tls_certificates)
        .build()
        .map_err(|error| error.to_string())?;

    let mut call = client
        .post(endpoint)
        .header("User-Agent", "Open-Cowork/1.0")
        .body(request.body);

    for (name, value) in request.headers {
        if name.eq_ignore_ascii_case("host") || name.eq_ignore_ascii_case("content-length") {
            continue;
        }
        let header_name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|error| format!("ungueltiger Header '{}': {}", name, error))?;
        let header_value = reqwest::header::HeaderValue::from_str(&value)
            .map_err(|error| format!("ungueltiger Header-Wert '{}': {}", name, error))?;
        call = call.header(header_name, header_value);
    }

    let response = call.send().await.map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    let body = response.text().await.map_err(|error| error.to_string())?;

    Ok(OpenAiCompatibleChatCompletionResponse { status, body })
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

    let parsed_url =
        Url::parse(url.trim()).map_err(|error| format!("ungueltige URL: {}", error))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|error| error.to_string())?;

    let api_key = request.api_key.as_deref();
    let status =
        match probe_connector_method(&client, parsed_url.as_str(), Method::HEAD, api_key).await {
            Ok(status) => status,
            Err(_) => {
                probe_connector_method(&client, parsed_url.as_str(), Method::GET, api_key).await?
            }
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
        let _ = audit::append_audit_event(
            app_data_dir,
            "connector",
            "reachability_test",
            Some(details),
        );
    }

    Ok(ConnectorReachabilityResponse {
        reachable,
        status: Some(status.as_u16()),
        message,
        checked_at,
    })
}

#[tauri::command]
fn gateway_status(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Database>>,
) -> Result<GatewayHealthPayload, String> {
    Ok(gateway_payload(build_local_gateway_subsystems(
        &app,
        state.inner(),
    )))
}

#[tauri::command]
async fn gateway_health(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Database>>,
    request: Option<GatewayHealthRequest>,
) -> Result<GatewayHealthPayload, String> {
    let mut rows = build_local_gateway_subsystems(&app, state.inner());
    if let Some(request) = request.filter(|entry| entry.include_provider_probe) {
        rows.push(gateway_provider_probe(&request).await);
    }
    Ok(gateway_payload(rows))
}

#[tauri::command]
async fn gateway_probe(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Database>>,
    request: GatewayProbeRequest,
) -> Result<GatewaySubsystemPayload, String> {
    let subsystem = request.subsystem.trim().to_ascii_lowercase();
    if subsystem == "provider" || subsystem == "active_provider" {
        let provider = request.provider.unwrap_or(GatewayHealthRequest {
            include_provider_probe: true,
            provider_kind: None,
            base_url: None,
            api_key: None,
            model: None,
            verify_tls_certificates: true,
        });
        return Ok(gateway_provider_probe(&provider).await);
    }

    let rows = build_local_gateway_subsystems(&app, state.inner());
    rows.into_iter()
        .find(|entry| entry.id == subsystem || entry.category == subsystem)
        .ok_or_else(|| format!("unknown gateway subsystem '{}'", request.subsystem))
}

#[tauri::command]
fn gateway_logs_tail(app: tauri::AppHandle, limit: Option<usize>) -> Result<Vec<String>, String> {
    let limit = limit.unwrap_or(80).clamp(1, 1000);
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|_| "audit log is unavailable".to_string())?;
    let (integrity, lines) = audit::verified_tail(&app_data_dir, limit)?;
    if !integrity.permits_read() {
        return Err("audit log integrity verification failed".to_string());
    }
    Ok(lines
        .into_iter()
        .map(|line| sensitive_data::redact_and_bound_text(&line, 16 * 1024))
        .collect())
}

#[tauri::command]
fn support_bundle_create(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Database>>,
    recovery_state: tauri::State<'_, db::StartupRecoveryReport>,
    path: String,
) -> Result<support_bundle::SupportBundleResponse, String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|_| "support diagnostics are unavailable".to_string())?;
    let (audit_integrity, audit_lines) = audit::verified_tail(&app_data_dir, 500)?;
    let diagnostics = state
        .support_diagnostics_snapshot()
        .map_err(|error| error.to_string())?;
    let response = support_bundle::create(
        Path::new(&path),
        &app.package_info().version.to_string(),
        &diagnostics,
        recovery_state.inner(),
        &audit_lines,
        &audit_integrity,
    )?;
    let _ = audit::append_audit_event(
        app_data_dir,
        "support",
        "bundle_created",
        Some(serde_json::json!({
            "bytes": response.size_bytes,
            "files": response.file_count,
        })),
    );
    Ok(response)
}

#[tauri::command]
fn startup_recovery_status(
    state: tauri::State<'_, db::StartupRecoveryReport>,
) -> db::StartupRecoveryReport {
    state.inner().clone()
}

#[tauri::command]
fn runtime_provider_mapping_resolve(
    request: RuntimeProviderMappingRequest,
) -> Result<RuntimeProviderMappingResponse, String> {
    map_provider_url_for_runtime(&request.base_url, request.runtime_mode.as_deref())
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
        .set_policy_flag(
            POLICY_FLAG_TOOL_DISPATCHER,
            request.flags.allow_tool_dispatcher,
        )
        .map_err(|err| err.to_string())?;
    state
        .set_policy_flag(POLICY_FLAG_MCP, request.flags.allow_mcp_tool_calls)
        .map_err(|err| err.to_string())?;
    state
        .set_policy_flag(POLICY_FLAG_WEB_FETCH, request.flags.allow_web_fetch)
        .map_err(|err| err.to_string())?;
    state
        .set_policy_flag(
            POLICY_FLAG_FILE_READ,
            request.flags.allow_file_read_extraction,
        )
        .map_err(|err| err.to_string())?;
    state
        .set_policy_flag(
            POLICY_FLAG_AUTO_COMPACT,
            request.flags.auto_compact_long_context,
        )
        .map_err(|err| err.to_string())?;
    state
        .set_policy_flag(
            POLICY_FLAG_SHELL_EXECUTION,
            request.flags.allow_shell_execution,
        )
        .map_err(|err| err.to_string())?;
    state
        .set_policy_flag(POLICY_FLAG_WEB_SEARCH, request.flags.allow_web_search)
        .map_err(|err| err.to_string())?;

    state
        .replace_policy_deny_rules(&request.deny_rules)
        .map_err(|err| err.to_string())?;

    let active_toolset_policy_id =
        normalize_active_toolset_policy_id(request.active_toolset_policy_id.as_deref())?;
    let enabled_tool_ids = if active_toolset_policy_id == CUSTOM_TOOLSET_POLICY_ID {
        normalize_policy_enabled_tool_ids(&request.enabled_tool_ids)
    } else {
        find_toolset_policy(active_toolset_policy_id.as_str())
            .ok_or_else(|| format!("unknown toolset policy {}", active_toolset_policy_id))?
            .tool_ids
    };

    state
        .replace_policy_tool_states(&build_policy_tool_states(&enabled_tool_ids))
        .map_err(|err| err.to_string())?;
    state
        .set_policy_setting(POLICY_SETTING_ACTIVE_TOOLSET, &active_toolset_policy_id)
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
    let workspace_path = request.workspace_path.as_deref().or(request.cwd.as_deref());
    let status = request.status.as_deref().unwrap_or("pending");
    let phase = request.phase.as_deref().unwrap_or("queued");
    let source = request.source.as_deref().unwrap_or("desktop");
    state
        .insert_engine_run_with_gateway_metadata(
            &request.id,
            request.parent_run_id.as_deref(),
            request.thread_id.as_deref(),
            request.session_id.as_deref(),
            &request.title,
            request.input_summary.as_deref(),
            status,
            phase,
            request.cwd.as_deref(),
            request.model.as_deref(),
            request.provider.as_deref(),
            request.source.as_deref(),
            workspace_path,
            request.provider_profile_id.as_deref(),
            request.runtime_mode.as_deref(),
            request.toolset_policy_id.as_deref(),
            request.channel_kind.as_deref(),
            request.channel_ref.as_deref(),
            request.retry_count.unwrap_or(0),
            request.resumed_from_run_id.as_deref(),
            request.checkpoint_json.as_deref(),
            request.metadata_json.as_deref(),
        )
        .map_err(|err| err.to_string())?;

    let event_payload = serde_json::json!({
      "status": status,
      "phase": phase,
      "source": source,
      "workspacePath": workspace_path,
    })
    .to_string();
    state
        .insert_engine_run_event_with_details(
            &uuid::Uuid::new_v4().to_string(),
            &request.id,
            "run_created",
            Some("Run created"),
            Some(&event_payload),
            None,
        )
        .map_err(|err| err.to_string())
}

#[tauri::command]
fn engine_run_update(
    state: tauri::State<'_, Arc<Database>>,
    request: EngineRunUpdateRequest,
) -> Result<(), String> {
    let previous_status = state
        .get_engine_run(&request.id)
        .map_err(|err| err.to_string())?
        .map(|run| run.status);
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
        .map_err(|err| err.to_string())?;

    if let (Some(before), Some(after)) = (previous_status, request.status.as_deref()) {
        if before != after {
            let event_payload = serde_json::json!({
              "from": before,
              "to": after,
              "phase": request.phase,
            })
            .to_string();
            state
                .insert_engine_run_event_with_details(
                    &uuid::Uuid::new_v4().to_string(),
                    &request.id,
                    "status_changed",
                    Some("Run status changed"),
                    Some(&event_payload),
                    None,
                )
                .map_err(|err| err.to_string())?;
        }
    }

    Ok(())
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
fn engine_run_cancel(state: tauri::State<'_, Arc<Database>>, id: String) -> Result<(), String> {
    let previous_status = state
        .get_engine_run(&id)
        .map_err(|err| err.to_string())?
        .map(|run| run.status);
    if let Some(sandbox) = state
        .get_worker_sandbox_by_run(&id)
        .map_err(|err| err.to_string())?
    {
        let _ = state.update_worker_sandbox(&sandbox.id, Some("canceled"), None);
    }
    state
        .update_engine_run(
            &id,
            Some("canceled"),
            Some("canceled"),
            None,
            None,
            None,
            None,
        )
        .map_err(|err| err.to_string())?;

    if previous_status.as_deref() != Some("canceled") {
        let event_payload = serde_json::json!({
          "from": previous_status,
          "to": "canceled",
        })
        .to_string();
        state
            .insert_engine_run_event_with_details(
                &uuid::Uuid::new_v4().to_string(),
                &id,
                "run_canceled",
                Some("Run canceled"),
                Some(&event_payload),
                None,
            )
            .map_err(|err| err.to_string())?;
    }

    Ok(())
}

#[tauri::command]
fn engine_run_resume(state: tauri::State<'_, Arc<Database>>, id: String) -> Result<(), String> {
    let existing = state
        .get_engine_run(&id)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "run not found".to_string())?;

    if existing.checkpoint_json.is_none() {
        return Err("run has no checkpoint".to_string());
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
fn engine_run_retry(state: tauri::State<'_, Arc<Database>>, id: String) -> Result<String, String> {
    let existing = state
        .get_engine_run(&id)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "run not found".to_string())?;
    let new_id = uuid::Uuid::new_v4().to_string();

    state
        .insert_engine_run_with_gateway_metadata(
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
            Some(existing.source.as_str()),
            existing.workspace_path.as_deref(),
            existing.provider_profile_id.as_deref(),
            Some(existing.runtime_mode.as_str()),
            existing.toolset_policy_id.as_deref(),
            existing.channel_kind.as_deref(),
            existing.channel_ref.as_deref(),
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
fn engine_run_event_append(
    state: tauri::State<'_, Arc<Database>>,
    request: EngineRunEventAppendRequest,
) -> Result<String, String> {
    let run_id = request.run_id.trim();
    let event_type = request.event_type.trim();
    if run_id.is_empty() {
        return Err("run_id must not be empty".to_string());
    }
    if event_type.is_empty() {
        return Err("event_type must not be empty".to_string());
    }

    let id = uuid::Uuid::new_v4().to_string();
    state
        .insert_engine_run_event_with_details(
            &id,
            run_id,
            event_type,
            request
                .summary
                .as_deref()
                .map(str::trim)
                .filter(|summary| !summary.is_empty()),
            request.payload_json.as_deref(),
            request
                .redaction_level
                .as_deref()
                .map(str::trim)
                .filter(|level| !level.is_empty()),
        )
        .map_err(|err| err.to_string())?;
    Ok(id)
}

#[tauri::command]
fn engine_run_event_list(
    state: tauri::State<'_, Arc<Database>>,
    run_id: String,
    limit: Option<i64>,
) -> Result<Vec<db::EngineRunEventRow>, String> {
    state
        .list_engine_run_events(run_id.trim(), limit.unwrap_or(200).clamp(1, 1000))
        .map_err(|err| err.to_string())
}

#[tauri::command]
fn engine_run_artifact_add(
    state: tauri::State<'_, Arc<Database>>,
    request: EngineRunArtifactAddRequest,
) -> Result<String, String> {
    let run_id = request.run_id.trim();
    let kind = request.kind.trim();
    let path = request.path.trim();
    if run_id.is_empty() {
        return Err("run_id must not be empty".to_string());
    }
    if kind.is_empty() {
        return Err("kind must not be empty".to_string());
    }
    if path.is_empty() {
        return Err("path must not be empty".to_string());
    }

    let id = uuid::Uuid::new_v4().to_string();
    state
        .insert_engine_run_artifact(
            &id,
            run_id,
            kind,
            path,
            request
                .title
                .as_deref()
                .map(str::trim)
                .filter(|title| !title.is_empty()),
            request
                .summary
                .as_deref()
                .map(str::trim)
                .filter(|summary| !summary.is_empty()),
        )
        .map_err(|err| err.to_string())?;
    Ok(id)
}

#[tauri::command]
fn engine_run_artifact_list(
    state: tauri::State<'_, Arc<Database>>,
    run_id: String,
    limit: Option<i64>,
) -> Result<Vec<db::EngineRunArtifactRow>, String> {
    state
        .list_engine_run_artifacts(run_id.trim(), limit.unwrap_or(100).clamp(1, 500))
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
    state
        .delete_runtime_instruction(&id)
        .map_err(|err| err.to_string())
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

fn authorize_worker_sandbox_source(
    state: &Arc<Database>,
    parent_run_id: Option<&str>,
    source_cwd: &str,
) -> Result<PathBuf, String> {
    let allowed_roots = if let Some(parent_run_id) = parent_run_id {
        if let Some(parent_sandbox) = state
            .get_worker_sandbox_by_run(parent_run_id)
            .map_err(|err| err.to_string())?
        {
            enforce_worker_sandbox_flag(
                &parent_sandbox,
                parent_sandbox.allow_file_read,
                "sandbox-snapshot",
            )?;
            parse_json_string_array(&parent_sandbox.allowed_roots_json)?
        } else {
            state
                .list_allowed_folders()
                .map_err(|err| err.to_string())?
        }
    } else {
        state
            .list_allowed_folders()
            .map_err(|err| err.to_string())?
    };

    let canonical_source = file_safety::ensure_path_allowed(Path::new(source_cwd), &allowed_roots)?;
    if !canonical_source.is_dir() {
        return Err("source_cwd must be an allowed directory".to_string());
    }
    Ok(canonical_source)
}

#[tauri::command]
fn worker_sandbox_create(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Database>>,
    credential_state: tauri::State<'_, Arc<credential_store::CredentialStore>>,
    request: WorkerSandboxCreateRequest,
) -> Result<db::WorkerSandboxRow, String> {
    worker_sandbox::validate_sandbox_id(&request.id)?;
    if request.run_id.trim().is_empty() {
        return Err("run_id must not be empty".to_string());
    }
    if state
        .get_worker_sandbox(&request.id)
        .map_err(|err| err.to_string())?
        .is_some()
    {
        return Err(format!("sandbox '{}' already exists", request.id));
    }
    if let Some(env_json) = request.env_json.as_deref() {
        parse_env_vars_json(Some(env_json))?;
    }

    let mode = request
        .mode
        .as_deref()
        .unwrap_or("workspace_copy")
        .trim()
        .to_lowercase();
    if mode != "workspace_copy" && mode != "native" && mode != "wsl" {
        return Err(format!(
            "sandbox mode '{}' is not supported (allowed: workspace_copy, native, wsl)",
            mode
        ));
    }
    if mode == "wsl" && !cfg!(target_os = "windows") {
        return Err("sandbox mode 'wsl' ist nur unter Windows available".to_string());
    }

    let source_cwd = authorize_worker_sandbox_source(
        &state,
        request.parent_run_id.as_deref(),
        &request.source_cwd,
    )?;

    let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
    let backend = if let Some(backend_id) = request.backend_id.as_deref() {
        state
            .list_terminal_backends()
            .map_err(|err| err.to_string())?
            .into_iter()
            .find(|item| item.id == backend_id)
            .ok_or_else(|| format!("backend '{}' not found", backend_id))?
    } else {
        terminal_backends::ensure_default_local_backend(&state, &credential_state)?
    };

    let workspace = if mode == "native" {
        let sandbox_root = worker_sandbox::sandbox_root(&app_data_dir, &request.id)?;
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

    let insert_sandbox = |stored_env_json: Option<&str>| {
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
                stored_env_json,
                Some(&metadata_json),
            )
            .map_err(|err| err.to_string())
    };
    if let Some(env_json) = request.env_json.as_deref() {
        secure_config::replace(
            &credential_state,
            secure_config::SecureConfigScope::WorkerSandbox,
            &request.id,
            env_json,
            None,
            |marker| insert_sandbox(Some(marker)),
        )?;
    } else {
        insert_sandbox(None)?;
    }

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
        .ok_or_else(|| "sandbox could not be loaded".to_string())
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
    state
        .get_worker_sandbox_by_run(&run_id)
        .map_err(|err| err.to_string())
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
    credential_state: tauri::State<'_, Arc<credential_store::CredentialStore>>,
    request: WorkerSandboxUpdateRequest,
) -> Result<(), String> {
    let existing = state
        .get_worker_sandbox(&request.id)
        .map_err(|error| error.to_string())?;
    state
        .update_worker_sandbox(
            &request.id,
            request.status.as_deref(),
            request.metadata_json.as_deref(),
        )
        .map_err(|err| err.to_string())?;
    if request
        .status
        .as_deref()
        .is_some_and(|status| ["completed", "failed", "canceled", "destroyed"].contains(&status))
    {
        state
            .update_worker_sandbox_env(&request.id, None)
            .map_err(|error| error.to_string())?;
        secure_config::delete_reference(
            &credential_state,
            secure_config::SecureConfigScope::WorkerSandbox,
            &request.id,
            existing.as_ref().and_then(|row| row.env_json.as_deref()),
        )?;
    }
    Ok(())
}

#[tauri::command]
fn worker_sandbox_destroy(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Database>>,
    credential_state: tauri::State<'_, Arc<credential_store::CredentialStore>>,
    id: String,
    remove_files: Option<bool>,
) -> Result<(), String> {
    worker_sandbox::validate_sandbox_id(&id)?;
    let existing = state
        .get_worker_sandbox(&id)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("sandbox '{}' not found", id))?;
    if remove_files.unwrap_or(true) {
        let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
        worker_sandbox::destroy_workspace_snapshot(&app_data_dir, &id)?;
    }
    state
        .update_worker_sandbox(&id, Some("destroyed"), None)
        .map_err(|err| err.to_string())?;
    state
        .update_worker_sandbox_env(&id, None)
        .map_err(|error| error.to_string())?;
    secure_config::delete_reference(
        &credential_state,
        secure_config::SecureConfigScope::WorkerSandbox,
        &id,
        existing.env_json.as_deref(),
    )
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
        "delegate" | "delegatetask" | "delegate_task" | "delegation" | "handoff" => {
            "delegate_task".to_string()
        }
        "shell" | "bash" | "bashtool" => "bash".to_string(),
        "read" | "read_file" | "filereadtool" => "read_file".to_string(),
        "edit" | "edit_file" | "fileedittool" | "write" | "multiedit" | "append" | "deletefile"
        | "delete_file" | "remove_file" | "rm" => "edit_file".to_string(),
        "createdirectory" | "create_directory" | "mkdir" | "make_dir" => {
            "create_directory".to_string()
        }
        "movepath" | "move_path" | "rename_path" => "move_path".to_string(),
        "copypath" | "copy_path" => "copy_path".to_string(),
        "glob" => "glob".to_string(),
        "grep" | "search" => "grep".to_string(),
        "webfetch" | "web_fetch" => "web_fetch".to_string(),
        "websearch" | "web_search" => "web_search".to_string(),
        "officeworkflow"
        | "office_workflow"
        | "generate_office_workflow"
        | "pptx_template_workflow"
        | "docx_template_workflow" => "office_workflow".to_string(),
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

fn build_toolset_policy(
    id: &str,
    label: &str,
    description: &str,
    risk_level: &str,
    tool_ids: &[&str],
) -> ToolsetPolicyPayload {
    let requested = tool_ids
        .iter()
        .map(|tool_id| (*tool_id).to_string())
        .collect::<Vec<_>>();

    ToolsetPolicyPayload {
        id: id.to_string(),
        label: label.to_string(),
        description: description.to_string(),
        risk_level: risk_level.to_string(),
        tool_ids: normalize_policy_enabled_tool_ids(&requested),
    }
}

fn toolset_policy_definitions() -> Vec<ToolsetPolicyPayload> {
    vec![
        build_toolset_policy(
            "host_full",
            "Host full",
            "Full local agent profile for trusted workspace automation.",
            "high",
            DEFAULT_POLICY_ENABLED_TOOL_IDS,
        ),
        build_toolset_policy(
            "safe_research",
            "Safe research",
            "Read-only workspace and web research without shell, file edits, MCP, or delegation.",
            "low",
            &[
                "read_file",
                "glob",
                "grep",
                "web_fetch",
                "web_search",
                "todo",
                "ask_user",
            ],
        ),
        build_toolset_policy(
            "code_edit",
            "Code edit",
            "Local development profile with filesystem edits and shell, without web, MCP, or delegation.",
            "medium",
            &[
                "bash",
                "read_file",
                "edit_file",
                "create_directory",
                "move_path",
                "copy_path",
                "glob",
                "grep",
                "office_workflow",
                "todo",
                "ask_user",
            ],
        ),
        build_toolset_policy(
            "remote_mcp",
            "Remote MCP",
            "Connector-oriented profile for remote tools and web research, without local shell or file edits.",
            "medium",
            &["web_fetch", "web_search", "todo", "ask_user", "mcp"],
        ),
        build_toolset_policy(
            "supervisor",
            "Supervisor",
            "Coordination profile for planning, asking the user, delegation, and read-only context gathering.",
            "medium",
            &[
                "read_file",
                "glob",
                "grep",
                "todo",
                "delegate_task",
                "ask_user",
                "mcp",
            ],
        ),
    ]
}

fn find_toolset_policy(policy_id: &str) -> Option<ToolsetPolicyPayload> {
    toolset_policy_definitions()
        .into_iter()
        .find(|policy| policy.id == policy_id)
}

fn normalize_active_toolset_policy_id(input: Option<&str>) -> Result<String, String> {
    let candidate = input.unwrap_or(CUSTOM_TOOLSET_POLICY_ID).trim();
    if candidate.is_empty() || candidate == CUSTOM_TOOLSET_POLICY_ID {
        return Ok(CUSTOM_TOOLSET_POLICY_ID.to_string());
    }

    if find_toolset_policy(candidate).is_some() {
        return Ok(candidate.to_string());
    }

    Err(format!("unknown toolset policy {}", candidate))
}

fn infer_active_toolset_policy_id(stored_id: Option<&str>, enabled_tool_ids: &[String]) -> String {
    if let Some(stored_id) = stored_id.map(str::trim) {
        if stored_id == CUSTOM_TOOLSET_POLICY_ID {
            return CUSTOM_TOOLSET_POLICY_ID.to_string();
        }
        if let Some(policy) = find_toolset_policy(stored_id) {
            if policy.tool_ids == enabled_tool_ids {
                return policy.id;
            }
        }
    }

    toolset_policy_definitions()
        .into_iter()
        .find(|policy| policy.tool_ids == enabled_tool_ids)
        .map(|policy| policy.id)
        .unwrap_or_else(|| CUSTOM_TOOLSET_POLICY_ID.to_string())
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

    if !policy
        .enabled_tool_ids
        .iter()
        .any(|enabled_tool_id| enabled_tool_id == &canonical_tool)
    {
        return Err(format!(
            "tool {} is disabled in the profile",
            canonical_tool
        ));
    }

    if policy
        .deny_rules
        .iter()
        .any(|rule| matches_deny_rule(rule, canonical_tool.as_str(), target))
    {
        return Err(format!("deny rule blockiert {}", canonical_tool));
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
        return Err(format!("sandbox {} is not active", sandbox.id));
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
        let canonical_target =
            file_safety::ensure_path_allowed(PathBuf::from(path).as_path(), &allowed_roots)?;
        if write_access {
            if let Some(read_only_roots_json) = sandbox.read_only_roots_json.as_deref() {
                let read_only_roots = parse_json_string_array(read_only_roots_json)?;
                if !read_only_roots.is_empty()
                    && file_safety::ensure_path_allowed(
                        canonical_target.as_path(),
                        &read_only_roots,
                    )
                    .is_ok()
                {
                    return Err(format!(
                        "sandbox {} allows read-only access for {}",
                        sandbox.id, path
                    ));
                }
            }
        }
        return Ok(canonical_target);
    }

    let allowed_folders = state
        .list_allowed_folders()
        .map_err(|err| err.to_string())?;
    file_safety::ensure_path_allowed(PathBuf::from(path).as_path(), &allowed_folders)
}

fn enforce_file_tool_policy(state: &Arc<Database>, tool: &str, target: &str) -> Result<(), String> {
    let policy = load_policy_state(state)?;
    let canonical_tool = canonical_policy_tool_id(tool);
    let allowed_by_flag = canonical_tool != "read_file" || policy.flags.allow_file_read_extraction;
    enforce_tool_policy(&policy, &canonical_tool, target, allowed_by_flag)
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
    let canonical =
        file_safety::ensure_path_allowed(PathBuf::from(base).as_path(), &allowed_roots)?;
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
        file_safety::ensure_path_allowed(Path::new(cwd), &allowed_roots).map_err(|_| {
            format!(
                "working directory liegt ausserhalb erlaubter roots: {}",
                cwd
            )
        })?;
    }

    if command_contains_path_traversal(command_text) {
        return Err("command blocked: path traversal (..) is not allowed".to_string());
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
            .filter(|tool_id| stored_tool_states.get(**tool_id).copied().unwrap_or(true))
            .map(|tool_id| (*tool_id).to_string())
            .collect()
    };
    let stored_active_toolset_policy_id = state
        .get_policy_setting(POLICY_SETTING_ACTIVE_TOOLSET)
        .map_err(|err| err.to_string())?;
    let active_toolset_policy_id = infer_active_toolset_policy_id(
        stored_active_toolset_policy_id.as_deref(),
        &enabled_tool_ids,
    );

    Ok(PolicyStatePayload {
        flags,
        deny_rules,
        enabled_tool_ids,
        active_toolset_policy_id,
        toolset_policies: toolset_policy_definitions(),
    })
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
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/' {
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
    credential_store: &credential_store::CredentialStore,
    backend_id: Option<&str>,
    run_id: Option<&str>,
) -> Result<(Option<String>, HashMap<String, String>, Option<String>), String> {
    let mut shell_override: Option<String> = None;
    let mut env_vars: HashMap<String, String> = HashMap::new();
    let mut runtime_mode: Option<String> = None;

    if let Some(active_run_id) = run_id {
        if let Some(sandbox) = load_run_sandbox(state, Some(active_run_id))? {
            enforce_worker_sandbox_flag(
                &sandbox,
                sandbox.allow_shell_execution,
                "shell-ausfuehrung",
            )?;
            let resolved_env = sandbox
                .env_json
                .as_deref()
                .map(|stored| {
                    secure_config::resolve(
                        credential_store,
                        secure_config::SecureConfigScope::WorkerSandbox,
                        &sandbox.id,
                        stored,
                    )
                })
                .transpose()?;
            env_vars.extend(parse_env_vars_json(resolved_env.as_deref())?);
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
            .ok_or_else(|| format!("backend '{}' not found", active_backend_id))?;

        if backend.backend_type != "local" {
            return Err(format!(
                "backend '{}' is not supported for sandboxed exec yet",
                backend.backend_type
            ));
        }

        let resolved_config = secure_config::resolve(
            credential_store,
            secure_config::SecureConfigScope::TerminalBackend,
            &backend.id,
            &backend.config_json,
        )?;
        let config =
            serde_json::from_str::<terminal_backends::LocalBackendConfig>(&resolved_config)
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
                .map(|shell| {
                    shell.eq_ignore_ascii_case("wsl") || shell.eq_ignore_ascii_case("wsl.exe")
                })
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
            if shell_lower.contains("powershell")
                || shell_lower.ends_with("pwsh")
                || shell_lower.ends_with("pwsh.exe")
            {
                cmd.args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    wrapped_command_text.as_str(),
                ]);
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
    suppress_command_window(&mut command);
    process_control::configure_process_tree(&mut command);

    let mut child = command.spawn().map_err(|err| err.to_string())?;
    let process_tree = process_control::attach_process_tree(&child).ok();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "stdout pipe unavailable".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "stderr pipe unavailable".to_string())?;

    let stream_for_stdout = stream_id.map(|value| value.to_string());
    let stream_for_stderr = stream_id.map(|value| value.to_string());
    let app_for_stdout = app.clone();
    let app_for_stderr = app.clone();

    let stdout_handle = thread::spawn(move || {
        let mut buffer = String::new();
        let reader = BufReader::new(stdout);
        for text in reader.lines().map_while(Result::ok) {
            buffer.push_str(&text);
            buffer.push('\n');
            emit_exec_chunk(
                &app_for_stdout,
                stream_for_stdout.as_deref(),
                "stdout",
                &text,
            );
        }
        buffer
    });

    let stderr_handle = thread::spawn(move || {
        let mut buffer = String::new();
        let reader = BufReader::new(stderr);
        for text in reader.lines().map_while(Result::ok) {
            buffer.push_str(&text);
            buffer.push('\n');
            emit_exec_chunk(
                &app_for_stderr,
                stream_for_stderr.as_deref(),
                "stderr",
                &text,
            );
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
                    process_control::terminate_process_tree(&mut child, process_tree.as_ref())
                        .map_err(|error| {
                            format!("failed to terminate timed-out process: {error}")
                        })?;
                    break None;
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(err) => return Err(err.to_string()),
        }
    };

    drop(process_tree);
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

    rows.into_iter()
        .filter(|row| {
            if !row.enabled {
                return false;
            }
            match row.scope_type.as_str() {
                "global" => true,
                "workspace" => row
                    .scope_ref
                    .as_deref()
                    .map(|scope| {
                        normalized_cwd.starts_with(&scope.replace('\\', "/").to_lowercase())
                    })
                    .unwrap_or(false),
                "folder" => row
                    .scope_ref
                    .as_deref()
                    .map(|scope| {
                        normalized_cwd.starts_with(&scope.replace('\\', "/").to_lowercase())
                    })
                    .unwrap_or(false),
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
        candidates.push(resource_dir.join("pdfium").join("bin").join("pdfium.dll"));
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
        .upsert_memory_entry(
            &id,
            &scope,
            &category,
            &key,
            &content,
            source_session_id.as_deref(),
            conf,
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn memory_mutate(
    state: tauri::State<'_, Arc<Database>>,
    action: String,
    target: String,
    content: Option<String>,
    old_text: Option<String>,
    source_session_id: Option<String>,
) -> Result<memory_engine::MemoryMutationResponse, String> {
    let db_arc = state.inner().clone();
    memory_engine::mutate_curated_memory(
        &db_arc,
        &action,
        &target,
        content.as_deref(),
        old_text.as_deref(),
        source_session_id.as_deref(),
    )
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
            .search_memory_entries(kw, scope.as_deref(), category.as_deref(), lim)
            .map_err(|e| e.to_string())
    } else if let Some(requested_scope) = scope {
        state
            .list_memory_entries(&requested_scope, category.as_deref(), lim)
            .map_err(|e| e.to_string())
    } else {
        state
            .list_all_memory_entries(category.as_deref(), lim)
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
fn user_profile_list(
    state: tauri::State<'_, Arc<Database>>,
) -> Result<Vec<db::UserProfileRow>, String> {
    state.list_user_profile().map_err(|e| e.to_string())
}

#[tauri::command]
fn user_profile_delete(state: tauri::State<'_, Arc<Database>>, key: String) -> Result<(), String> {
    state
        .delete_user_profile_entry(&key)
        .map_err(|e| e.to_string())
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
fn skill_list(
    state: tauri::State<'_, Arc<Database>>,
    limit: Option<i64>,
) -> Result<Vec<db::SkillRow>, String> {
    state
        .list_skills(limit.unwrap_or(100))
        .map_err(|e| e.to_string())
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
    let db_arc = state.inner().clone();
    let mut snapshot = memory_engine::create_memory_snapshot(&db_arc)?;
    snapshot.session_id = id.clone();
    let snapshot_json = serde_json::to_string(&snapshot).map_err(|error| error.to_string())?;
    state
        .insert_session(
            &id,
            thread_id.as_deref(),
            &title,
            Some(&snapshot_json),
            model_used.as_deref(),
            provider.as_deref(),
            personality.as_deref(),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn session_memory_snapshot(
    state: tauri::State<'_, Arc<Database>>,
    session_id: String,
) -> Result<memory_engine::FrozenMemorySnapshot, String> {
    if let Some(snapshot_json) = state
        .get_session_memory_snapshot(&session_id)
        .map_err(|error| error.to_string())?
    {
        return serde_json::from_str(&snapshot_json).map_err(|error| error.to_string());
    }

    let db_arc = state.inner().clone();
    let mut snapshot = memory_engine::create_memory_snapshot(&db_arc)?;
    snapshot.session_id = session_id.clone();
    let snapshot_json = serde_json::to_string(&snapshot).map_err(|error| error.to_string())?;
    state
        .save_session_snapshot(&session_id, &snapshot_json)
        .map_err(|error| error.to_string())?;
    Ok(snapshot)
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
fn session_list(
    state: tauri::State<'_, Arc<Database>>,
    limit: Option<i64>,
) -> Result<Vec<db::SessionRow>, String> {
    state
        .list_sessions(limit.unwrap_or(100))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn session_search(
    state: tauri::State<'_, Arc<Database>>,
    query: String,
    limit: Option<i64>,
) -> Result<Vec<db::SessionSearchResultRow>, String> {
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
    let mut snapshot = memory_engine::create_memory_snapshot(&db_arc)?;
    snapshot.session_id = session_id.clone();
    let snapshot_json = serde_json::to_string(&snapshot).map_err(|e| e.to_string())?;
    state
        .save_session_snapshot(&session_id, &snapshot_json)
        .map_err(|e| e.to_string())?;
    Ok(snapshot_json)
}

#[tauri::command]
fn session_get(
    state: tauri::State<'_, Arc<Database>>,
    id: String,
) -> Result<Option<db::SessionRow>, String> {
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
fn learning_list(
    state: tauri::State<'_, Arc<Database>>,
    limit: Option<i64>,
) -> Result<Vec<db::LearningOutcomeRow>, String> {
    state
        .list_learning_outcomes(limit.unwrap_or(100))
        .map_err(|e| e.to_string())
}

// -- Terminal backend commands ----------------------------------------------

#[tauri::command]
fn backend_upsert(
    state: tauri::State<'_, Arc<Database>>,
    credential_state: tauri::State<'_, Arc<credential_store::CredentialStore>>,
    id: String,
    name: String,
    backend_type: String,
    config_json: String,
) -> Result<(), String> {
    terminal_backends::validate_backend_type(&backend_type)?;
    terminal_backends::validate_backend_config(&backend_type, &config_json)?;
    let existing = state
        .list_terminal_backends()
        .map_err(|error| error.to_string())?;
    if existing
        .iter()
        .any(|entry| entry.name == name && entry.id != id)
    {
        return Err("terminal backend name is already in use".to_string());
    }
    let previous = existing
        .iter()
        .find(|entry| entry.id == id)
        .map(|entry| entry.config_json.as_str());
    secure_config::replace(
        &credential_state,
        secure_config::SecureConfigScope::TerminalBackend,
        &id,
        &config_json,
        previous,
        |marker| {
            state
                .upsert_terminal_backend(&id, &name, &backend_type, marker)
                .map_err(|error| error.to_string())
        },
    )
    .map(|_| ())
}

#[tauri::command]
fn backend_list(
    state: tauri::State<'_, Arc<Database>>,
) -> Result<Vec<db::TerminalBackendRow>, String> {
    state.list_terminal_backends().map_err(|e| e.to_string())
}

#[tauri::command]
fn backend_delete(
    state: tauri::State<'_, Arc<Database>>,
    credential_state: tauri::State<'_, Arc<credential_store::CredentialStore>>,
    id: String,
) -> Result<(), String> {
    let stored_config = state
        .list_terminal_backends()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|entry| entry.id == id)
        .map(|entry| entry.config_json);
    state
        .delete_terminal_backend(&id)
        .map_err(|e| e.to_string())?;
    secure_config::delete_reference(
        &credential_state,
        secure_config::SecureConfigScope::TerminalBackend,
        &id,
        stored_config.as_deref(),
    )
}

#[tauri::command]
async fn backend_exec(
    state: tauri::State<'_, Arc<Database>>,
    credential_state: tauri::State<'_, Arc<credential_store::CredentialStore>>,
    backend_id: String,
    command: String,
    working_dir: Option<String>,
    timeout_ms: Option<u64>,
) -> Result<terminal_backends::BackendExecResponse, String> {
    let db_arc = state.inner().clone();
    let credential_store = credential_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        terminal_backends::dispatch_exec(
            &db_arc,
            &credential_store,
            &backend_id,
            &command,
            working_dir.as_deref(),
            timeout_ms,
        )
    })
    .await
    .map_err(|error| format!("terminal backend worker failed: {error}"))?
}

#[tauri::command]
fn backend_ensure_local(
    state: tauri::State<'_, Arc<Database>>,
    credential_state: tauri::State<'_, Arc<credential_store::CredentialStore>>,
) -> Result<db::TerminalBackendRow, String> {
    let db_arc = state.inner().clone();
    terminal_backends::ensure_default_local_backend(&db_arc, &credential_state)
}

#[tauri::command]
fn terminal_create(
    app: tauri::AppHandle,
    registry: tauri::State<'_, TerminalSessionRegistry>,
    request: terminal_sessions::TerminalCreateRequest,
) -> Result<terminal_sessions::TerminalCreateResponse, String> {
    terminal_sessions::create_terminal_session(app, registry, request)
}

#[tauri::command]
fn terminal_write(
    registry: tauri::State<'_, TerminalSessionRegistry>,
    request: terminal_sessions::TerminalWriteRequest,
) -> Result<(), String> {
    terminal_sessions::write_terminal_session(registry, request)
}

#[tauri::command]
fn terminal_resize(
    registry: tauri::State<'_, TerminalSessionRegistry>,
    request: terminal_sessions::TerminalResizeRequest,
) -> Result<(), String> {
    terminal_sessions::resize_terminal_session(registry, request)
}

#[tauri::command]
fn terminal_interrupt(
    registry: tauri::State<'_, TerminalSessionRegistry>,
    request: terminal_sessions::TerminalSessionRequest,
) -> Result<(), String> {
    terminal_sessions::interrupt_terminal_session(registry, request)
}

#[tauri::command]
fn terminal_kill(
    registry: tauri::State<'_, TerminalSessionRegistry>,
    request: terminal_sessions::TerminalSessionRequest,
) -> Result<(), String> {
    terminal_sessions::kill_terminal_session(registry, request)
}

#[tauri::command]
fn terminal_close(
    registry: tauri::State<'_, TerminalSessionRegistry>,
    request: terminal_sessions::TerminalSessionRequest,
) -> Result<(), String> {
    terminal_sessions::close_terminal_session(registry, request)
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
    Ok(process_manager::approve_and_start(
        &db_arc,
        &process_id,
        approved,
    ))
}

#[tauri::command]
fn process_list(
    state: tauri::State<'_, Arc<Database>>,
) -> Result<Vec<process_manager::ProcessStatusResult>, String> {
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
    role: Option<String>,
    goal: Option<String>,
    system_prompt: String,
    skills_markdown: Option<String>,
    temperature: Option<f64>,
    model_override: Option<String>,
    icon: Option<String>,
    is_default: Option<bool>,
) -> Result<(), String> {
    let resolved_goal = goal
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(description.as_str());
    state
        .upsert_personality(
            &id,
            &name,
            &description,
            role.as_deref().unwrap_or("custom"),
            resolved_goal,
            &system_prompt,
            skills_markdown.as_deref().unwrap_or(""),
            temperature,
            model_override.as_deref(),
            icon.as_deref(),
            is_default.unwrap_or(false),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn personality_list(
    state: tauri::State<'_, Arc<Database>>,
) -> Result<Vec<db::PersonalityRow>, String> {
    state.list_personalities().map_err(|e| e.to_string())
}

#[tauri::command]
fn personality_delete(state: tauri::State<'_, Arc<Database>>, id: String) -> Result<(), String> {
    state.delete_personality(&id).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolset_policy_definitions_are_canonical() {
        let safe_research = find_toolset_policy("safe_research").unwrap();

        assert_eq!(
            safe_research.tool_ids,
            vec![
                "read_file".to_string(),
                "glob".to_string(),
                "grep".to_string(),
                "web_fetch".to_string(),
                "web_search".to_string(),
                "todo".to_string(),
                "ask_user".to_string(),
            ]
        );
        assert!(!safe_research.tool_ids.contains(&"bash".to_string()));
        assert!(!safe_research.tool_ids.contains(&"edit_file".to_string()));
    }

    #[test]
    fn active_toolset_inference_detects_custom_edits() {
        let safe_research = find_toolset_policy("safe_research").unwrap();
        assert_eq!(
            infer_active_toolset_policy_id(Some("safe_research"), &safe_research.tool_ids),
            "safe_research"
        );

        let mut custom_tools = safe_research.tool_ids;
        custom_tools.push("bash".to_string());
        assert_eq!(
            infer_active_toolset_policy_id(Some("safe_research"), &custom_tools),
            CUSTOM_TOOLSET_POLICY_ID
        );
    }

    #[test]
    fn delete_file_uses_the_edit_file_policy_capability() {
        assert_eq!(canonical_policy_tool_id("DeleteFile"), "edit_file");
        assert_eq!(canonical_policy_tool_id("delete_file"), "edit_file");
        assert_eq!(canonical_policy_tool_id("rm"), "edit_file");
    }

    #[test]
    fn backend_file_mutations_respect_the_active_toolset_policy() {
        let database = Arc::new(Database::open_in_memory().unwrap());
        let safe_research = find_toolset_policy("safe_research").unwrap();
        database
            .replace_policy_tool_states(&build_policy_tool_states(&safe_research.tool_ids))
            .unwrap();
        database
            .set_policy_setting(POLICY_SETTING_ACTIVE_TOOLSET, "safe_research")
            .unwrap();

        assert!(
            enforce_file_tool_policy(&database, "read_file", "C:\\workspace\\notes.txt").is_ok()
        );
        assert!(
            enforce_file_tool_policy(&database, "edit_file", "C:\\workspace\\notes.txt").is_err()
        );
        assert!(
            enforce_file_tool_policy(&database, "delete_file", "C:\\workspace\\notes.txt").is_err()
        );
        assert!(enforce_file_tool_policy(
            &database,
            "create_directory",
            "C:\\workspace\\generated"
        )
        .is_err());
    }

    #[test]
    fn deny_rule_errors_do_not_echo_sensitive_targets() {
        let database = Arc::new(Database::open_in_memory().unwrap());
        database
            .replace_policy_deny_rules(&["web_fetch:*api_key=*".to_string()])
            .unwrap();
        let policy = load_policy_state(&database).unwrap();
        let target = "https://example.com/data?api_key=super-secret";

        let error = enforce_tool_policy(&policy, "web_fetch", target, true).unwrap_err();

        assert!(error.contains("web_fetch"));
        assert!(!error.contains("super-secret"));
        assert!(!error.contains(target));
    }

    #[test]
    fn runtime_provider_mapping_rewrites_localhost_for_isolation() {
        let mapped =
            map_provider_url_for_runtime("http://localhost:11434", Some("isolated")).unwrap();

        assert!(mapped.changed);
        assert_eq!(mapped.mapped_url, "http://host.docker.internal:11434");
    }

    #[test]
    fn runtime_provider_mapping_rewrites_loopback_for_isolation() {
        let mapped =
            map_provider_url_for_runtime("https://127.0.0.1:8080/v1", Some("docker")).unwrap();

        assert!(mapped.changed);
        assert_eq!(mapped.mapped_url, "https://host.docker.internal:8080/v1");
    }

    #[test]
    fn runtime_provider_mapping_keeps_external_hosts() {
        let mapped =
            map_provider_url_for_runtime("https://openrouter.ai/api/v1", Some("isolated")).unwrap();

        assert!(!mapped.changed);
        assert_eq!(mapped.mapped_url, "https://openrouter.ai/api/v1");
    }

    #[test]
    fn runtime_provider_mapping_keeps_host_runtime_localhost() {
        let mapped = map_provider_url_for_runtime("http://localhost:11434", Some("host")).unwrap();

        assert!(!mapped.changed);
        assert_eq!(mapped.mapped_url, "http://localhost:11434");
    }

    fn security_test_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "open-cowork-security-{}-{}",
            label,
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).expect("security test root should be created");
        root
    }

    #[test]
    fn worker_sandbox_source_requires_an_allowed_root() {
        let database = Arc::new(Database::open_in_memory().unwrap());
        let parent = security_test_root("sandbox-source");
        let allowed_root = parent.join("allowed");
        let outside_root = parent.join("outside");
        fs::create_dir_all(&allowed_root).unwrap();
        fs::create_dir_all(&outside_root).unwrap();
        database
            .add_allowed_folder(&allowed_root.display().to_string())
            .unwrap();

        let authorized =
            authorize_worker_sandbox_source(&database, None, &allowed_root.display().to_string())
                .expect("allowed source should be accepted");
        assert_eq!(authorized, allowed_root.canonicalize().unwrap());
        assert!(authorize_worker_sandbox_source(
            &database,
            None,
            &outside_root.display().to_string()
        )
        .is_err());

        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn run_file_access_does_not_fall_back_to_global_roots() {
        let database = Arc::new(Database::open_in_memory().unwrap());
        let parent = security_test_root("run-file-access");
        let global_root = parent.join("global");
        let sandbox_root = parent.join("sandbox");
        let global_file = global_root.join("original.txt");
        let sandbox_file = sandbox_root.join("copy.txt");
        fs::create_dir_all(&global_root).unwrap();
        fs::create_dir_all(&sandbox_root).unwrap();
        fs::write(&global_file, "original").unwrap();
        fs::write(&sandbox_file, "copy").unwrap();
        database
            .add_allowed_folder(&global_root.display().to_string())
            .unwrap();
        database
            .insert_engine_run(
                "run-sandboxed",
                None,
                None,
                None,
                "Sandboxed test run",
                None,
                "running",
                "executing",
                Some(&sandbox_root.display().to_string()),
                None,
                None,
                0,
                None,
                None,
                None,
            )
            .unwrap();
        let allowed_roots_json =
            serde_json::to_string(&vec![sandbox_root.display().to_string()]).unwrap();
        database
            .insert_worker_sandbox(
                "sandbox-test",
                "run-sandboxed",
                None,
                None,
                "active",
                "workspace_copy",
                &sandbox_root.display().to_string(),
                &sandbox_root.display().to_string(),
                &allowed_roots_json,
                None,
                true,
                true,
                false,
                false,
                false,
                false,
                None,
                None,
            )
            .unwrap();

        let allowed = ensure_run_file_access(
            &database,
            Some("run-sandboxed"),
            &sandbox_file.display().to_string(),
            true,
        )
        .expect("sandbox file should remain writable");
        assert_eq!(allowed, sandbox_file.canonicalize().unwrap());
        assert!(ensure_run_file_access(
            &database,
            Some("run-sandboxed"),
            &global_file.display().to_string(),
            true,
        )
        .is_err());

        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn shell_validation_applies_root_and_traversal_guards_to_pty_commands() {
        let database = Arc::new(Database::open_in_memory().unwrap());
        let parent = security_test_root("shell-guard");
        let allowed_root = parent.join("allowed");
        let outside_file = parent.join("outside.txt");
        fs::create_dir_all(&allowed_root).unwrap();
        fs::write(&outside_file, "secret").unwrap();
        database
            .add_allowed_folder(&allowed_root.display().to_string())
            .unwrap();
        let cwd = allowed_root.display().to_string();

        assert!(
            validate_shell_execution_request(&database, "Write-Output safe", Some(&cwd), None)
                .is_ok()
        );
        assert!(validate_shell_execution_request(
            &database,
            "Get-Content ..\\outside.txt",
            Some(&cwd),
            None
        )
        .is_err());
        assert!(validate_shell_execution_request(
            &database,
            &format!("Get-Content '{}'", outside_file.display()),
            Some(&cwd),
            None
        )
        .is_err());

        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn secure_config_migration_removes_plaintext_and_preserves_terminal_runtime() {
        let database = Arc::new(Database::open_in_memory().unwrap());
        let credentials = credential_store::CredentialStore::in_memory();
        let terminal_plaintext =
            r#"{"shell":null,"workingDir":null,"envVars":{"MIGRATION_SECRET":"terminal-secret"}}"#;
        let memory_plaintext = r#"{"unknownCredentialName":"memory-secret"}"#;
        let gateway_plaintext = r#"{"headers":{"X-Custom":"gateway-secret"}}"#;
        let sandbox_plaintext = r#"{"SANDBOX_VALUE":"sandbox-secret"}"#;

        database
            .upsert_terminal_backend(
                "backend-secure",
                "Secure local",
                "local",
                terminal_plaintext,
            )
            .unwrap();
        database
            .update_terminal_backend_status("backend-secure", "active")
            .unwrap();
        database
            .upsert_memory_provider(
                "memory-secure",
                "Secure memory",
                "custom",
                memory_plaintext,
                true,
            )
            .unwrap();
        database
            .upsert_tool_gateway_entry(
                "gateway-secure",
                "custom",
                "Secure gateway",
                gateway_plaintext,
                true,
            )
            .unwrap();
        database
            .insert_engine_run(
                "run-secure",
                None,
                None,
                None,
                "Secure config migration",
                None,
                "running",
                "executing",
                None,
                None,
                None,
                0,
                None,
                None,
                None,
            )
            .unwrap();
        database
            .insert_worker_sandbox(
                "sandbox-secure",
                "run-secure",
                None,
                Some("backend-secure"),
                "active",
                "native",
                "C:\\workspace",
                "C:\\workspace",
                "[]",
                None,
                true,
                true,
                true,
                false,
                false,
                false,
                Some(sandbox_plaintext),
                None,
            )
            .unwrap();

        let migrated = migrate_secure_config_rows(&database, &credentials).unwrap();
        assert_eq!(migrated.terminal_backends, 1);
        assert_eq!(migrated.memory_providers, 1);
        assert_eq!(migrated.tool_gateways, 1);
        assert_eq!(migrated.worker_sandboxes, 1);

        let terminal = database.list_terminal_backends().unwrap().remove(0);
        let memory = database.list_memory_providers().unwrap().remove(0);
        let gateway = database.list_tool_gateway_entries().unwrap().remove(0);
        let sandbox = database
            .get_worker_sandbox("sandbox-secure")
            .unwrap()
            .unwrap();
        for stored in [
            terminal.config_json.as_str(),
            memory.config_json.as_str(),
            gateway.config_json.as_str(),
            sandbox.env_json.as_deref().unwrap(),
        ] {
            assert!(stored.contains("$openCoworkCredential"));
            assert!(!stored.contains("secret"));
        }

        assert_eq!(
            secure_config::resolve(
                &credentials,
                secure_config::SecureConfigScope::MemoryProvider,
                &memory.id,
                &memory.config_json,
            ),
            Ok(memory_plaintext.to_string())
        );
        assert_eq!(
            secure_config::resolve(
                &credentials,
                secure_config::SecureConfigScope::WorkerSandbox,
                &sandbox.id,
                sandbox.env_json.as_deref().unwrap(),
            ),
            Ok(sandbox_plaintext.to_string())
        );

        let command = if cfg!(target_os = "windows") {
            "[Console]::Out.Write($env:MIGRATION_SECRET)"
        } else {
            "printf %s \"$MIGRATION_SECRET\""
        };
        let execution = terminal_backends::dispatch_exec(
            &database,
            &credentials,
            "backend-secure",
            command,
            None,
            Some(5_000),
        )
        .unwrap();
        assert_eq!(execution.exit_code, Some(0));
        assert_eq!(execution.stdout, "terminal-secret");

        let repeated = migrate_secure_config_rows(&database, &credentials).unwrap();
        assert_eq!(repeated.terminal_backends, 0);
        assert_eq!(repeated.memory_providers, 0);
        assert_eq!(repeated.tool_gateways, 0);
        assert_eq!(repeated.worker_sandboxes, 0);
    }

    #[test]
    fn gateway_context_never_exposes_stored_configuration() {
        let gateway = db::ToolGatewayRow {
            id: "gateway-1".to_string(),
            tool_type: "custom".to_string(),
            name: "private-tool".to_string(),
            config_json: r#"{"arbitrary":"must-not-reach-model"}"#.to_string(),
            enabled: true,
            created_at: "2026-07-10T00:00:00Z".to_string(),
            updated_at: "2026-07-10T00:00:00Z".to_string(),
        };

        let context = find_gateway_context("private-tool", &[gateway]).unwrap();

        assert!(context.contains("private-tool"));
        assert!(!context.contains("must-not-reach-model"));
        assert!(!context.contains("Configuration"));
    }
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
fn insights_summary(
    state: tauri::State<'_, Arc<Database>>,
) -> Result<insights::InsightsSummary, String> {
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
        .upsert_rpc_pipeline(
            &id,
            &name,
            description.as_deref(),
            &steps_json,
            zero_context.unwrap_or(false),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn pipeline_list(
    state: tauri::State<'_, Arc<Database>>,
) -> Result<Vec<db::RpcPipelineRow>, String> {
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
    credential_state: tauri::State<'_, Arc<credential_store::CredentialStore>>,
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
    secure_config::validate_json_document(&config_json)?;
    let existing = state
        .list_memory_providers()
        .map_err(|error| error.to_string())?;
    if existing
        .iter()
        .any(|entry| entry.name == name && entry.id != id)
    {
        return Err("memory provider name is already in use".to_string());
    }
    let previous = existing
        .iter()
        .find(|entry| entry.id == id)
        .map(|entry| entry.config_json.as_str());
    secure_config::replace(
        &credential_state,
        secure_config::SecureConfigScope::MemoryProvider,
        &id,
        &config_json,
        previous,
        |marker| {
            state
                .upsert_memory_provider(&id, &name, &provider_type, marker, enabled.unwrap_or(true))
                .map_err(|error| error.to_string())
        },
    )
    .map(|_| ())
}

#[tauri::command]
fn memory_provider_list(
    state: tauri::State<'_, Arc<Database>>,
) -> Result<Vec<db::MemoryProviderRow>, String> {
    state.list_memory_providers().map_err(|e| e.to_string())
}

#[tauri::command]
fn memory_provider_delete(
    state: tauri::State<'_, Arc<Database>>,
    credential_state: tauri::State<'_, Arc<credential_store::CredentialStore>>,
    id: String,
) -> Result<(), String> {
    let stored_config = state
        .list_memory_providers()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|entry| entry.id == id)
        .map(|entry| entry.config_json);
    state
        .delete_memory_provider(&id)
        .map_err(|e| e.to_string())?;
    secure_config::delete_reference(
        &credential_state,
        secure_config::SecureConfigScope::MemoryProvider,
        &id,
        stored_config.as_deref(),
    )
}

// -- Tool Gateway commands --------------------------------------------------

#[tauri::command]
fn tool_gateway_upsert(
    state: tauri::State<'_, Arc<Database>>,
    credential_state: tauri::State<'_, Arc<credential_store::CredentialStore>>,
    id: String,
    tool_type: String,
    name: String,
    config_json: String,
    enabled: Option<bool>,
) -> Result<(), String> {
    match tool_type.as_str() {
        "web_search" | "image_gen" | "tts" | "browser" | "code_exec" | "mcp" | "rest" | "grpc"
        | "custom" => {}
        other => return Err(format!("Unbekannter Tool-Typ: {}", other)),
    }
    secure_config::validate_json_document(&config_json)?;
    let existing = state
        .list_tool_gateway_entries()
        .map_err(|error| error.to_string())?;
    if existing
        .iter()
        .any(|entry| entry.name == name && entry.id != id)
    {
        return Err("tool gateway name is already in use".to_string());
    }
    let previous = existing
        .iter()
        .find(|entry| entry.id == id)
        .map(|entry| entry.config_json.as_str());
    secure_config::replace(
        &credential_state,
        secure_config::SecureConfigScope::ToolGateway,
        &id,
        &config_json,
        previous,
        |marker| {
            state
                .upsert_tool_gateway_entry(&id, &tool_type, &name, marker, enabled.unwrap_or(true))
                .map_err(|error| error.to_string())
        },
    )
    .map(|_| ())
}

#[tauri::command]
fn tool_gateway_list(
    state: tauri::State<'_, Arc<Database>>,
) -> Result<Vec<db::ToolGatewayRow>, String> {
    state.list_tool_gateway_entries().map_err(|e| e.to_string())
}

#[tauri::command]
fn tool_gateway_delete(
    state: tauri::State<'_, Arc<Database>>,
    credential_state: tauri::State<'_, Arc<credential_store::CredentialStore>>,
    id: String,
) -> Result<(), String> {
    let stored_config = state
        .list_tool_gateway_entries()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|entry| entry.id == id)
        .map(|entry| entry.config_json);
    state
        .delete_tool_gateway_entry(&id)
        .map_err(|e| e.to_string())?;
    secure_config::delete_reference(
        &credential_state,
        secure_config::SecureConfigScope::ToolGateway,
        &id,
        stored_config.as_deref(),
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SecureConfigMigrationResponse {
    terminal_backends: usize,
    memory_providers: usize,
    tool_gateways: usize,
    worker_sandboxes: usize,
}

fn migrate_secure_config_rows(
    state: &Arc<Database>,
    credential_state: &credential_store::CredentialStore,
) -> Result<SecureConfigMigrationResponse, String> {
    let mut response = SecureConfigMigrationResponse {
        terminal_backends: 0,
        memory_providers: 0,
        tool_gateways: 0,
        worker_sandboxes: 0,
    };

    for row in state
        .list_terminal_backends()
        .map_err(|error| error.to_string())?
    {
        if secure_config::is_reference(
            &row.config_json,
            secure_config::SecureConfigScope::TerminalBackend,
            &row.id,
        )? {
            continue;
        }
        secure_config::replace(
            credential_state,
            secure_config::SecureConfigScope::TerminalBackend,
            &row.id,
            &row.config_json,
            None,
            |marker| {
                state
                    .upsert_terminal_backend(&row.id, &row.name, &row.backend_type, marker)
                    .map_err(|error| error.to_string())
            },
        )?;
        response.terminal_backends += 1;
    }

    for row in state
        .list_memory_providers()
        .map_err(|error| error.to_string())?
    {
        if secure_config::is_reference(
            &row.config_json,
            secure_config::SecureConfigScope::MemoryProvider,
            &row.id,
        )? {
            continue;
        }
        secure_config::replace(
            credential_state,
            secure_config::SecureConfigScope::MemoryProvider,
            &row.id,
            &row.config_json,
            None,
            |marker| {
                state
                    .upsert_memory_provider(
                        &row.id,
                        &row.name,
                        &row.provider_type,
                        marker,
                        row.enabled,
                    )
                    .map_err(|error| error.to_string())
            },
        )?;
        response.memory_providers += 1;
    }

    for row in state
        .list_tool_gateway_entries()
        .map_err(|error| error.to_string())?
    {
        if secure_config::is_reference(
            &row.config_json,
            secure_config::SecureConfigScope::ToolGateway,
            &row.id,
        )? {
            continue;
        }
        secure_config::replace(
            credential_state,
            secure_config::SecureConfigScope::ToolGateway,
            &row.id,
            &row.config_json,
            None,
            |marker| {
                state
                    .upsert_tool_gateway_entry(
                        &row.id,
                        &row.tool_type,
                        &row.name,
                        marker,
                        row.enabled,
                    )
                    .map_err(|error| error.to_string())
            },
        )?;
        response.tool_gateways += 1;
    }

    for row in state
        .list_worker_sandboxes(100_000, None)
        .map_err(|error| error.to_string())?
    {
        let Some(env_json) = row.env_json.as_deref() else {
            continue;
        };
        if secure_config::is_reference(
            env_json,
            secure_config::SecureConfigScope::WorkerSandbox,
            &row.id,
        )? {
            continue;
        }
        secure_config::replace(
            credential_state,
            secure_config::SecureConfigScope::WorkerSandbox,
            &row.id,
            env_json,
            None,
            |marker| {
                state
                    .update_worker_sandbox_env(&row.id, Some(marker))
                    .map_err(|error| error.to_string())
            },
        )?;
        response.worker_sandboxes += 1;
    }

    Ok(response)
}

#[tauri::command]
fn secure_config_migrate(
    state: tauri::State<'_, Arc<Database>>,
    credential_state: tauri::State<'_, Arc<credential_store::CredentialStore>>,
) -> Result<SecureConfigMigrationResponse, String> {
    migrate_secure_config_rows(&state, &credential_state)
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

            let startup_audit_integrity = audit::integrity_report(&app_data_dir);
            if !startup_audit_integrity.permits_read() {
                log::error!("Audit log integrity verification failed during startup");
            }

            let database = Database::open(app_data_dir.clone()).expect("failed to open database");
            let recovery = database
                .recover_after_unclean_shutdown(&chrono::Utc::now().to_rfc3339())
                .expect("failed to recover interrupted runtime state");
            if recovery.total() > 0 {
                let details = serde_json::to_value(&recovery).ok();
                let _ =
                    audit::append_audit_event(app_data_dir, "runtime", "startup_recovery", details);
            }
            let shared_database = Arc::new(database);
            app.manage(shared_database.clone());
            app.manage(recovery);
            app.manage(Arc::new(credential_store::CredentialStore::native()));
            app.manage(WatchRegistry::default());
            app.manage(CrewExecutionRegistry::default());
            app.manage(ChatStreamRegistry::default());
            app.manage(TerminalSessionRegistry::default());
            app.manage(CrewPythonBridge::default());
            app.manage(ClaudeCodeBridge::new());
            configure_pdfium_search_paths(app.handle());
            start_scheduler_worker(app.handle().clone(), shared_database);

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ollama_health_check,
            generate_plan,
            chat_turn,
            chat_turn_stream,
            chat_turn_stream_cancel,
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
            shell_command_validate,
            exec_command,
            project_list,
            project_upsert,
            project_delete,
            project_resource_upsert,
            project_resource_delete,
            project_resource_set_enabled,
            project_attach_thread,
            project_detach_thread,
            db_save_thread,
            db_list_threads,
            db_update_thread_provider_settings,
            db_update_thread_permission_config,
            db_delete_thread,
            db_save_message,
            db_update_message_content,
            db_delete_messages,
            db_list_messages,
            db_save_task,
            db_update_task_status,
            db_list_tasks,
            work_task_list,
            work_task_upsert,
            work_task_delete,
            work_task_update_status,
            db_save_step,
            db_update_step,
            db_list_steps,
            execute_task,
            audit_event,
            credential_set,
            credential_get,
            credential_delete,
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
            office_detect_apps,
            office_open_document,
            office_export_preview,
            document_render_preview,
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
            engine_run_event_append,
            engine_run_event_list,
            engine_run_artifact_add,
            engine_run_artifact_list,
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
            memory_mutate,
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
            session_memory_snapshot,
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
            terminal_create,
            terminal_write,
            terminal_resize,
            terminal_interrupt,
            terminal_kill,
            terminal_close,
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
            secure_config_migrate,
            connector_test_reachability,
            gateway_status,
            gateway_health,
            gateway_probe,
            gateway_logs_tail,
            support_bundle_create,
            startup_recovery_status,
            runtime_provider_mapping_resolve,
            crew_provider_health_check,
            crew_provider_models_list,
            openai_compatible_chat_completion,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
