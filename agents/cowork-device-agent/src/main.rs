use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env, fs,
    net::{IpAddr, SocketAddr},
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use cowork_contracts::{
    AppendRunEventRequest, ApprovalRequest, ApprovalState, BeginSnapshotUploadRequest, Capability,
    CapabilityDescriptor, CompleteRunRequest, CreateApprovalRequest, CreateCheckpointRequest,
    CreateInputRequest, ExecutorClientMessage, ExecutorHeartbeat, ExecutorKind,
    ExecutorRegistration, ExecutorServerMessage, FailRunRequest, InputRequestState,
    PersonalDeviceRemoteControlMode, PullSyncChangesResponse, PushSyncChangesRequest,
    PushSyncChangesResponse, RunError, RunEvent, RunEventKind, RunInputRequest, RunLease,
    RunRecord, RunState, SnapshotManifest, SnapshotUploadChunk, SnapshotUploadFile,
    SnapshotUploadSession, SyncApplyStatus, SyncChange, SyncOperation, SCHEMA_VERSION,
};
use cowork_runtime::{
    crew::{prepare_crew_request, CrewModelConfig},
    AgentRuntime, ModelConfig as AgentModelConfig, RuntimeHost, ToolDefinition, ToolInvocation,
    ToolOutput,
};
use futures_util::{SinkExt, StreamExt};
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue as ReqwestHeaderValue, ACCEPT, CONTENT_TYPE},
    redirect::Policy,
    Client, Method, Url,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        client::IntoClientRequest,
        http::{header::AUTHORIZATION, HeaderValue},
        Message,
    },
};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

mod crew;
mod network_safety;
mod windows_desktop;

const SNAPSHOT_CHUNK_BYTES: usize = 16 * 1024 * 1024;
const MAX_EXECUTOR_MCP_BINDINGS: usize = 64;
const MAX_EXECUTOR_MCP_FILE_BYTES: u64 = 512 * 1024;
const MAX_MCP_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
const LATEST_HTTP_MCP_PROTOCOL_VERSION: &str = "2025-11-25";
const SUPPORTED_HTTP_MCP_PROTOCOL_VERSIONS: [&str; 3] = ["2025-03-26", "2025-06-18", "2025-11-25"];

#[derive(Debug)]
struct LeaseExecution {
    result: Value,
    result_snapshot_manifest_id: Option<Uuid>,
    result_diff_summary: Value,
}

#[derive(Debug, Clone)]
struct WorkspaceInventory {
    root: PathBuf,
    files: Vec<SnapshotUploadFile>,
    fingerprints: BTreeMap<String, String>,
    total_bytes: u64,
}

#[derive(Clone)]
struct Config {
    server_url: String,
    token: String,
    executor_id: Uuid,
    kind: ExecutorKind,
    pool_id: Option<Uuid>,
    display_name: String,
    capabilities: Vec<CapabilityDescriptor>,
    personal_device_remote_control: Option<PersonalDeviceRemoteControlMode>,
    model_base_url: Option<String>,
    model_api_key: Option<String>,
    model_name: String,
    poll_interval: Duration,
    workspace_root: PathBuf,
    local_daemon: Option<LocalDaemonClient>,
    mcp_bindings: Vec<ExecutorMcpBinding>,
    crew_runtime: Option<crew::CrewRuntimeConfig>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ExecutorMcpTransport {
    #[default]
    Stdio,
    StreamableHttp,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ExecutorMcpBinding {
    name: String,
    #[serde(default)]
    transport: ExecutorMcpTransport,
    #[serde(default)]
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    environment: BTreeMap<String, String>,
    #[serde(default)]
    url: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
}

impl ExecutorMcpBinding {
    fn secret_values(&self) -> impl Iterator<Item = &String> {
        match self.transport {
            ExecutorMcpTransport::Stdio => self.environment.values(),
            ExecutorMcpTransport::StreamableHttp => self.headers.values(),
        }
    }
}

struct ManagedMcpProcessJob {
    #[cfg(windows)]
    handle: isize,
}

impl ManagedMcpProcessJob {
    fn attach(child: &tokio::process::Child) -> Result<Self> {
        #[cfg(windows)]
        {
            use std::{ffi::c_void, mem::size_of, ptr};
            use windows_sys::Win32::{
                Foundation::CloseHandle,
                System::{
                    JobObjects::{
                        AssignProcessToJobObject, CreateJobObjectW,
                        JobObjectExtendedLimitInformation, SetInformationJobObject,
                        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                    },
                    Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE},
                },
            };
            let process_id = child.id().context("MCP server has no process ID")?;
            unsafe {
                let job = CreateJobObjectW(ptr::null(), ptr::null());
                if job.is_null() {
                    bail!("failed to create the MCP process job");
                }
                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as *const c_void,
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ) == 0
                {
                    CloseHandle(job);
                    bail!("failed to configure the MCP process job");
                }
                let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, process_id);
                if process.is_null() {
                    CloseHandle(job);
                    bail!("failed to open the MCP server for job assignment");
                }
                let assigned = AssignProcessToJobObject(job, process);
                CloseHandle(process);
                if assigned == 0 {
                    CloseHandle(job);
                    bail!("failed to assign the MCP server to its lifecycle job");
                }
                Ok(Self {
                    handle: job as isize,
                })
            }
        }
        #[cfg(not(windows))]
        {
            let _ = child;
            Ok(Self {})
        }
    }

    fn close(self) {}
}

#[cfg(windows)]
impl Drop for ManagedMcpProcessJob {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle as _);
        }
    }
}

#[derive(Clone)]
struct ControlPlaneClient {
    http: Client,
    server_url: String,
    token: String,
    executor_id: Uuid,
}

#[derive(Debug, Clone)]
struct LocalDaemonClient {
    endpoint: String,
    token: String,
}

#[derive(Debug, Serialize)]
struct LocalDaemonRequest<'a> {
    id: Uuid,
    token: &'a str,
    method: &'a str,
    params: &'a Value,
}

#[derive(Debug, Deserialize)]
struct LocalDaemonResponse {
    id: Value,
    result: Option<Value>,
    error: Option<LocalDaemonError>,
}

#[derive(Debug, Deserialize)]
struct LocalDaemonError {
    code: String,
    message: String,
}

#[derive(Debug, Deserialize)]
struct LocalSyncState {
    local_cursor: i64,
    remote_cursor: i64,
}

#[derive(Debug, Deserialize)]
struct LocalSyncChanges {
    changes: Vec<LocalSyncChange>,
}

#[derive(Debug, Deserialize)]
struct LocalSyncChange {
    cursor: i64,
    entity_type: String,
    entity_id: String,
    revision: i64,
    operation: String,
    entity: LocalSyncEntity,
    created_at: String,
}

#[derive(Debug, Deserialize)]
struct LocalSyncEntity {
    payload: Value,
    tombstone: bool,
}

#[derive(Debug, Serialize)]
struct ChatCompletionRequest<'a> {
    model: &'a str,
    messages: [ChatMessage<'a>; 1],
}

#[derive(Debug, Serialize)]
struct ChatMessage<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<ChatUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ChatResponseMessage {
    content: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ChatUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

struct ModelCallResult {
    content: String,
    usage: Option<ChatUsage>,
}

#[tokio::main]
async fn main() -> Result<()> {
    if env::args().nth(1).as_deref() == Some("executor-mcp-tool") {
        if env::args().nth(2).is_some() {
            bail!("executor-mcp-tool does not accept additional arguments");
        }
        return run_executor_mcp_tool_bridge().await;
    }
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "cowork_device_agent=info".into()),
        )
        .init();
    let config = Config::from_env()?;
    if let Some(runtime) = &config.crew_runtime {
        crew::verify_runtime(runtime).await?;
    }
    let client = ControlPlaneClient {
        http: Client::builder()
            .timeout(Duration::from_secs(20 * 60))
            .build()?,
        server_url: config.server_url.clone(),
        token: config.token.clone(),
        executor_id: config.executor_id,
    };
    let mut labels = BTreeMap::from([
        ("os".to_owned(), env::consts::OS.to_owned()),
        ("arch".to_owned(), env::consts::ARCH.to_owned()),
    ]);
    if let Some(mode) = config.personal_device_remote_control {
        labels.insert(
            "local_remote_control_mode".to_owned(),
            personal_remote_control_mode_name(mode).to_owned(),
        );
    }
    if config.local_daemon.is_some() {
        labels.insert("local_runtime_bridge".to_owned(), "enabled".to_owned());
    }
    let registration = ExecutorRegistration {
        schema_version: SCHEMA_VERSION,
        executor_id: config.executor_id,
        kind: config.kind,
        pool_id: config.pool_id,
        owner_user_id: None,
        display_name: config.display_name.clone(),
        protocol_version: SCHEMA_VERSION,
        capabilities: config.capabilities.clone(),
        labels,
        personal_device_remote_control: config.personal_device_remote_control,
        max_concurrent_runs: 1,
    };

    let registered = loop {
        if let Some(daemon) = &config.local_daemon {
            if let Err(error) = daemon.verify_device(config.executor_id).await {
                tracing::warn!(?error, "local daemon bridge is unavailable; retrying");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        }
        match client.register(&registration).await {
            Ok(record) => break record,
            Err(error) => {
                tracing::warn!(?error, "executor registration failed; retrying");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    };
    let sync_user_id = registered.registration.owner_user_id;
    if config.kind == ExecutorKind::PersonalDevice && sync_user_id.is_none() {
        bail!("personal device registration did not return its owner identity");
    }
    tracing::info!(executor_id = %config.executor_id, kind = ?config.kind, "executor registered");

    loop {
        if let Err(error) = run_websocket(&client, &config, sync_user_id).await {
            tracing::warn!(?error, "executor WebSocket disconnected; retrying");
        }
        tokio::time::sleep(config.poll_interval).await;
    }
}

impl Config {
    fn from_env() -> Result<Self> {
        let kind = match value_or("COWORK_AGENT_KIND", "personal_device").as_str() {
            "personal_device" => ExecutorKind::PersonalDevice,
            "managed_windows" => ExecutorKind::ManagedWindows,
            other => {
                bail!("COWORK_AGENT_KIND must be personal_device or managed_windows; got {other}")
            }
        };
        if kind == ExecutorKind::ManagedWindows && !cfg!(windows) {
            bail!("managed_windows agents can only run on Windows");
        }
        let pool_id = env::var("COWORK_EXECUTOR_POOL_ID")
            .ok()
            .map(|value| value.parse().context("invalid COWORK_EXECUTOR_POOL_ID"))
            .transpose()?;
        if kind == ExecutorKind::ManagedWindows && pool_id.is_none() {
            bail!("managed Windows agents require COWORK_EXECUTOR_POOL_ID");
        }
        let model_base_url = optional("COWORK_MODEL_BASE_URL");
        let default_capability = if model_base_url.is_some() {
            "model.external"
        } else {
            ""
        };
        let mcp_bindings = load_executor_mcp_bindings(kind)?;
        let mut capabilities: Vec<CapabilityDescriptor> =
            value_or("COWORK_AGENT_CAPABILITIES", default_capability)
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|name| CapabilityDescriptor {
                    schema_version: SCHEMA_VERSION,
                    name: Capability::from(name),
                    version: env!("CARGO_PKG_VERSION").to_owned(),
                    attributes: BTreeMap::new(),
                })
                .collect();
        let advertises_crew = capabilities
            .iter()
            .any(|capability| capability.name.0 == "crew.python");
        let crew_runtime = crew::runtime_from_env(kind, advertises_crew, model_base_url.is_some())?;
        if let Some(capability) = capabilities
            .iter_mut()
            .find(|capability| capability.name.0 == "crew.python")
        {
            if kind == ExecutorKind::ManagedWindows {
                capability.attributes.insert(
                    "adapter".to_owned(),
                    Value::String("pinned_python".to_owned()),
                );
                capability.attributes.insert(
                    "crewai_version".to_owned(),
                    Value::String(crew::EXPECTED_CREWAI_VERSION.to_owned()),
                );
            }
        }
        let mcp_capability = capabilities
            .iter_mut()
            .find(|capability| capability.name.0 == "tool.mcp.invoke");
        if mcp_bindings.is_empty() {
            if kind == ExecutorKind::ManagedWindows && mcp_capability.is_some() {
                bail!(
                    "managed Windows executors may advertise tool.mcp.invoke only with COWORK_MCP_BINDINGS_FILE"
                );
            }
        } else {
            if model_base_url.is_none() {
                bail!("managed Windows MCP execution requires COWORK_MODEL_BASE_URL");
            }
            let server_names = mcp_bindings
                .iter()
                .map(|binding| Value::String(binding.name.clone()))
                .collect::<Vec<_>>();
            if let Some(capability) = mcp_capability {
                capability
                    .attributes
                    .insert("server_names".to_owned(), Value::Array(server_names));
                capability.attributes.insert(
                    "binding_source".to_owned(),
                    Value::String("executor_local_file".to_owned()),
                );
            } else {
                capabilities.push(CapabilityDescriptor {
                    schema_version: SCHEMA_VERSION,
                    name: Capability::from("tool.mcp.invoke"),
                    version: env!("CARGO_PKG_VERSION").to_owned(),
                    attributes: BTreeMap::from([
                        ("server_names".to_owned(), Value::Array(server_names)),
                        (
                            "binding_source".to_owned(),
                            Value::String("executor_local_file".to_owned()),
                        ),
                    ]),
                });
            }
        }
        let advertises_windows_desktop = capabilities
            .iter()
            .any(|capability| capability.name.0 == "desktop.windows");
        let advertises_linux_desktop = capabilities
            .iter()
            .any(|capability| capability.name.0 == "desktop.linux");
        if advertises_windows_desktop && !cfg!(windows) {
            bail!("desktop.windows can only be advertised by a Windows executor");
        }
        if advertises_linux_desktop && !cfg!(target_os = "linux") {
            bail!("desktop.linux can only be advertised by a Linux executor");
        }
        if (advertises_windows_desktop || advertises_linux_desktop) && !windows_desktop::available()
        {
            bail!("the advertised desktop capability is unavailable in the current interactive session");
        }
        let personal_device_remote_control = if kind == ExecutorKind::PersonalDevice {
            Some(parse_personal_remote_control_mode(&value_or(
                "COWORK_PERSONAL_REMOTE_CONTROL",
                "confirm_each_session",
            ))?)
        } else {
            None
        };
        let local_daemon = match optional("COWORK_LOCAL_DAEMON_IPC_ENDPOINT") {
            Some(endpoint) if kind == ExecutorKind::PersonalDevice => Some(LocalDaemonClient {
                endpoint,
                token: required_secret("COWORK_LOCAL_DAEMON_IPC_TOKEN")?,
            }),
            Some(_) => bail!("the local daemon bridge is only valid for personal devices"),
            None if optional("COWORK_LOCAL_DAEMON_IPC_TOKEN").is_some()
                || optional("COWORK_LOCAL_DAEMON_IPC_TOKEN_FILE").is_some() =>
            {
                bail!("COWORK_LOCAL_DAEMON_IPC_ENDPOINT is required with a daemon IPC token")
            }
            None => None,
        };
        Ok(Self {
            server_url: validated_server_url(&required("COWORK_SERVER_URL")?)?,
            token: required_secret("COWORK_AGENT_TOKEN")?,
            executor_id: required("COWORK_EXECUTOR_ID")?
                .parse()
                .context("invalid COWORK_EXECUTOR_ID")?,
            kind,
            pool_id,
            display_name: value_or("COWORK_EXECUTOR_NAME", "Open Cowork Device"),
            capabilities,
            personal_device_remote_control,
            model_base_url,
            model_api_key: optional("COWORK_MODEL_API_KEY"),
            model_name: value_or("COWORK_MODEL_NAME", "local-model"),
            poll_interval: Duration::from_millis(
                value_or("COWORK_AGENT_POLL_MS", "2000")
                    .parse()
                    .context("invalid COWORK_AGENT_POLL_MS")?,
            ),
            workspace_root: PathBuf::from(value_or(
                "COWORK_AGENT_WORKSPACE_ROOT",
                ".cowork-agent-workspaces",
            )),
            local_daemon,
            mcp_bindings,
            crew_runtime,
        })
    }
}

impl ControlPlaneClient {
    async fn register(
        &self,
        registration: &ExecutorRegistration,
    ) -> Result<cowork_contracts::ExecutorRecord> {
        self.request::<ExecutorRegistration, cowork_contracts::ExecutorRecord>(
            Method::POST,
            &format!("/api/v1/agent/executors/{}/register", self.executor_id),
            Some(registration),
        )
        .await
    }

    async fn push_sync_changes(&self, changes: Vec<SyncChange>) -> Result<PushSyncChangesResponse> {
        self.request(
            Method::POST,
            &format!("/api/v1/agent/executors/{}/sync/changes", self.executor_id),
            Some(&PushSyncChangesRequest { changes }),
        )
        .await
    }

    async fn pull_sync_changes(&self, after: i64) -> Result<PullSyncChangesResponse> {
        self.request::<(), _>(
            Method::GET,
            &format!(
                "/api/v1/agent/executors/{}/sync/changes?after={}&limit=100",
                self.executor_id,
                after.max(0)
            ),
            None,
        )
        .await
    }

    async fn request<B, R>(&self, method: Method, path: &str, body: Option<&B>) -> Result<R>
    where
        B: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let mut request = self
            .http
            .request(method, format!("{}{}", self.server_url, path))
            .bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .context("control plane request failed")?;
        let status = response.status();
        let bytes = response.bytes().await?;
        if !status.is_success() {
            bail!(
                "control plane returned {status}: {}",
                String::from_utf8_lossy(&bytes)
            );
        }
        serde_json::from_slice(&bytes).context("invalid control plane response")
    }

    fn leased_request(
        &self,
        method: Method,
        path: &str,
        lease_token: Uuid,
    ) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{}", self.server_url, path))
            .bearer_auth(&self.token)
            .header("x-cowork-lease-token", lease_token.to_string())
    }

    async fn run_snapshot(&self, lease: &RunLease) -> Result<SnapshotManifest> {
        let path = format!(
            "/api/v1/agent/executors/{}/runs/{}/snapshot",
            self.executor_id, lease.run.spec.id
        );
        let response = self
            .leased_request(Method::GET, &path, lease.lease_token)
            .send()
            .await
            .context("snapshot manifest request failed")?;
        decode_response(response).await
    }

    async fn snapshot_chunk(&self, lease: &RunLease, digest: &str) -> Result<Vec<u8>> {
        let path = format!(
            "/api/v1/agent/executors/{}/runs/{}/snapshot/chunks/{digest}",
            self.executor_id, lease.run.spec.id
        );
        let response = self
            .leased_request(Method::GET, &path, lease.lease_token)
            .send()
            .await
            .with_context(|| format!("snapshot chunk {digest} request failed"))?;
        decode_bytes(response).await
    }

    async fn begin_result_snapshot(
        &self,
        lease: &RunLease,
        inventory: &WorkspaceInventory,
    ) -> Result<SnapshotUploadSession> {
        let path = format!(
            "/api/v1/agent/executors/{}/runs/{}/result-snapshot",
            self.executor_id, lease.run.spec.id
        );
        let response = self
            .leased_request(Method::POST, &path, lease.lease_token)
            .json(&BeginSnapshotUploadRequest {
                project_id: lease.run.spec.project_id,
                total_bytes: inventory.total_bytes,
                files: inventory.files.clone(),
                expires_at: None,
            })
            .send()
            .await
            .context("result snapshot initialization failed")?;
        decode_response(response).await
    }

    async fn existing_result_snapshot(&self, lease: &RunLease) -> Result<Option<SnapshotManifest>> {
        let path = format!(
            "/api/v1/agent/executors/{}/runs/{}/result-snapshot",
            self.executor_id, lease.run.spec.id
        );
        let response = self
            .leased_request(Method::GET, &path, lease.lease_token)
            .send()
            .await
            .context("result snapshot recovery lookup failed")?;
        decode_response(response).await
    }

    async fn upload_result_chunk(
        &self,
        lease: &RunLease,
        manifest_id: Uuid,
        digest: &str,
        bytes: Vec<u8>,
    ) -> Result<()> {
        let path = format!(
            "/api/v1/agent/executors/{}/runs/{}/result-snapshot/{manifest_id}/chunks/{digest}",
            self.executor_id, lease.run.spec.id
        );
        let response = self
            .leased_request(Method::PUT, &path, lease.lease_token)
            .header("content-type", "application/octet-stream")
            .body(bytes)
            .send()
            .await
            .with_context(|| format!("result snapshot chunk {digest} upload failed"))?;
        let _: cowork_contracts::SnapshotChunkReceipt = decode_response(response).await?;
        Ok(())
    }

    async fn commit_result_snapshot(
        &self,
        lease: &RunLease,
        manifest_id: Uuid,
    ) -> Result<SnapshotManifest> {
        let path = format!(
            "/api/v1/agent/executors/{}/runs/{}/result-snapshot/{manifest_id}/commit",
            self.executor_id, lease.run.spec.id
        );
        let response = self
            .leased_request(Method::POST, &path, lease.lease_token)
            .send()
            .await
            .context("result snapshot commit failed")?;
        decode_response(response).await
    }

    async fn upload_artifact(
        &self,
        lease: &RunLease,
        source_event_id: Option<Uuid>,
        path: &str,
        source: &str,
        bytes: Vec<u8>,
    ) -> Result<Value> {
        let mut url = reqwest::Url::parse(&format!(
            "{}/api/v1/agent/executors/{}/runs/{}/artifacts",
            self.server_url, self.executor_id, lease.run.spec.id
        ))?;
        url.query_pairs_mut()
            .append_pair("path", path)
            .append_pair("source", source);
        if let Some(source_event_id) = source_event_id {
            url.query_pairs_mut()
                .append_pair("source_event_id", &source_event_id.to_string());
        }
        let response = self
            .http
            .post(url)
            .bearer_auth(&self.token)
            .header("x-cowork-lease-token", lease.lease_token.to_string())
            .header("content-type", "application/octet-stream")
            .body(bytes)
            .send()
            .await
            .context("artifact upload failed")?;
        decode_response(response).await
    }

    async fn append_event(&self, lease: &RunLease, event: &RunEvent) -> Result<()> {
        let _: RunEvent = self
            .request(
                Method::POST,
                &format!(
                    "/api/v1/agent/executors/{}/runs/{}/events",
                    self.executor_id, lease.run.spec.id
                ),
                Some(&AppendRunEventRequest {
                    lease_token: lease.lease_token,
                    source_event_id: Some(event.event_id),
                    kind: event.kind,
                    payload: event.payload.clone(),
                }),
            )
            .await?;
        Ok(())
    }

    async fn create_checkpoint(
        &self,
        lease: &RunLease,
        source_checkpoint_id: Uuid,
        safe_to_resume: bool,
        executor_state: Value,
    ) -> Result<()> {
        let _: cowork_contracts::RunCheckpoint = self
            .request(
                Method::POST,
                &format!(
                    "/api/v1/agent/executors/{}/runs/{}/checkpoints",
                    self.executor_id, lease.run.spec.id
                ),
                Some(&CreateCheckpointRequest {
                    lease_token: lease.lease_token,
                    source_checkpoint_id: Some(source_checkpoint_id),
                    safe_to_resume,
                    executor_state,
                }),
            )
            .await?;
        Ok(())
    }

    async fn await_approval(
        &self,
        lease: &RunLease,
        source_request_id: Uuid,
        requested_action: Value,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<bool> {
        let approval: ApprovalRequest = self
            .request(
                Method::POST,
                &format!(
                    "/api/v1/agent/executors/{}/runs/{}/approvals",
                    self.executor_id, lease.run.spec.id
                ),
                Some(&CreateApprovalRequest {
                    lease_token: lease.lease_token,
                    source_request_id: Some(source_request_id),
                    requested_action,
                    expires_at,
                }),
            )
            .await?;
        loop {
            let current: ApprovalRequest = self
                .request::<(), _>(
                    Method::GET,
                    &format!(
                        "/api/v1/agent/executors/{}/runs/{}/approvals/{}",
                        self.executor_id, lease.run.spec.id, approval.id
                    ),
                    None,
                )
                .await?;
            match current.state {
                ApprovalState::Pending => tokio::time::sleep(Duration::from_secs(1)).await,
                ApprovalState::Approved => return Ok(true),
                ApprovalState::Rejected | ApprovalState::Expired => return Ok(false),
            }
        }
    }

    async fn await_input(
        &self,
        lease: &RunLease,
        source_request_id: Uuid,
        prompt: Value,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Option<Value>> {
        let input: RunInputRequest = self
            .request(
                Method::POST,
                &format!(
                    "/api/v1/agent/executors/{}/runs/{}/input-requests",
                    self.executor_id, lease.run.spec.id
                ),
                Some(&CreateInputRequest {
                    lease_token: lease.lease_token,
                    source_request_id: Some(source_request_id),
                    prompt,
                    expires_at,
                }),
            )
            .await?;
        loop {
            let current: RunInputRequest = self
                .request::<(), _>(
                    Method::GET,
                    &format!(
                        "/api/v1/agent/executors/{}/runs/{}/input-requests/{}",
                        self.executor_id, lease.run.spec.id, input.id
                    ),
                    None,
                )
                .await?;
            match current.state {
                InputRequestState::Pending => tokio::time::sleep(Duration::from_secs(1)).await,
                InputRequestState::Submitted => return Ok(current.response),
                InputRequestState::Expired => return Ok(None),
            }
        }
    }
}

impl LocalDaemonClient {
    async fn verify_device(&self, executor_id: Uuid) -> Result<()> {
        let health = self.call("health", json!({})).await?;
        let device_id: Uuid = health
            .get("device_id")
            .and_then(Value::as_str)
            .context("local daemon health is missing device_id")?
            .parse()
            .context("local daemon returned an invalid device_id")?;
        if device_id != executor_id {
            bail!("local daemon device ID {device_id} does not match executor ID {executor_id}");
        }
        Ok(())
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value> {
        #[cfg(windows)]
        {
            use tokio::net::windows::named_pipe::ClientOptions;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                match ClientOptions::new().open(&self.endpoint) {
                    Ok(stream) => return self.call_stream(stream, method, params).await,
                    Err(error) if tokio::time::Instant::now() < deadline => {
                        tracing::debug!(?error, "waiting for local daemon named pipe");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                    Err(error) => {
                        return Err(error).context("failed to connect to local daemon named pipe")
                    }
                }
            }
        }
        #[cfg(unix)]
        {
            let stream = tokio::net::UnixStream::connect(&self.endpoint)
                .await
                .context("failed to connect to local daemon Unix socket")?;
            self.call_stream(stream, method, params).await
        }
        #[cfg(not(any(windows, unix)))]
        {
            let _ = (method, params);
            bail!("the local daemon bridge is unsupported on this platform")
        }
    }

    async fn call_stream<S>(&self, stream: S, method: &str, params: Value) -> Result<Value>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let id = Uuid::new_v4();
        let request = LocalDaemonRequest {
            id,
            token: &self.token,
            method,
            params: &params,
        };
        let encoded = serde_json::to_vec(&request)?;
        if encoded.len() > 16 * 1024 * 1024 {
            bail!("local daemon request exceeds 16 MiB");
        }
        let (reader, mut writer) = tokio::io::split(stream);
        writer.write_all(&encoded).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
        let mut line = String::new();
        BufReader::new(reader).read_line(&mut line).await?;
        if line.len() > 16 * 1024 * 1024 {
            bail!("local daemon response exceeds 16 MiB");
        }
        let response: LocalDaemonResponse =
            serde_json::from_str(line.trim()).context("invalid local daemon response")?;
        if response.id != Value::String(id.to_string()) {
            bail!("local daemon response ID does not match its request")
        }
        if let Some(error) = response.error {
            bail!("local daemon {}: {}", error.code, error.message);
        }
        response
            .result
            .context("local daemon returned neither a result nor an error")
    }
}

async fn decode_response<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    let status = response.status();
    let bytes = response.bytes().await?;
    if !status.is_success() {
        bail!(
            "control plane returned {status}: {}",
            String::from_utf8_lossy(&bytes)
        );
    }
    serde_json::from_slice(&bytes).context("invalid control plane response")
}

async fn decode_bytes(response: reqwest::Response) -> Result<Vec<u8>> {
    let status = response.status();
    let bytes = response.bytes().await?;
    if !status.is_success() {
        bail!(
            "control plane returned {status}: {}",
            String::from_utf8_lossy(&bytes)
        );
    }
    Ok(bytes.to_vec())
}

async fn run_websocket(
    client: &ControlPlaneClient,
    config: &Config,
    sync_user_id: Option<Uuid>,
) -> Result<()> {
    let ws_url = websocket_url(&client.server_url, client.executor_id)?;
    let mut request = ws_url.into_client_request()?;
    request.headers_mut().insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", client.token))?,
    );
    let (mut socket, _) = connect_async(request)
        .await
        .context("failed to connect executor WebSocket")?;
    let (result_tx, mut result_rx) = mpsc::channel::<ExecutorClientMessage>(4);
    let mut active_lease: Option<RunLease> = None;
    let mut active_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut desktop_tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    // Viewing and controlling are cached separately. A local approval to view a
    // screen must never silently become permission to inject input later.
    let personal_desktop_decisions =
        std::sync::Arc::new(Mutex::new(HashMap::<(Uuid, bool), bool>::new()));
    let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let sync_task = if config.kind == ExecutorKind::PersonalDevice {
        config.local_daemon.clone().map(|daemon| {
            let client = client.clone();
            let user_id =
                sync_user_id.expect("personal device owner was validated at registration");
            tokio::spawn(async move { metadata_sync_loop(client, daemon, user_id).await })
        })
    } else {
        None
    };

    let outcome = async {
        loop {
            tokio::select! {
                _ = heartbeat.tick() => {
                    desktop_tasks.retain(|task| !task.is_finished());
                    let active_run_ids = active_lease
                        .as_ref()
                        .map(|lease| vec![lease.run.spec.id])
                        .unwrap_or_default();
                    send_executor_message(&mut socket, &ExecutorClientMessage::Heartbeat {
                        heartbeat: ExecutorHeartbeat {
                            protocol_version: SCHEMA_VERSION,
                            active_run_ids,
                            health: json!({"status": "ready"}),
                        },
                    }).await?;
                    if let Some(lease) = &active_lease {
                        send_executor_message(&mut socket, &ExecutorClientMessage::LeaseHeartbeat {
                            run_id: lease.run.spec.id,
                            lease_token: lease.lease_token,
                        }).await?;
                    }
                }
                result = result_rx.recv(), if active_lease.is_some() => {
                    let Some(result) = result else { bail!("executor result channel closed") };
                    send_executor_message(&mut socket, &result).await?;
                    active_lease = None;
                    for task in desktop_tasks.drain(..) {
                        task.abort();
                    }
                    if let Some(task) = active_task.take() {
                        let _ = task.await;
                    }
                }
                incoming = socket.next() => {
                    let incoming = incoming.context("executor WebSocket closed")??;
                    match incoming {
                        Message::Text(text) => {
                            let message: ExecutorServerMessage = serde_json::from_str(&text)
                                .context("invalid server WebSocket message")?;
                            match message {
                                ExecutorServerMessage::Hello { schema_version, executor_id, .. } => {
                                    cowork_contracts::ensure_compatible(schema_version)?;
                                    if executor_id != config.executor_id {
                                        bail!("server WebSocket identity does not match this executor");
                                    }
                                }
                                ExecutorServerMessage::Lease { lease } => {
                                    let lease = *lease;
                                    if active_lease.is_some() {
                                        bail!("server assigned a second lease to a busy executor");
                                    }
                                    tracing::info!(run_id = %lease.run.spec.id, "received run lease");
                                    active_lease = Some(lease.clone());
                                    let config = config.clone();
                                    let client = client.clone();
                                    let tx = result_tx.clone();
                                    active_task = Some(tokio::spawn(async move {
                                        let run_id = lease.run.spec.id;
                                        let outcome = execute_lease(&client, &config, &lease).await;
                                        let cleanup = cleanup_run_workspace(&config, &lease).await;
                                        let outcome = match (outcome, cleanup) {
                                            (Ok(value), Ok(())) => Ok(value),
                                            (Ok(_), Err(error)) => Err(error.context("executor workspace cleanup failed")),
                                            (Err(error), Ok(())) => Err(error),
                                            (Err(error), Err(cleanup)) => Err(error.context(format!("executor workspace cleanup also failed: {cleanup:#}"))),
                                        };
                                        let message = match outcome {
                                            Ok(execution) => ExecutorClientMessage::Complete {
                                                run_id,
                                                request: CompleteRunRequest {
                                                    lease_token: lease.lease_token,
                                                    result: execution.result,
                                                    result_snapshot_manifest_id: execution.result_snapshot_manifest_id,
                                                    result_diff_summary: execution.result_diff_summary,
                                                },
                                            },
                                            Err(error) => ExecutorClientMessage::Fail {
                                                run_id,
                                                request: FailRunRequest {
                                                    lease_token: lease.lease_token,
                                                    error: RunError {
                                                        code: "device_operation_failed".to_owned(),
                                                        message: error.to_string(),
                                                        retryable: false,
                                                        details: Value::Null,
                                                    },
                                                },
                                            },
                                        };
                                        let _ = tx.send(message).await;
                                    }));
                                }
                                ExecutorServerMessage::DesktopStreamRequested {
                                    run_id,
                                    session_id,
                                    stream_id,
                                    control,
                                } => {
                                    let leased = active_lease
                                        .as_ref()
                                        .is_some_and(|lease| lease.run.spec.id == run_id);
                                    let desktop_capability = if cfg!(windows) {
                                        Some("desktop.windows")
                                    } else if cfg!(target_os = "linux") {
                                        Some("desktop.linux")
                                    } else {
                                        None
                                    };
                                    let capable = windows_desktop::available()
                                        && desktop_capability.is_some_and(|required| {
                                            config.capabilities.iter().any(|capability| {
                                                capability.name.0 == required
                                            })
                                        });
                                    if !leased || !capable {
                                        tracing::warn!(
                                            %run_id,
                                            %session_id,
                                            %stream_id,
                                            leased,
                                            capable,
                                            "refusing an unauthorized desktop stream request"
                                        );
                                        continue;
                                    }
                                    let client = client.clone();
                                    let config = config.clone();
                                    let decisions = personal_desktop_decisions.clone();
                                    desktop_tasks.push(tokio::spawn(async move {
                                        let allowed = authorize_desktop_stream(
                                            &config,
                                            &decisions,
                                            run_id,
                                            session_id,
                                            control,
                                        )
                                        .await;
                                        let allowed = match allowed {
                                            Ok(allowed) => allowed,
                                            Err(error) => {
                                                tracing::warn!(?error, %run_id, %session_id, "local desktop authorization failed");
                                                false
                                            }
                                        };
                                        if !allowed {
                                            tracing::info!(%run_id, %session_id, %stream_id, "local user denied desktop access");
                                            if let Err(error) = reject_desktop_stream(&client, stream_id).await {
                                                tracing::warn!(?error, %run_id, %session_id, %stream_id, "failed to reject desktop stream promptly");
                                            }
                                            return;
                                        }
                                        if let Err(error) = open_desktop_stream(&client, stream_id, control).await {
                                            tracing::warn!(
                                                ?error,
                                                %run_id,
                                                %session_id,
                                                %stream_id,
                                                "personal or managed desktop stream failed"
                                            );
                                        }
                                    }));
                                }
                                ExecutorServerMessage::Ack { operation, run_id } => {
                                    tracing::debug!(%operation, ?run_id, "executor operation acknowledged");
                                }
                                ExecutorServerMessage::Error { code, message, run_id } => {
                                    tracing::warn!(%code, %message, ?run_id, "control plane rejected executor operation");
                                    if run_id.is_some_and(|id| active_lease.as_ref().is_some_and(|lease| lease.run.spec.id == id)) {
                                        if let (Some(daemon), Some(lease)) = (&config.local_daemon, &active_lease) {
                                            let _ = daemon.call("runs.cancel", json!({"run_id": lease.run.spec.id})).await;
                                        }
                                        if let Some(task) = active_task.take() { task.abort(); }
                                        active_lease = None;
                                    }
                                }
                            }
                        }
                        Message::Ping(payload) => socket.send(Message::Pong(payload)).await?,
                        Message::Pong(_) => {}
                        Message::Close(_) => bail!("control plane closed executor WebSocket"),
                        Message::Binary(_) | Message::Frame(_) => {}
                    }
                }
            }
        }
    }
    .await;
    if let Some(task) = active_task {
        task.abort();
    }
    for task in desktop_tasks {
        task.abort();
    }
    if let Some(task) = sync_task {
        task.abort();
    }
    outcome
}

async fn metadata_sync_loop(client: ControlPlaneClient, daemon: LocalDaemonClient, user_id: Uuid) {
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        if let Err(error) = tokio::time::timeout(
            Duration::from_secs(30),
            synchronize_metadata_once(&client, &daemon, user_id),
        )
        .await
        .context("metadata sync cycle timed out")
        .and_then(|result| result)
        {
            tracing::warn!(?error, "background metadata sync cycle failed");
        }
    }
}

async fn synchronize_metadata_once(
    client: &ControlPlaneClient,
    daemon: &LocalDaemonClient,
    user_id: Uuid,
) -> Result<()> {
    let peer_id = format!("{}#{}", client.server_url, client.executor_id);
    for _ in 0..5 {
        let state: LocalSyncState = serde_json::from_value(
            daemon
                .call("sync.state", json!({"peer_id": peer_id}))
                .await?,
        )?;
        let page: LocalSyncChanges = serde_json::from_value(
            daemon
                .call(
                    "entities.changes",
                    json!({"after": state.local_cursor, "limit": 100}),
                )
                .await?,
        )?;
        if page.changes.is_empty() {
            break;
        }
        let changes = page
            .changes
            .iter()
            .map(|change| local_change_for_server(user_id, client.executor_id, change))
            .collect::<Result<Vec<_>>>()?;
        let response = client.push_sync_changes(changes).await?;
        if response.results.len() != page.changes.len() {
            bail!("metadata sync response length does not match its request");
        }
        for (local, result) in page.changes.iter().zip(response.results) {
            if result.status == SyncApplyStatus::Conflict {
                let entity = result
                    .entity
                    .context("metadata conflict response omitted the current server entity")?;
                apply_remote_entity(
                    daemon,
                    &peer_id,
                    state.remote_cursor,
                    user_id,
                    RemoteEntityInput {
                        entity_type: &entity.entity_type,
                        entity_id: entity.entity_id,
                        revision: entity.revision,
                        payload: entity.payload.as_ref(),
                        tombstone: entity.tombstone,
                        updated_at: entity.updated_at,
                    },
                )
                .await?;
            }
            daemon
                .call(
                    "sync.ack_local",
                    json!({"peer_id": peer_id, "cursor": local.cursor}),
                )
                .await?;
        }
        if page.changes.len() < 100 {
            break;
        }
    }
    for _ in 0..5 {
        let state: LocalSyncState = serde_json::from_value(
            daemon
                .call("sync.state", json!({"peer_id": peer_id}))
                .await?,
        )?;
        let response = client.pull_sync_changes(state.remote_cursor).await?;
        if response.changes.is_empty() {
            break;
        }
        for change in &response.changes {
            apply_remote_entity(
                daemon,
                &peer_id,
                change.cursor,
                user_id,
                RemoteEntityInput {
                    entity_type: &change.entity_type,
                    entity_id: change.entity_id,
                    revision: change.revision,
                    payload: change.payload.as_ref(),
                    tombstone: change.operation == SyncOperation::Delete,
                    updated_at: change.created_at,
                },
            )
            .await?;
        }
        if response.changes.len() < 100 {
            break;
        }
    }
    Ok(())
}

fn local_change_for_server(
    user_id: Uuid,
    device_id: Uuid,
    change: &LocalSyncChange,
) -> Result<SyncChange> {
    let entity_id = stable_sync_entity_id(user_id, &change.entity_type, &change.entity_id);
    let operation = match change.operation.as_str() {
        "upsert" if !change.entity.tombstone => SyncOperation::Upsert,
        "delete" if change.entity.tombstone => SyncOperation::Delete,
        other => bail!("invalid local sync operation {other}"),
    };
    let client_timestamp = chrono::DateTime::parse_from_rfc3339(&change.created_at)
        .map(|value| value.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::Utc::now());
    let payload = if operation == SyncOperation::Upsert {
        Some(sync_payload_for_server(
            user_id,
            &change.entity_type,
            &change.entity_id,
            &change.entity.payload,
        )?)
    } else {
        None
    };
    Ok(SyncChange {
        schema_version: SCHEMA_VERSION,
        operation_id: stable_sync_operation_id(device_id, change.cursor),
        device_id,
        entity_type: change.entity_type.clone(),
        entity_id,
        base_revision: change.revision.saturating_sub(1),
        operation,
        payload,
        client_timestamp,
    })
}

fn sync_payload_for_server(
    user_id: Uuid,
    entity_type: &str,
    local_entity_id: &str,
    payload: &Value,
) -> Result<Value> {
    let mut payload = payload
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("local {entity_type} payload must be a JSON object"))?;
    if Uuid::parse_str(local_entity_id).is_err() {
        payload.insert(
            "_cowork_local_entity_id".to_owned(),
            Value::String(local_entity_id.to_owned()),
        );
    }
    if entity_type == "schedule" {
        if let Some(local_profile_id) = payload
            .get("model_profile_id")
            .and_then(Value::as_str)
            .filter(|value| Uuid::parse_str(value).is_err())
            .map(str::to_owned)
        {
            payload.insert(
                "_cowork_local_model_profile_id".to_owned(),
                Value::String(local_profile_id.clone()),
            );
            payload.insert(
                "model_profile_id".to_owned(),
                Value::String(
                    stable_sync_entity_id(user_id, "provider_profile", &local_profile_id)
                        .to_string(),
                ),
            );
        }
    }
    Ok(Value::Object(payload))
}

struct RemoteEntityInput<'a> {
    entity_type: &'a str,
    entity_id: Uuid,
    revision: i64,
    payload: Option<&'a Value>,
    tombstone: bool,
    updated_at: chrono::DateTime<chrono::Utc>,
}

async fn apply_remote_entity(
    daemon: &LocalDaemonClient,
    peer_id: &str,
    remote_cursor: i64,
    user_id: Uuid,
    entity: RemoteEntityInput<'_>,
) -> Result<()> {
    let local_entity_id = local_entity_id_for_remote(
        daemon,
        user_id,
        entity.entity_type,
        entity.entity_id,
        entity.payload,
    )
    .await?;
    let local_payload = entity
        .payload
        .map(|payload| sync_payload_for_local(entity.entity_type, payload));
    daemon
        .call(
            "sync.apply_remote",
            json!({
                "peer_id": peer_id,
                "remote_cursor": remote_cursor,
                "entity": {
                    "entity_type": entity.entity_type,
                    "entity_id": local_entity_id,
                    "revision": entity.revision,
                    "payload": local_payload,
                    "tombstone": entity.tombstone,
                    "updated_at": entity.updated_at,
                },
            }),
        )
        .await?;
    Ok(())
}

fn sync_payload_for_local(entity_type: &str, payload: &Value) -> Value {
    let Some(mut payload) = payload.as_object().cloned() else {
        return payload.clone();
    };
    payload.remove("_cowork_local_entity_id");
    if entity_type == "schedule" {
        if let Some(local_profile_id) = payload
            .remove("_cowork_local_model_profile_id")
            .and_then(|value| value.as_str().map(str::to_owned))
        {
            payload.insert(
                "model_profile_id".to_owned(),
                Value::String(local_profile_id),
            );
        }
    } else {
        payload.remove("_cowork_local_model_profile_id");
    }
    Value::Object(payload)
}

async fn local_entity_id_for_remote(
    daemon: &LocalDaemonClient,
    user_id: Uuid,
    entity_type: &str,
    entity_id: Uuid,
    payload: Option<&Value>,
) -> Result<String> {
    if let Some(local_id) = payload
        .and_then(|value| value.get("_cowork_local_entity_id"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(local_id.to_owned());
    }
    let local_entities = daemon
        .call(
            "entities.list",
            json!({"entity_type": entity_type, "include_tombstones": true}),
        )
        .await?;
    if let Some(items) = local_entities.as_array() {
        for item in items {
            let Some(local_id) = item.get("id").and_then(Value::as_str) else {
                continue;
            };
            if stable_sync_entity_id(user_id, entity_type, local_id) == entity_id {
                return Ok(local_id.to_owned());
            }
        }
    }
    Ok(entity_id.to_string())
}

fn stable_sync_entity_id(user_id: Uuid, entity_type: &str, local_entity_id: &str) -> Uuid {
    if let Ok(id) = Uuid::parse_str(local_entity_id) {
        return id;
    }
    let mut hasher = Sha256::new();
    hasher.update(b"open-cowork-global-entity-id-v1\0");
    hasher.update(user_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(entity_type.as_bytes());
    hasher.update(b"\0");
    hasher.update(local_entity_id.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn stable_sync_operation_id(device_id: Uuid, cursor: i64) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(b"open-cowork-device-sync-operation-v1\0");
    hasher.update(device_id.as_bytes());
    hasher.update(cursor.to_be_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

async fn authorize_desktop_stream(
    config: &Config,
    decisions: &Mutex<HashMap<(Uuid, bool), bool>>,
    run_id: Uuid,
    session_id: Uuid,
    control: bool,
) -> Result<bool> {
    if config.kind == ExecutorKind::ManagedWindows {
        return Ok(true);
    }
    match config
        .personal_device_remote_control
        .unwrap_or(PersonalDeviceRemoteControlMode::ConfirmEachSession)
    {
        PersonalDeviceRemoteControlMode::Off => Ok(false),
        PersonalDeviceRemoteControlMode::Unattended => Ok(true),
        PersonalDeviceRemoteControlMode::ConfirmEachSession => {
            let mut decisions = decisions.lock().await;
            if let Some(allowed) = decisions.get(&(session_id, control)) {
                return Ok(*allowed);
            }
            let allowed =
                windows_desktop::confirm_personal_session(run_id, session_id, control).await?;
            decisions.insert((session_id, control), allowed);
            if allowed && control {
                decisions.insert((session_id, false), true);
            }
            Ok(allowed)
        }
    }
}

async fn reject_desktop_stream(client: &ControlPlaneClient, stream_id: Uuid) -> Result<()> {
    let ws_url = desktop_stream_url(&client.server_url, client.executor_id, stream_id)?;
    let mut request = ws_url.into_client_request()?;
    request.headers_mut().insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", client.token))?,
    );
    let (mut socket, _) = connect_async(request)
        .await
        .context("failed to open denied reverse desktop WebSocket")?;
    socket.close(None).await?;
    Ok(())
}

async fn open_desktop_stream(
    client: &ControlPlaneClient,
    stream_id: Uuid,
    control: bool,
) -> Result<()> {
    let ws_url = desktop_stream_url(&client.server_url, client.executor_id, stream_id)?;
    let mut request = ws_url.into_client_request()?;
    request.headers_mut().insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", client.token))?,
    );
    let (socket, _) = connect_async(request)
        .await
        .context("failed to open reverse desktop WebSocket")?;
    windows_desktop::serve(socket, control).await
}

async fn send_executor_message(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    message: &ExecutorClientMessage,
) -> Result<()> {
    socket
        .send(Message::Text(serde_json::to_string(message)?.into()))
        .await?;
    Ok(())
}

fn websocket_url(server_url: &str, executor_id: Uuid) -> Result<String> {
    websocket_url_for_path(
        server_url,
        &format!("/api/v1/agent/executors/{executor_id}/connect"),
    )
}

fn desktop_stream_url(server_url: &str, executor_id: Uuid, stream_id: Uuid) -> Result<String> {
    websocket_url_for_path(
        server_url,
        &format!("/api/v1/agent/executors/{executor_id}/desktop-streams/{stream_id}"),
    )
}

fn websocket_url_for_path(server_url: &str, path: &str) -> Result<String> {
    let mut url = reqwest::Url::parse(server_url).context("invalid COWORK_SERVER_URL")?;
    match url.scheme() {
        "https" => url.set_scheme("wss").expect("wss scheme is valid"),
        "http" if matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1")) => {
            url.set_scheme("ws").expect("ws scheme is valid")
        }
        "http" => bail!("remote executor connections require HTTPS"),
        scheme => bail!("unsupported server URL scheme {scheme}"),
    }
    url.set_path(path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

async fn execute_lease(
    client: &ControlPlaneClient,
    config: &Config,
    lease: &RunLease,
) -> Result<LeaseExecution> {
    if config.kind == ExecutorKind::ManagedWindows
        && lease
            .run
            .spec
            .input
            .get("task_runner")
            .and_then(Value::as_str)
            == Some("crew")
    {
        return crew::execute_managed_run(client, config, lease).await;
    }
    if config.kind == ExecutorKind::ManagedWindows
        && lease.run.spec.input.get("windows_office").is_some()
    {
        return Ok(LeaseExecution {
            result: execute_windows_office(client, config, lease).await?,
            result_snapshot_manifest_id: None,
            result_diff_summary: Value::Null,
        });
    }
    if config.kind == ExecutorKind::ManagedWindows
        && !selected_executor_mcp_names(&lease.run.spec.input)?.is_empty()
    {
        return execute_managed_mcp_run(client, config, lease).await;
    }
    if let Some(daemon) = &config.local_daemon {
        return execute_via_local_daemon(client, config, daemon, lease).await;
    }
    client
        .append_event(
            lease,
            &RunEvent {
                schema_version: SCHEMA_VERSION,
                run_id: lease.run.spec.id,
                sequence: 0,
                event_id: Uuid::new_v4(),
                kind: RunEventKind::ModelStarted,
                payload: json!({"adapter":"openai_compatible","model":config.model_name}),
                created_at: chrono::Utc::now(),
            },
        )
        .await?;
    let response = call_model(config, &lease.run.spec.input).await?;
    let usage = response.usage.map(|usage| {
        json!({
            "prompt_tokens":usage.prompt_tokens,
            "completion_tokens":usage.completion_tokens,
            "total_tokens":usage.total_tokens.max(
                usage.prompt_tokens.saturating_add(usage.completion_tokens)
            ),
        })
    });
    client
        .append_event(
            lease,
            &RunEvent {
                schema_version: SCHEMA_VERSION,
                run_id: lease.run.spec.id,
                sequence: 0,
                event_id: Uuid::new_v4(),
                kind: RunEventKind::ModelCompleted,
                payload: json!({
                    "adapter":"openai_compatible",
                    "content":response.content,
                    "usage":usage,
                }),
                created_at: chrono::Utc::now(),
            },
        )
        .await?;
    Ok(LeaseExecution {
        result: json!({"content":response.content,"usage":usage}),
        result_snapshot_manifest_id: None,
        result_diff_summary: Value::Null,
    })
}

async fn execute_managed_mcp_run(
    client: &ControlPlaneClient,
    config: &Config,
    lease: &RunLease,
) -> Result<LeaseExecution> {
    let selected = selected_executor_mcp_names(&lease.run.spec.input)?;
    let bindings = config
        .mcp_bindings
        .iter()
        .filter(|binding| selected.binary_search(&binding.name).is_ok())
        .collect::<Vec<_>>();
    if bindings.len() != selected.len() {
        bail!("this managed Windows executor does not have every selected MCP binding");
    }
    let model = AgentModelConfig {
        base_url: config
            .model_base_url
            .clone()
            .context("managed Windows MCP execution requires a model endpoint")?,
        api_key: config.model_api_key.clone(),
        model: config.model_name.clone(),
        timeout: Duration::from_secs(20 * 60),
        max_steps: 64,
        verify_tls_certificates: true,
    };
    let runtime = AgentRuntime::new(model)?;
    let workspace = if lease.run.spec.snapshot_id.is_some() {
        materialize_run_workspace(client, config, lease).await?
    } else {
        let path = config.workspace_root.join(lease.run.spec.id.to_string());
        if path.parent() != Some(config.workspace_root.as_path()) {
            bail!("refusing a managed MCP workspace outside the configured root");
        }
        tokio::fs::create_dir_all(&path).await?;
        path
    };
    let before = inventory_workspace(&workspace).await?;
    let mut secrets = bindings
        .iter()
        .flat_map(|binding| binding.secret_values())
        .filter(|value| !value.is_empty())
        .cloned()
        .collect::<Vec<_>>();
    secrets.extend(
        config
            .model_api_key
            .clone()
            .filter(|value| !value.is_empty()),
    );
    let host = ManagedMcpRuntimeHost {
        client,
        lease,
        workspace: &workspace,
        bindings,
        secrets,
    };
    let result = runtime
        .execute(&lease.run.spec, &host)
        .await
        .map_err(|error| anyhow::anyhow!(host.redact(&format!("{error:#}"))))?;
    let content = host.redact(&result.content);
    let after = inventory_workspace(&workspace).await?;
    let diff_summary = workspace_diff_summary(Some(&before), &after);
    let result_snapshot_manifest_id = if before.fingerprints == after.fingerprints {
        None
    } else {
        Some(publish_result_snapshot(client, lease, &after).await?)
    };
    Ok(LeaseExecution {
        result: json!({
            "content":content,
            "steps":result.steps,
            "usage":{
                "prompt_tokens":result.prompt_tokens,
                "completion_tokens":result.completion_tokens,
                "total_tokens":result.prompt_tokens.saturating_add(result.completion_tokens),
            }
        }),
        result_snapshot_manifest_id,
        result_diff_summary: diff_summary,
    })
}

struct ManagedMcpRuntimeHost<'a> {
    client: &'a ControlPlaneClient,
    lease: &'a RunLease,
    workspace: &'a Path,
    bindings: Vec<&'a ExecutorMcpBinding>,
    secrets: Vec<String>,
}

impl ManagedMcpRuntimeHost<'_> {
    fn redact(&self, value: &str) -> String {
        redact_executor_secrets(value, &self.secrets)
    }

    async fn append(&self, kind: RunEventKind, mut payload: Value) -> Result<()> {
        redact_executor_secret_value(&mut payload, &self.secrets);
        self.client
            .append_event(
                self.lease,
                &RunEvent {
                    schema_version: SCHEMA_VERSION,
                    run_id: self.lease.run.spec.id,
                    sequence: 0,
                    event_id: Uuid::new_v4(),
                    kind,
                    payload,
                    created_at: chrono::Utc::now(),
                },
            )
            .await
    }
}

#[async_trait]
impl RuntimeHost for ManagedMcpRuntimeHost<'_> {
    fn tools(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "MCPTool".to_owned(),
            description: format!(
                "Call a tool on one of these executor-bound MCP servers: {}",
                self.bindings
                    .iter()
                    .map(|binding| binding.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            input_schema: json!({
                "type":"object",
                "properties":{
                    "server_name":{"type":"string"},
                    "tool_name":{"type":"string"},
                    "arguments":{"type":"object"}
                },
                "required":["server_name","tool_name","arguments"],
                "additionalProperties":false
            }),
            required_capability: Some(Capability::from("tool.mcp.invoke")),
            mutating: true,
        }]
    }

    async fn emit(&self, kind: RunEventKind, payload: Value) -> Result<()> {
        self.append(kind, payload).await
    }

    async fn execute_tool(&self, invocation: ToolInvocation) -> Result<ToolOutput> {
        if invocation.name != "MCPTool" {
            bail!("managed Windows executor received an unsupported tool");
        }
        let server_name = invocation
            .arguments
            .get("server_name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("MCPTool requires server_name")?;
        let tool_name = invocation
            .arguments
            .get("tool_name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("MCPTool requires tool_name")?;
        let arguments = invocation
            .arguments
            .get("arguments")
            .and_then(Value::as_object)
            .cloned()
            .context("MCPTool arguments must be an object")?;
        let binding = self
            .bindings
            .iter()
            .copied()
            .find(|binding| binding.name == server_name)
            .with_context(|| format!("MCP server {server_name:?} is not selected for this Run"))?;
        self.client
            .create_checkpoint(
                self.lease,
                Uuid::new_v4(),
                false,
                json!({
                    "phase":"managed_windows_mcp_dispatched",
                    "server_name":server_name,
                    "tool_name":tool_name,
                }),
            )
            .await?;
        let response =
            invoke_executor_mcp(binding, tool_name, Value::Object(arguments), self.workspace)
                .await
                .map_err(|error| anyhow::anyhow!(self.redact(&format!("{error:#}"))))?;
        let is_error = response
            .get("result")
            .and_then(|result| result.get("isError"))
            .and_then(Value::as_bool)
            == Some(true);
        let mut content = response.get("result").cloned().unwrap_or(Value::Null);
        redact_executor_secret_value(&mut content, &self.secrets);
        Ok(ToolOutput {
            content: serde_json::to_string_pretty(&content)?,
            is_error,
            safe_to_resume: true,
            metadata: json!({"server_name":server_name,"tool_name":tool_name}),
        })
    }

    async fn checkpoint(&self, mut state: Value, _safe_to_resume: bool) -> Result<()> {
        redact_executor_secret_value(&mut state, &self.secrets);
        self.client
            .create_checkpoint(self.lease, Uuid::new_v4(), false, state)
            .await
    }
}

fn selected_executor_mcp_names(input: &Value) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    let mut names = Vec::new();
    for item in input
        .get("frozen_runtime_context")
        .and_then(|context| context.get("mcp_metadata"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = item
            .get("definition")
            .and_then(|definition| definition.get("name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| {
                !name.is_empty() && name.len() <= 256 && !name.chars().any(char::is_control)
            })
            .context("selected MCP metadata contains an invalid name")?;
        if seen.insert(name.to_owned()) {
            names.push(name.to_owned());
        }
        if names.len() > MAX_EXECUTOR_MCP_BINDINGS {
            bail!("a Run may select at most {MAX_EXECUTOR_MCP_BINDINGS} MCP servers");
        }
    }
    names.sort();
    Ok(names)
}

async fn run_executor_mcp_tool_bridge() -> Result<()> {
    use std::io::Read as _;

    let mut input = Vec::new();
    let stdin = std::io::stdin();
    std::io::Read::take(stdin.lock(), (MAX_MCP_MESSAGE_BYTES + 1) as u64)
        .read_to_end(&mut input)?;
    let mut response = if input.len() > MAX_MCP_MESSAGE_BYTES {
        json!({
            "success":false,
            "server_name":null,
            "tool_name":null,
            "result":null,
            "error":format!("MCP bridge input exceeds {MAX_MCP_MESSAGE_BYTES} bytes"),
        })
    } else {
        executor_mcp_tool_bridge_response(&input, &env::current_dir()?).await
    };
    let secrets = serde_json::from_slice::<Value>(&input)
        .ok()
        .and_then(|value| value.get("server").cloned())
        .into_iter()
        .flat_map(|server| {
            ["environment", "headers"]
                .into_iter()
                .filter_map(move |key| server.get(key).and_then(Value::as_object).cloned())
                .flat_map(|values| values.into_values())
        })
        .filter_map(|value| value.as_str().map(ToOwned::to_owned))
        .collect::<Vec<_>>();
    redact_executor_secret_value(&mut response, &secrets);
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    std::io::Write::write_all(&mut stdout, &serde_json::to_vec(&response)?)?;
    std::io::Write::write_all(&mut stdout, b"\n")?;
    std::io::Write::flush(&mut stdout)?;
    Ok(())
}

async fn executor_mcp_tool_bridge_response(input: &[u8], workspace: &Path) -> Value {
    #[derive(Deserialize)]
    struct Request {
        server: ExecutorMcpBinding,
        tool_name: String,
        #[serde(default)]
        arguments: Value,
    }

    let result = async {
        let request: Request = serde_json::from_slice(input).context("invalid MCP bridge JSON")?;
        let bytes = serde_json::to_vec(&[request.server])?;
        let bindings = parse_executor_mcp_bindings(ExecutorKind::ManagedWindows, &bytes)?;
        validate_executor_mcp_command_files(&bindings)?;
        let binding = bindings
            .first()
            .context("MCP bridge request did not contain a binding")?;
        let tool_name = request.tool_name.trim();
        let response = invoke_executor_mcp(binding, tool_name, request.arguments, workspace).await?;
        let result = response.get("result").cloned().unwrap_or(Value::Null);
        let is_error = result.get("isError").and_then(Value::as_bool) == Some(true);
        Ok::<_, anyhow::Error>(json!({
            "success":!is_error,
            "server_name":binding.name,
            "tool_name":tool_name,
            "protocol_version":response
                .get("protocol_version")
                .cloned()
                .unwrap_or(Value::Null),
            "result":result,
            "error":if is_error { Value::String("MCP tool returned isError=true".to_owned()) } else { Value::Null },
        }))
    }
    .await;
    match result {
        Ok(response) => response,
        Err(error) => json!({
            "success":false,
            "server_name":null,
            "tool_name":null,
            "result":null,
            "error":format!("{error:#}"),
        }),
    }
}

async fn invoke_executor_mcp(
    binding: &ExecutorMcpBinding,
    tool_name: &str,
    arguments: Value,
    workspace: &Path,
) -> Result<Value> {
    if tool_name.is_empty() || tool_name.len() > 1_024 || tool_name.chars().any(char::is_control) {
        bail!("MCP tool name is missing or invalid");
    }
    if !arguments.is_object() {
        bail!("MCP tool arguments must be an object");
    }
    match binding.transport {
        ExecutorMcpTransport::Stdio => {
            invoke_executor_stdio_mcp(binding, tool_name, arguments, workspace).await
        }
        ExecutorMcpTransport::StreamableHttp => {
            invoke_executor_http_mcp(binding, tool_name, arguments).await
        }
    }
}

async fn invoke_executor_stdio_mcp(
    binding: &ExecutorMcpBinding,
    tool_name: &str,
    arguments: Value,
    workspace: &Path,
) -> Result<Value> {
    let mut command = tokio::process::Command::new(&binding.command);
    command
        .args(&binding.args)
        .current_dir(workspace)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for name in [
        "SystemRoot",
        "WINDIR",
        "PATH",
        "PATHEXT",
        "TEMP",
        "TMP",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "PROGRAMDATA",
    ] {
        if let Some(value) = env::var_os(name) {
            command.env(name, value);
        }
    }
    command.envs(&binding.environment);
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to start MCP server {:?}", binding.name))?;
    let _process_job = match ManagedMcpProcessJob::attach(&child) {
        Ok(job) => job,
        Err(error) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(error)
                .with_context(|| format!("failed to isolate MCP server {:?}", binding.name));
        }
    };
    let mut stdin = child
        .stdin
        .take()
        .context("MCP server did not expose stdin")?;
    let stdout = child
        .stdout
        .take()
        .context("MCP server did not expose stdout")?;
    let mut stdout = BufReader::new(stdout);
    let protocol = async {
        let initialized = executor_mcp_request(
            &mut stdin,
            &mut stdout,
            1,
            "initialize",
            json!({
                "protocolVersion":"2024-11-05",
                "clientInfo":{
                    "name":"Open Cowork managed Windows executor",
                    "version":env!("CARGO_PKG_VERSION")
                },
                "capabilities":{}
            }),
        )
        .await?;
        write_executor_mcp_message(
            &mut stdin,
            &json!({
                "jsonrpc":"2.0",
                "method":"notifications/initialized",
                "params":{}
            }),
        )
        .await?;
        let result = executor_mcp_request(
            &mut stdin,
            &mut stdout,
            2,
            "tools/call",
            json!({"name":tool_name,"arguments":arguments}),
        )
        .await?;
        Ok(json!({
            "server_name":binding.name,
            "tool_name":tool_name,
            "protocol_version":initialized.get("protocolVersion"),
            "result":result,
        }))
    }
    .await;
    drop(stdin);
    if child.try_wait()?.is_none() {
        child
            .kill()
            .await
            .context("failed to terminate MCP server")?;
    }
    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    protocol
}

struct ExecutorHttpMcpClient {
    client: Client,
    url: Url,
    headers: HeaderMap,
    session_id: Option<String>,
    protocol_version: String,
    initialized: bool,
}

impl ExecutorHttpMcpClient {
    async fn new(binding: &ExecutorMcpBinding, allow_insecure_test: bool) -> Result<Self> {
        let url = validate_executor_http_endpoint(&binding.url, allow_insecure_test)?;
        let headers = validate_executor_http_headers(&binding.headers)?;
        let host = url
            .host_str()
            .context("executor MCP HTTP endpoint has no hostname")?
            .to_owned();
        let port = url
            .port_or_known_default()
            .context("executor MCP HTTP endpoint has no port")?;
        let mut addresses = tokio::net::lookup_host((host.as_str(), port))
            .await
            .with_context(|| format!("failed to resolve executor MCP endpoint {host:?}"))?
            .collect::<Vec<SocketAddr>>();
        addresses.sort_unstable();
        addresses.dedup();
        if addresses.is_empty()
            || (!allow_insecure_test
                && addresses
                    .iter()
                    .any(|address| !network_safety::is_public_destination(address.ip())))
        {
            bail!("executor MCP endpoint did not resolve exclusively to public addresses");
        }
        let client = Client::builder()
            .redirect(Policy::none())
            .https_only(!allow_insecure_test)
            .no_proxy()
            .timeout(Duration::from_secs(120))
            .resolve_to_addrs(&host, &addresses)
            .build()
            .context("failed to construct the executor MCP HTTP client")?;
        Ok(Self {
            client,
            url,
            headers,
            session_id: None,
            protocol_version: LATEST_HTTP_MCP_PROTOCOL_VERSION.to_owned(),
            initialized: false,
        })
    }

    fn request(&self, method: Method, accept: &'static str) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .request(method, self.url.clone())
            .headers(self.headers.clone())
            .header(ACCEPT, accept);
        if let Some(session_id) = &self.session_id {
            request = request.header("mcp-session-id", session_id);
        }
        if self.initialized {
            request = request.header("mcp-protocol-version", &self.protocol_version);
        }
        request
    }

    async fn post(&mut self, payload: Value, expects_response: bool) -> Result<Value> {
        let request_id = payload.get("id").and_then(Value::as_u64);
        let encoded = serde_json::to_vec(&payload)?;
        if encoded.len() > MAX_MCP_MESSAGE_BYTES {
            bail!("MCP HTTP request exceeds {MAX_MCP_MESSAGE_BYTES} bytes");
        }
        let mut response = self
            .request(Method::POST, "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .body(encoded)
            .send()
            .await
            .context("executor MCP HTTP request failed")?;
        if !expects_response {
            if response.status() != reqwest::StatusCode::ACCEPTED {
                bail!(
                    "executor MCP HTTP notification returned status {}, expected 202",
                    response.status()
                );
            }
            let _ = read_executor_http_body(&mut response).await?;
            return Ok(Value::Null);
        }
        if !response.status().is_success() {
            bail!(
                "executor MCP HTTP endpoint returned status {}",
                response.status()
            );
        }
        if self.session_id.is_none() {
            if let Some(session_id) = response.headers().get("mcp-session-id") {
                let session_id = session_id
                    .to_str()
                    .context("executor MCP endpoint returned a non-ASCII session ID")?;
                if session_id.is_empty()
                    || session_id.len() > 1_024
                    || !session_id.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
                {
                    bail!("executor MCP endpoint returned an invalid session ID");
                }
                self.session_id = Some(session_id.to_owned());
            }
        }
        let request_id = request_id.context("executor MCP response request has no JSON-RPC ID")?;
        let (mut message, mut last_event_id, mut retry_ms) =
            parse_executor_http_response(response, request_id).await?;
        for _ in 0..4 {
            if let Some(message) = message {
                return unwrap_executor_http_rpc(message);
            }
            let Some(event_id) = last_event_id.as_deref() else {
                break;
            };
            if retry_ms > 0 {
                tokio::time::sleep(Duration::from_millis(retry_ms.min(5_000))).await;
            }
            let response = self
                .request(Method::GET, "text/event-stream")
                .header("last-event-id", event_id)
                .send()
                .await
                .context("executor MCP SSE resume failed")?;
            if !response.status().is_success() {
                bail!(
                    "executor MCP SSE resume returned status {}",
                    response.status()
                );
            }
            let parsed = parse_executor_http_response(response, request_id).await?;
            message = parsed.0;
            if parsed.1.is_some() {
                last_event_id = parsed.1;
            }
            retry_ms = parsed.2;
        }
        bail!("executor MCP SSE stream ended before its JSON-RPC response")
    }

    async fn close(&self) {
        if self.session_id.is_none() {
            return;
        }
        let request = self
            .request(Method::DELETE, "application/json, text/event-stream")
            .send();
        let _ = tokio::time::timeout(Duration::from_secs(5), request).await;
    }
}

async fn invoke_executor_http_mcp(
    binding: &ExecutorMcpBinding,
    tool_name: &str,
    arguments: Value,
) -> Result<Value> {
    tokio::time::timeout(
        Duration::from_secs(120),
        invoke_executor_http_mcp_with_policy(binding, tool_name, arguments, false),
    )
    .await
    .context("executor MCP HTTP protocol timed out")?
}

async fn invoke_executor_http_mcp_with_policy(
    binding: &ExecutorMcpBinding,
    tool_name: &str,
    arguments: Value,
    allow_insecure_test: bool,
) -> Result<Value> {
    let mut client = ExecutorHttpMcpClient::new(binding, allow_insecure_test).await?;
    let result = async {
        let initialized = client
            .post(
                json!({
                    "jsonrpc":"2.0",
                    "id":1,
                    "method":"initialize",
                    "params":{
                        "protocolVersion":LATEST_HTTP_MCP_PROTOCOL_VERSION,
                        "clientInfo":{
                            "name":"Open Cowork managed Windows executor",
                            "version":env!("CARGO_PKG_VERSION")
                        },
                        "capabilities":{}
                    }
                }),
                true,
            )
            .await?;
        let protocol_version = initialized
            .get("protocolVersion")
            .and_then(Value::as_str)
            .context("executor MCP HTTP initialize response omitted protocolVersion")?;
        if !SUPPORTED_HTTP_MCP_PROTOCOL_VERSIONS.contains(&protocol_version) {
            bail!("executor MCP HTTP server negotiated unsupported protocol {protocol_version:?}");
        }
        client.protocol_version = protocol_version.to_owned();
        client.initialized = true;
        client
            .post(
                json!({
                    "jsonrpc":"2.0",
                    "method":"notifications/initialized",
                    "params":{}
                }),
                false,
            )
            .await?;
        let result = client
            .post(
                json!({
                    "jsonrpc":"2.0",
                    "id":2,
                    "method":"tools/call",
                    "params":{"name":tool_name,"arguments":arguments}
                }),
                true,
            )
            .await?;
        Ok(json!({
            "server_name":binding.name,
            "tool_name":tool_name,
            "protocol_version":protocol_version,
            "result":result,
        }))
    }
    .await;
    client.close().await;
    result
}

async fn read_executor_http_body(response: &mut reqwest::Response) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len().saturating_add(chunk.len()) > MAX_MCP_MESSAGE_BYTES {
            bail!("MCP HTTP response exceeds {MAX_MCP_MESSAGE_BYTES} bytes");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn parse_executor_http_response(
    mut response: reqwest::Response,
    request_id: u64,
) -> Result<(Option<Value>, Option<String>, u64)> {
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if content_type == "application/json" || content_type.ends_with("+json") {
        let body = read_executor_http_body(&mut response).await?;
        let message: Value =
            serde_json::from_slice(&body).context("executor MCP HTTP JSON response is invalid")?;
        if message.get("id").and_then(Value::as_u64) != Some(request_id) {
            bail!("executor MCP HTTP response has the wrong JSON-RPC ID");
        }
        return Ok((Some(message), None, 0));
    }
    if content_type != "text/event-stream" {
        bail!("executor MCP HTTP endpoint returned unsupported content type {content_type:?}");
    }
    let mut pending = Vec::new();
    let mut total_bytes = 0_usize;
    let mut last_event_id = None;
    let mut retry_ms = 0_u64;
    while let Some(chunk) = response.chunk().await? {
        total_bytes = total_bytes.saturating_add(chunk.len());
        if total_bytes > MAX_MCP_MESSAGE_BYTES {
            bail!("MCP HTTP response exceeds {MAX_MCP_MESSAGE_BYTES} bytes");
        }
        pending.extend_from_slice(&chunk);
        while let Some((event_end, separator_end)) = executor_sse_event_boundary(&pending) {
            let event = pending[..event_end].to_vec();
            pending.drain(..separator_end);
            if let Some(message) =
                parse_executor_sse_event(&event, request_id, &mut last_event_id, &mut retry_ms)?
            {
                return Ok((Some(message), last_event_id, retry_ms));
            }
        }
    }
    if !pending.is_empty() {
        if let Some(message) =
            parse_executor_sse_event(&pending, request_id, &mut last_event_id, &mut retry_ms)?
        {
            return Ok((Some(message), last_event_id, retry_ms));
        }
    }
    Ok((None, last_event_id, retry_ms))
}

fn executor_sse_event_boundary(bytes: &[u8]) -> Option<(usize, usize)> {
    let lf = bytes.windows(2).position(|window| window == b"\n\n");
    let crlf = bytes.windows(4).position(|window| window == b"\r\n\r\n");
    match (lf, crlf) {
        (Some(left), Some(right)) if left <= right => Some((left, left + 2)),
        (Some(_), Some(right)) => Some((right, right + 4)),
        (Some(left), None) => Some((left, left + 2)),
        (None, Some(right)) => Some((right, right + 4)),
        (None, None) => None,
    }
}

fn parse_executor_sse_event(
    event: &[u8],
    request_id: u64,
    last_event_id: &mut Option<String>,
    retry_ms: &mut u64,
) -> Result<Option<Value>> {
    let event = std::str::from_utf8(event).context("executor MCP SSE response is not UTF-8")?;
    let mut data = Vec::new();
    for line in event.lines() {
        if line.starts_with(':') {
            continue;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "data" => data.push(value),
            "id" if !value.contains('\0') => {
                if value.is_empty() {
                    *last_event_id = None;
                } else if value.len() > 1_024
                    || !value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
                {
                    bail!("executor MCP SSE event ID is invalid");
                } else {
                    *last_event_id = Some(value.to_owned());
                }
            }
            "retry" => {
                if let Ok(value) = value.parse::<u64>() {
                    *retry_ms = value.min(5_000);
                }
            }
            _ => {}
        }
    }
    if data.is_empty() {
        return Ok(None);
    }
    let message: Value = serde_json::from_str(&data.join("\n"))
        .context("executor MCP SSE event contains invalid JSON")?;
    if message.get("id").and_then(Value::as_u64) == Some(request_id)
        && (message.get("result").is_some() || message.get("error").is_some())
    {
        return Ok(Some(message));
    }
    if message.get("id").is_some() && message.get("method").is_some() {
        bail!("executor MCP HTTP server requests are not supported by this one-shot client");
    }
    Ok(None)
}

fn unwrap_executor_http_rpc(message: Value) -> Result<Value> {
    if let Some(error) = message.get("error") {
        let detail = error
            .get("message")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| error.to_string());
        bail!(
            "executor MCP server rejected request: {}",
            detail.chars().take(2_000).collect::<String>()
        );
    }
    Ok(message.get("result").cloned().unwrap_or(Value::Null))
}

async fn executor_mcp_request<W, R>(
    stdin: &mut W,
    stdout: &mut R,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value>
where
    W: AsyncWrite + Unpin,
    R: AsyncBufRead + Unpin,
{
    write_executor_mcp_message(
        stdin,
        &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
    )
    .await?;
    for _ in 0..=256 {
        let line = tokio::time::timeout(Duration::from_secs(120), read_executor_mcp_line(stdout))
            .await
            .with_context(|| format!("MCP request {method} timed out"))??
            .with_context(|| format!("MCP server closed stdout during {method}"))?;
        let Ok(message) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        if message.get("id").and_then(Value::as_u64) != Some(id) {
            continue;
        }
        if let Some(error) = message.get("error") {
            let detail = error
                .get("message")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| error.to_string());
            bail!(
                "MCP server rejected {method}: {}",
                detail.chars().take(2_000).collect::<String>()
            );
        }
        return Ok(message.get("result").cloned().unwrap_or(Value::Null));
    }
    bail!("MCP server exceeded the unsolicited-message limit during {method}")
}

async fn write_executor_mcp_message<W: AsyncWrite + Unpin>(
    stdin: &mut W,
    value: &Value,
) -> Result<()> {
    let mut encoded = serde_json::to_vec(value)?;
    if encoded.len() > MAX_MCP_MESSAGE_BYTES {
        bail!("MCP request exceeds {MAX_MCP_MESSAGE_BYTES} bytes");
    }
    encoded.push(b'\n');
    stdin.write_all(&encoded).await?;
    stdin.flush().await?;
    Ok(())
}

async fn read_executor_mcp_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok((!line.is_empty()).then_some(line));
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |index| index + 1);
        if line.len().saturating_add(take) > MAX_MCP_MESSAGE_BYTES {
            bail!("MCP response line exceeds {MAX_MCP_MESSAGE_BYTES} bytes");
        }
        line.extend_from_slice(&available[..take]);
        reader.consume(take);
        if newline.is_some() {
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(Some(line));
        }
    }
}

fn redact_executor_secrets(value: &str, secrets: &[String]) -> String {
    let mut secrets = secrets
        .iter()
        .filter(|secret| !secret.is_empty())
        .collect::<Vec<_>>();
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    secrets
        .into_iter()
        .fold(value.to_owned(), |redacted, secret| {
            redacted.replace(secret.as_str(), "[REDACTED]")
        })
}

fn redact_executor_secret_value(value: &mut Value, secrets: &[String]) {
    match value {
        Value::String(text) => *text = redact_executor_secrets(text, secrets),
        Value::Array(values) => {
            for value in values {
                redact_executor_secret_value(value, secrets);
            }
        }
        Value::Object(object) => {
            for value in object.values_mut() {
                redact_executor_secret_value(value, secrets);
            }
        }
        _ => {}
    }
}

async fn execute_via_local_daemon(
    client: &ControlPlaneClient,
    config: &Config,
    daemon: &LocalDaemonClient,
    lease: &RunLease,
) -> Result<LeaseExecution> {
    daemon.verify_device(config.executor_id).await?;
    let existing: Option<RunRecord> = daemon
        .call("server_runs.get", json!({"run_id": lease.run.spec.id}))
        .await?
        .get("run")
        .filter(|run| !run.is_null())
        .cloned()
        .map(serde_json::from_value)
        .transpose()?;
    if existing
        .as_ref()
        .is_some_and(|existing| existing.spec.idempotency_key != lease.run.spec.idempotency_key)
    {
        bail!("local daemon has different immutable inputs for this server run ID");
    }
    let snapshot_workspace = if existing.is_none() && lease.run.spec.snapshot_id.is_some() {
        Some(materialize_run_workspace(client, config, lease).await?)
    } else {
        None
    };
    let model_config = if lease
        .run
        .spec
        .input
        .get("task_runner")
        .and_then(Value::as_str)
        == Some("crew")
    {
        let base_url = config.model_base_url.as_ref().context(
            "personal Crew runs require COWORK_MODEL_BASE_URL on the outbound device agent",
        )?;
        let model = CrewModelConfig {
            base_url: base_url.clone(),
            api_key: config.model_api_key.clone(),
            model: config.model_name.clone(),
            timeout: Duration::from_secs(20 * 60),
            verify_tls_certificates: true,
        };
        let definition = lease
            .run
            .spec
            .input
            .get("crew_definition")
            .cloned()
            .context("the personal Crew run has no frozen crew_definition")?;
        let crew_request = prepare_crew_request(definition, &lease.run.spec, &model)?;
        Some(json!({
            "base_url": base_url,
            "api_key": config.model_api_key,
            "model": config.model_name,
            "timeout_ms": 20 * 60 * 1_000_u64,
            "max_steps": 1,
            "verify_tls_certificates": true,
            "mcp_servers": [],
            "crew_request": crew_request,
            "codex_request": null,
        }))
    } else {
        config.model_base_url.as_ref().map(|base_url| {
            json!({
                "base_url": base_url,
                "api_key": config.model_api_key,
                "model": config.model_name,
                "timeout_ms": 20 * 60 * 1_000_u64,
                "max_steps": 64,
                "verify_tls_certificates": true,
                "mcp_servers": [],
                "crew_request": null,
                "codex_request": null,
            })
        })
    };
    let imported: RunRecord = serde_json::from_value(
        daemon
            .call(
                "server_runs.import",
                json!({
                    "run_spec": lease.run.spec,
                    "model_config": model_config,
                    "workspace_path": snapshot_workspace,
                    "defer_start": true,
                }),
            )
            .await?,
    )?;
    if imported.spec.id != lease.run.spec.id {
        bail!("local daemon imported a different server run ID");
    }
    let workspace = if lease.run.spec.snapshot_id.is_some() {
        let candidate = config.workspace_root.join(lease.run.spec.id.to_string());
        if tokio::fs::try_exists(&candidate).await? {
            Some(candidate)
        } else {
            None
        }
    } else {
        daemon
            .call("runs.workspace", json!({"run_id": lease.run.spec.id}))
            .await?
            .get("workspace_path")
            .and_then(Value::as_str)
            .map(PathBuf::from)
    };
    let before = if imported.state == RunState::WaitingForExecutor {
        match workspace.as_deref() {
            Some(workspace) => Some(inventory_workspace(workspace).await?),
            None => None,
        }
    } else {
        None
    };
    if imported.state == RunState::WaitingForExecutor {
        let started: RunRecord = serde_json::from_value(
            daemon
                .call("server_runs.start", json!({"run_id": lease.run.spec.id}))
                .await?,
        )?;
        if started.spec.id != lease.run.spec.id {
            bail!("local daemon started a different server run ID");
        }
    }
    let outcome = relay_local_daemon_run(
        client,
        daemon,
        lease,
        workspace.as_deref(),
        imported.state.is_terminal(),
    )
    .await;
    if outcome.is_err() {
        let _ = daemon
            .call("runs.cancel", json!({"run_id": lease.run.spec.id}))
            .await;
    }
    let result = outcome?;
    if let Some(manifest) = client.existing_result_snapshot(lease).await? {
        return Ok(LeaseExecution {
            result,
            result_snapshot_manifest_id: Some(manifest.id),
            result_diff_summary: Value::Null,
        });
    }
    let Some(workspace) = workspace.as_deref() else {
        return Ok(LeaseExecution {
            result,
            result_snapshot_manifest_id: None,
            result_diff_summary: Value::Null,
        });
    };
    let after = inventory_workspace(workspace).await?;
    let diff_summary = workspace_diff_summary(before.as_ref(), &after);
    if before
        .as_ref()
        .is_some_and(|before| before.fingerprints == after.fingerprints)
    {
        return Ok(LeaseExecution {
            result,
            result_snapshot_manifest_id: None,
            result_diff_summary: diff_summary,
        });
    }
    let manifest_id = publish_result_snapshot(client, lease, &after).await?;
    Ok(LeaseExecution {
        result,
        result_snapshot_manifest_id: Some(manifest_id),
        result_diff_summary: diff_summary,
    })
}

async fn relay_local_daemon_run(
    client: &ControlPlaneClient,
    daemon: &LocalDaemonClient,
    lease: &RunLease,
    workspace: Option<&Path>,
    recovering_terminal_run: bool,
) -> Result<Value> {
    let run_id = lease.run.spec.id;
    let mut cursor = 0_i64;
    loop {
        // Read the record before its events. The daemon persists every terminal
        // event before the matching terminal record, so observing a terminal
        // record here guarantees that the following event query can drain the
        // complete durable event log before this bridge returns.
        let record: RunRecord =
            serde_json::from_value(daemon.call("runs.get", json!({"run_id": run_id})).await?)?;
        let events: Vec<RunEvent> = serde_json::from_value(
            daemon
                .call("runs.events", json!({"run_id": run_id, "after": cursor}))
                .await?,
        )?;
        for event in events {
            cursor = cursor.max(event.sequence);
            match event.kind {
                RunEventKind::ApprovalRequested | RunEventKind::InputRequested
                    if recovering_terminal_run =>
                {
                    let mut recovered = event.clone();
                    recovered.kind = RunEventKind::Warning;
                    recovered.payload = json!({
                        "code": "recovered_resolved_intervention",
                        "original_kind": match event.kind {
                            RunEventKind::ApprovalRequested => "approval_requested",
                            _ => "input_requested",
                        },
                        "message": "A locally resolved intervention was recovered after the device connection was interrupted.",
                    });
                    client.append_event(lease, &recovered).await?;
                }
                RunEventKind::ApprovalRequested => {
                    relay_local_approval(client, daemon, lease, &event).await?;
                }
                RunEventKind::InputRequested => {
                    relay_local_input(client, daemon, lease, &event).await?;
                }
                RunEventKind::CheckpointCreated => {
                    relay_local_checkpoint(client, daemon, lease, &event).await?;
                }
                RunEventKind::ArtifactCreated => {
                    relay_local_artifact(client, lease, workspace, &event).await?;
                }
                RunEventKind::Created
                | RunEventKind::StateChanged
                | RunEventKind::ApprovalResolved
                | RunEventKind::InputReceived
                | RunEventKind::Completed
                | RunEventKind::Failed => {}
                _ => client.append_event(lease, &event).await?,
            }
        }
        match record.state {
            RunState::Completed => return Ok(record.result.unwrap_or(Value::Null)),
            RunState::Failed | RunState::Interrupted => {
                let error = record.error.unwrap_or(RunError {
                    code: "local_daemon_failed".to_owned(),
                    message: "the local daemon run failed without details".to_owned(),
                    retryable: false,
                    details: Value::Null,
                });
                bail!("{}: {}", error.code, error.message);
            }
            RunState::Canceled | RunState::Expired => {
                bail!("local daemon run ended in state {:?}", record.state)
            }
            _ => tokio::time::sleep(Duration::from_millis(250)).await,
        }
    }
}

async fn relay_local_checkpoint(
    client: &ControlPlaneClient,
    daemon: &LocalDaemonClient,
    lease: &RunLease,
    event: &RunEvent,
) -> Result<()> {
    let sequence = event
        .payload
        .get("sequence")
        .and_then(Value::as_i64)
        .context("local checkpoint event is missing its sequence")?;
    let checkpoints = daemon
        .call("runs.checkpoints", json!({"run_id": lease.run.spec.id}))
        .await?;
    let checkpoint = checkpoints
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("sequence").and_then(Value::as_i64) == Some(sequence))
        })
        .context("local checkpoint event has no persisted checkpoint")?;
    let source_checkpoint_id = checkpoint
        .get("id")
        .and_then(Value::as_str)
        .context("local checkpoint has no stable ID")?
        .parse()
        .context("local checkpoint ID is invalid")?;
    client
        .create_checkpoint(
            lease,
            source_checkpoint_id,
            checkpoint
                .get("safe_to_resume")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            checkpoint
                .get("executor_state")
                .cloned()
                .unwrap_or(Value::Null),
        )
        .await
}

async fn relay_local_approval(
    client: &ControlPlaneClient,
    daemon: &LocalDaemonClient,
    lease: &RunLease,
    event: &RunEvent,
) -> Result<()> {
    let local_id = event
        .payload
        .get("id")
        .and_then(Value::as_str)
        .context("local approval event is missing its ID")?;
    let source_request_id: Uuid = local_id.parse().context("local approval ID is invalid")?;
    let request = event
        .payload
        .get("request")
        .cloned()
        .context("local approval event is missing its request")?;
    let expires_at = event
        .payload
        .get("expires_at")
        .and_then(Value::as_str)
        .map(str::parse)
        .transpose()
        .context("local approval expiry is invalid")?;
    let approved = client
        .await_approval(lease, source_request_id, request, expires_at)
        .await?;
    daemon
        .call(
            "runs.approvals.resolve",
            json!({
                "run_id": lease.run.spec.id,
                "approval_id": local_id,
                "decision": if approved { "approved" } else { "rejected" },
            }),
        )
        .await?;
    Ok(())
}

async fn relay_local_input(
    client: &ControlPlaneClient,
    daemon: &LocalDaemonClient,
    lease: &RunLease,
    event: &RunEvent,
) -> Result<()> {
    let local_id = event
        .payload
        .get("id")
        .and_then(Value::as_str)
        .context("local input event is missing its ID")?;
    let source_request_id: Uuid = local_id.parse().context("local input ID is invalid")?;
    let prompt = event
        .payload
        .get("request")
        .cloned()
        .context("local input event is missing its prompt")?;
    let expires_at = event
        .payload
        .get("expires_at")
        .and_then(Value::as_str)
        .map(str::parse)
        .transpose()
        .context("local input expiry is invalid")?;
    let response = client
        .await_input(lease, source_request_id, prompt, expires_at)
        .await?
        .unwrap_or(Value::Null);
    daemon
        .call(
            "runs.input_requests.respond",
            json!({
                "run_id": lease.run.spec.id,
                "input_id": local_id,
                "response": response,
            }),
        )
        .await?;
    Ok(())
}

async fn relay_local_artifact(
    client: &ControlPlaneClient,
    lease: &RunLease,
    workspace: Option<&Path>,
    event: &RunEvent,
) -> Result<()> {
    let workspace = workspace.context("local artifact has no run workspace")?;
    let relative = event
        .payload
        .get("path")
        .and_then(Value::as_str)
        .context("local artifact event is missing its path")?;
    let path = safe_run_path(workspace, relative)?;
    let metadata = tokio::fs::metadata(&path)
        .await
        .context("local artifact file is unavailable")?;
    if !metadata.is_file() || metadata.len() > 64 * 1024 * 1024 {
        bail!("local artifact must be a file no larger than 64 MiB");
    }
    let bytes = tokio::fs::read(&path).await?;
    let source = event
        .payload
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or("LocalDaemon");
    client
        .upload_artifact(lease, Some(event.event_id), relative, source, bytes)
        .await?;
    Ok(())
}

#[cfg(windows)]
fn supports_windows_office(config: &Config) -> bool {
    cfg!(windows)
        && config.kind == ExecutorKind::ManagedWindows
        && config
            .capabilities
            .iter()
            .any(|item| item.name.0 == "office.microsoft")
}

#[cfg(windows)]
async fn execute_windows_office(
    client: &ControlPlaneClient,
    config: &Config,
    lease: &RunLease,
) -> Result<Value> {
    #[derive(Deserialize, Serialize)]
    struct OfficeRequest {
        application: String,
        #[serde(default = "default_office_action")]
        action: String,
        source: String,
        output: String,
        #[serde(default)]
        preview_output: Option<String>,
        #[serde(default)]
        parameters: Value,
    }

    fn default_office_action() -> String {
        "export_pdf".to_owned()
    }

    if !supports_windows_office(config) {
        bail!("this executor does not advertise office.microsoft");
    }
    let request: OfficeRequest = serde_json::from_value(
        lease
            .run
            .spec
            .input
            .get("windows_office")
            .cloned()
            .context("windows_office input is missing")?,
    )?;
    if !matches!(
        request.application.as_str(),
        "word" | "excel" | "powerpoint"
    ) {
        bail!("application must be word, excel, or powerpoint");
    }
    let run_root = materialize_run_workspace(client, config, lease).await?;
    let source = safe_run_path(&run_root, &request.source)?;
    let output = safe_run_path(&run_root, &request.output)?;
    let preview_output = request
        .preview_output
        .as_deref()
        .map(|path| safe_run_path(&run_root, path))
        .transpose()?;
    if !source.is_file() {
        bail!("Office source file does not exist in the run workspace");
    }
    if output.exists() {
        bail!("Office output already exists; refusing to overwrite it");
    }
    if !request.output.replace('\\', "/").starts_with("artifacts/") {
        bail!("Office output must stay below the artifacts directory");
    }
    let source_extension = normalized_extension(&source)?;
    if is_active_office_extension(&source_extension) {
        bail!("macro-enabled Office source formats are blocked by policy");
    }
    let expected_source = match request.application.as_str() {
        "word" => ["doc", "docx"].as_slice(),
        "excel" => ["xls", "xlsx"].as_slice(),
        "powerpoint" => ["ppt", "pptx"].as_slice(),
        _ => unreachable!(),
    };
    if !expected_source.contains(&source_extension.as_str()) {
        bail!("Office source extension does not match the selected application");
    }
    let output_extension = normalized_extension(&output)?;
    if request.action == "export_pdf" {
        if output_extension != "pdf" {
            bail!("Office PDF export output must use the .pdf extension");
        }
    } else {
        let expected_output = match request.application.as_str() {
            "word" => "docx",
            "excel" => "xlsx",
            "powerpoint" => "pptx",
            _ => unreachable!(),
        };
        if output_extension != expected_output {
            bail!("edited Office output must use the modern non-macro format .{expected_output}");
        }
    }
    if !matches!(
        request.action.as_str(),
        "export_pdf"
            | "replace_text"
            | "word_append_paragraph"
            | "word_format_text"
            | "excel_set_cell"
            | "excel_format_range"
            | "excel_add_chart"
            | "powerpoint_add_slide"
            | "powerpoint_format_text"
    ) {
        bail!("unsupported Microsoft Office action {}", request.action);
    }
    if let Some(parent) = output.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    if let Some(preview) = &preview_output {
        if normalized_extension(preview)? != "pdf"
            || !request
                .preview_output
                .as_deref()
                .unwrap_or_default()
                .starts_with("artifacts/")
        {
            bail!("Office preview output must be a PDF below artifacts/");
        }
        if preview.exists() {
            bail!("Office preview output already exists; refusing to overwrite it");
        }
        if let Some(parent) = preview.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
    }

    let mut command = tokio::process::Command::new("powershell.exe");
    command
        .kill_on_drop(true)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            include_str!("windows_office.ps1"),
        ])
        .env("COWORK_OFFICE_APP", &request.application)
        .env("COWORK_OFFICE_ACTION", &request.action)
        .env("COWORK_OFFICE_SOURCE", &source)
        .env("COWORK_OFFICE_OUTPUT", &output)
        .env(
            "COWORK_OFFICE_PREVIEW_OUTPUT",
            preview_output
                .as_ref()
                .map(|path| path.as_os_str())
                .unwrap_or_default(),
        )
        .env(
            "COWORK_OFFICE_PARAMETERS",
            serde_json::to_string(&request.parameters)?,
        );
    let result = match tokio::time::timeout(Duration::from_secs(8 * 60), command.output()).await {
        Ok(result) => result.context("failed to start the interactive Office adapter")?,
        Err(_) => {
            cleanup_office_processes().await;
            bail!("Office automation timed out, likely because an unexpected dialog requires manual review");
        }
    };
    if !result.status.success() {
        cleanup_office_processes().await;
        bail!(
            "Office automation failed: {}",
            String::from_utf8_lossy(&result.stderr)
                .chars()
                .take(4_000)
                .collect::<String>()
        );
    }
    let mut artifacts = Vec::new();
    for (path, workspace_path) in std::iter::once((&output, request.output.as_str())).chain(
        preview_output
            .as_ref()
            .zip(request.preview_output.as_deref()),
    ) {
        let artifact_bytes = tokio::fs::read(path).await?;
        artifacts.push(
            client
                .upload_artifact(
                    lease,
                    None,
                    &workspace_path.replace('\\', "/"),
                    "MicrosoftOffice",
                    artifact_bytes,
                )
                .await?,
        );
    }
    Ok(json!({
        "application": request.application,
        "action": request.action,
        "source": request.source,
        "output": request.output,
        "artifacts": artifacts
    }))
}

#[cfg(not(windows))]
async fn execute_windows_office(
    _client: &ControlPlaneClient,
    _config: &Config,
    _lease: &RunLease,
) -> Result<Value> {
    bail!("Microsoft Office automation is only available on managed Windows executors")
}

fn safe_run_path(run_root: &Path, relative: &str) -> Result<PathBuf> {
    if relative.is_empty() || relative.contains('\\') {
        bail!("run workspace paths must use non-empty relative POSIX paths");
    }
    let path = Path::new(relative);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("run workspace paths must be relative and cannot contain parent segments");
    }
    Ok(run_root.join(path))
}

#[cfg(any(windows, test))]
fn normalized_extension(path: &Path) -> Result<String> {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| value.trim_start_matches('.'))
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
        .context("Office path must have a UTF-8 file extension")
}

#[cfg(any(windows, test))]
fn is_active_office_extension(extension: &str) -> bool {
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "docm" | "dotm" | "xlsm" | "xltm" | "xlam" | "pptm" | "potm" | "ppam" | "sldm"
    )
}

async fn inventory_workspace(root: &Path) -> Result<WorkspaceInventory> {
    let root = tokio::fs::canonicalize(root)
        .await
        .context("personal run workspace is unavailable")?;
    if !tokio::fs::metadata(&root).await?.is_dir() {
        bail!("personal run workspace is not a directory");
    }
    let mut walker = ignore::WalkBuilder::new(&root);
    walker
        .hidden(false)
        .follow_links(false)
        .parents(false)
        .ignore(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .add_custom_ignore_filename(".coworkignore");
    let mut paths = Vec::new();
    for entry in walker.build() {
        let entry = entry.context("failed to evaluate .coworkignore project boundary")?;
        if entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            paths.push(entry.into_path());
        }
    }
    paths.sort();
    let mut files = Vec::with_capacity(paths.len());
    let mut fingerprints = BTreeMap::new();
    let mut total_bytes = 0_u64;
    for path in paths {
        let relative = path
            .strip_prefix(&root)
            .context("workspace walker returned a path outside its root")?;
        let relative = relative
            .components()
            .map(|component| match component {
                Component::Normal(value) => value
                    .to_str()
                    .map(ToOwned::to_owned)
                    .context("snapshot paths must be valid UTF-8"),
                _ => bail!("snapshot path is not normalized"),
            })
            .collect::<Result<Vec<_>>>()?
            .join("/");
        if relative.is_empty() {
            bail!("snapshot path cannot be empty");
        }
        let metadata = tokio::fs::symlink_metadata(&path).await?;
        if !metadata.file_type().is_file() {
            bail!("workspace file changed type while it was inventoried: {relative}");
        }
        let (chunks, fingerprint, measured_size) = hash_snapshot_file(&path).await?;
        if measured_size != metadata.len() {
            bail!("workspace file changed size while it was inventoried: {relative}");
        }
        total_bytes = total_bytes
            .checked_add(measured_size)
            .context("workspace exceeds the supported snapshot size")?;
        if total_bytes > 20 * 1024 * 1024 * 1024_u64 {
            bail!("workspace exceeds the supported 20 GiB snapshot size");
        }
        fingerprints.insert(relative.clone(), fingerprint);
        files.push(SnapshotUploadFile {
            path: relative,
            size: measured_size,
            mode: snapshot_file_mode(&metadata),
            modified_at: metadata
                .modified()
                .map(chrono::DateTime::<chrono::Utc>::from)
                .context("workspace file has no usable modification time")?,
            chunks,
        });
    }
    Ok(WorkspaceInventory {
        root,
        files,
        fingerprints,
        total_bytes,
    })
}

async fn hash_snapshot_file(path: &Path) -> Result<(Vec<SnapshotUploadChunk>, String, u64)> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut buffer = vec![0_u8; SNAPSHOT_CHUNK_BYTES];
    let mut file_hash = Sha256::new();
    let mut chunks = Vec::new();
    let mut total = 0_u64;
    loop {
        let size = read_snapshot_chunk(&mut file, &mut buffer).await?;
        if size == 0 {
            break;
        }
        let bytes = &buffer[..size];
        file_hash.update(bytes);
        total = total
            .checked_add(size as u64)
            .context("snapshot file size overflow")?;
        chunks.push(SnapshotUploadChunk {
            digest: hex::encode(Sha256::digest(bytes)),
            plaintext_size: size as u64,
        });
    }
    Ok((chunks, hex::encode(file_hash.finalize()), total))
}

async fn read_snapshot_chunk(file: &mut tokio::fs::File, buffer: &mut [u8]) -> Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        let read = file.read(&mut buffer[filled..]).await?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    Ok(filled)
}

#[cfg(unix)]
fn snapshot_file_mode(metadata: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode()
}

#[cfg(not(unix))]
fn snapshot_file_mode(metadata: &std::fs::Metadata) -> u32 {
    if metadata.permissions().readonly() {
        0o444
    } else {
        0o644
    }
}

fn workspace_diff_summary(
    before: Option<&WorkspaceInventory>,
    after: &WorkspaceInventory,
) -> Value {
    let empty = BTreeMap::new();
    let before = before.map_or(&empty, |inventory| &inventory.fingerprints);
    let mut added = Vec::new();
    let mut modified = Vec::new();
    let mut deleted = Vec::new();
    for (path, fingerprint) in &after.fingerprints {
        match before.get(path) {
            None => added.push(path.clone()),
            Some(previous) if previous != fingerprint => modified.push(path.clone()),
            Some(_) => {}
        }
    }
    for path in before.keys() {
        if !after.fingerprints.contains_key(path) {
            deleted.push(path.clone());
        }
    }
    const PREVIEW_LIMIT: usize = 1_000;
    json!({
        "added_count": added.len(),
        "modified_count": modified.len(),
        "deleted_count": deleted.len(),
        "added": added.iter().take(PREVIEW_LIMIT).collect::<Vec<_>>(),
        "modified": modified.iter().take(PREVIEW_LIMIT).collect::<Vec<_>>(),
        "deleted": deleted.iter().take(PREVIEW_LIMIT).collect::<Vec<_>>(),
        "paths_truncated": added.len() > PREVIEW_LIMIT || modified.len() > PREVIEW_LIMIT || deleted.len() > PREVIEW_LIMIT,
    })
}

async fn publish_result_snapshot(
    client: &ControlPlaneClient,
    lease: &RunLease,
    inventory: &WorkspaceInventory,
) -> Result<Uuid> {
    let session = client.begin_result_snapshot(lease, inventory).await?;
    if session.max_chunk_bytes < SNAPSHOT_CHUNK_BYTES as u64
        && inventory
            .files
            .iter()
            .flat_map(|file| &file.chunks)
            .any(|chunk| chunk.plaintext_size > session.max_chunk_bytes)
    {
        bail!("control plane result-snapshot chunk limit changed during this run");
    }
    let missing: HashSet<&str> = session.missing_chunks.iter().map(String::as_str).collect();
    let canonical_root = tokio::fs::canonicalize(&inventory.root).await?;
    let mut uploaded = HashSet::new();
    for snapshot_file in &inventory.files {
        let path = safe_run_path(&inventory.root, &snapshot_file.path)?;
        let canonical_path = tokio::fs::canonicalize(&path)
            .await
            .with_context(|| format!("snapshot file disappeared: {}", snapshot_file.path))?;
        if !canonical_path.starts_with(&canonical_root)
            || tokio::fs::symlink_metadata(&path)
                .await?
                .file_type()
                .is_symlink()
        {
            bail!(
                "snapshot file escaped through a symlink: {}",
                snapshot_file.path
            );
        }
        let mut file = tokio::fs::File::open(&canonical_path).await?;
        let mut buffer = vec![0_u8; SNAPSHOT_CHUNK_BYTES];
        for expected in &snapshot_file.chunks {
            let size = read_snapshot_chunk(&mut file, &mut buffer).await?;
            let bytes = &buffer[..size];
            let digest = hex::encode(Sha256::digest(bytes));
            if size as u64 != expected.plaintext_size || digest != expected.digest {
                bail!("workspace changed while publishing {}", snapshot_file.path);
            }
            if missing.contains(expected.digest.as_str()) && uploaded.insert(digest.clone()) {
                client
                    .upload_result_chunk(lease, session.manifest_id, &digest, bytes.to_vec())
                    .await?;
            }
        }
        if read_snapshot_chunk(&mut file, &mut buffer).await? != 0 {
            bail!("workspace grew while publishing {}", snapshot_file.path);
        }
    }
    let manifest = client
        .commit_result_snapshot(lease, session.manifest_id)
        .await?;
    if manifest.id != session.manifest_id || manifest.project_id != lease.run.spec.project_id {
        bail!("control plane committed a different result snapshot");
    }
    Ok(manifest.id)
}

#[cfg(windows)]
async fn cleanup_office_processes() {
    // Managed executors are required to use a dedicated Windows account. If a
    // COM call hangs behind an unexpected modal dialog, remove only Office
    // processes owned by that account before returning the executor to health.
    let _ = tokio::process::Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-Process WINWORD,EXCEL,POWERPNT -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue",
        ])
        .output()
        .await;
}

async fn materialize_run_workspace(
    client: &ControlPlaneClient,
    config: &Config,
    lease: &RunLease,
) -> Result<PathBuf> {
    let run_root = config.workspace_root.join(lease.run.spec.id.to_string());
    let Some(snapshot_id) = lease.run.spec.snapshot_id else {
        tokio::fs::create_dir_all(&run_root).await?;
        return Ok(run_root);
    };
    if tokio::fs::try_exists(&run_root).await? {
        tokio::fs::remove_dir_all(&run_root).await?;
    }
    tokio::fs::create_dir_all(&run_root).await?;
    let manifest = client.run_snapshot(lease).await?;
    if manifest.id != snapshot_id || manifest.project_id != lease.run.spec.project_id {
        bail!("control plane returned a snapshot for a different run or project");
    }
    let mut materialized_bytes = 0_u64;
    for file in &manifest.files {
        let target = safe_run_path(&run_root, &file.path)?;
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut output = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&target)
            .await
            .with_context(|| format!("failed to create snapshot file {}", file.path))?;
        let mut file_bytes = 0_u64;
        for chunk in &file.chunks {
            let bytes = client.snapshot_chunk(lease, &chunk.digest).await?;
            if bytes.len() as u64 != chunk.plaintext_size
                || hex::encode(Sha256::digest(&bytes)) != chunk.digest
            {
                bail!(
                    "snapshot chunk {} failed local integrity verification",
                    chunk.digest
                );
            }
            output.write_all(&bytes).await?;
            file_bytes = file_bytes
                .checked_add(bytes.len() as u64)
                .context("snapshot file size overflow")?;
        }
        output.flush().await?;
        if file_bytes != file.size {
            bail!(
                "snapshot file {} materialized {file_bytes} bytes, expected {}",
                file.path,
                file.size
            );
        }
        materialized_bytes = materialized_bytes
            .checked_add(file_bytes)
            .context("snapshot size overflow")?;
    }
    if materialized_bytes != manifest.total_bytes {
        bail!(
            "snapshot materialized {materialized_bytes} bytes, expected {}",
            manifest.total_bytes
        );
    }
    Ok(run_root)
}

async fn cleanup_run_workspace(config: &Config, lease: &RunLease) -> Result<()> {
    let cleanup = config.kind == ExecutorKind::ManagedWindows
        || (config.local_daemon.is_some() && lease.run.spec.snapshot_id.is_some());
    if !cleanup {
        return Ok(());
    }
    let run_root = config.workspace_root.join(lease.run.spec.id.to_string());
    if run_root.parent() != Some(config.workspace_root.as_path()) {
        bail!("refusing to clean a run workspace outside the configured workspace root");
    }
    if tokio::fs::try_exists(&run_root).await? {
        tokio::fs::remove_dir_all(&run_root).await?;
    }
    #[cfg(windows)]
    {
        let _ = tokio::process::Command::new("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Set-Clipboard -Value $null",
            ])
            .output()
            .await;
    }
    Ok(())
}

async fn call_model(config: &Config, input: &Value) -> Result<ModelCallResult> {
    let prompt = input
        .get("prompt")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| input.to_string());
    let request = ChatCompletionRequest {
        model: &config.model_name,
        messages: [ChatMessage {
            role: "user",
            content: &prompt,
        }],
    };
    let endpoint = format!(
        "{}/chat/completions",
        config
            .model_base_url
            .as_deref()
            .context("device model endpoint is not configured")?
            .trim_end_matches('/')
    );
    let http = Client::new();
    let mut builder = http.post(endpoint).json(&request);
    if let Some(api_key) = &config.model_api_key {
        builder = builder.bearer_auth(api_key);
    }
    let response = builder.send().await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        bail!(
            "model endpoint returned {status}: {}",
            body.chars().take(2000).collect::<String>()
        );
    }
    let response: ChatCompletionResponse = serde_json::from_str(&body)?;
    let content = response
        .choices
        .into_iter()
        .next()
        .and_then(|choice| choice.message.content)
        .context("model response did not contain message content")?;
    Ok(ModelCallResult {
        content,
        usage: response.usage,
    })
}

fn load_executor_mcp_bindings(kind: ExecutorKind) -> Result<Vec<ExecutorMcpBinding>> {
    let Some(path) = optional("COWORK_MCP_BINDINGS_FILE") else {
        return Ok(Vec::new());
    };
    if kind != ExecutorKind::ManagedWindows {
        bail!("COWORK_MCP_BINDINGS_FILE is currently supported only for managed Windows executors");
    }
    let metadata = fs::symlink_metadata(&path)
        .with_context(|| format!("failed to inspect COWORK_MCP_BINDINGS_FILE {path}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("COWORK_MCP_BINDINGS_FILE must reference a regular non-symlink file");
    }
    if metadata.len() > MAX_EXECUTOR_MCP_FILE_BYTES {
        bail!("COWORK_MCP_BINDINGS_FILE exceeds {MAX_EXECUTOR_MCP_FILE_BYTES} bytes");
    }
    let bytes = fs::read(&path)
        .with_context(|| format!("failed to read COWORK_MCP_BINDINGS_FILE {path}"))?;
    let bindings = parse_executor_mcp_bindings(kind, &bytes)?;
    validate_executor_mcp_command_files(&bindings)?;
    Ok(bindings)
}

fn validate_executor_mcp_command_files(bindings: &[ExecutorMcpBinding]) -> Result<()> {
    for binding in bindings {
        if binding.transport == ExecutorMcpTransport::StreamableHttp {
            continue;
        }
        let command = Path::new(&binding.command);
        if !command.is_absolute() {
            bail!("managed Windows MCP commands must use absolute executable paths");
        }
        let metadata = fs::symlink_metadata(command).with_context(|| {
            format!(
                "failed to inspect MCP executable for binding {:?}",
                binding.name
            )
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            bail!("managed Windows MCP commands must reference regular non-symlink files");
        }
    }
    Ok(())
}

fn parse_executor_mcp_bindings(
    kind: ExecutorKind,
    bytes: &[u8],
) -> Result<Vec<ExecutorMcpBinding>> {
    if kind != ExecutorKind::ManagedWindows {
        bail!("executor MCP bindings are currently supported only for managed Windows");
    }
    if bytes.len() as u64 > MAX_EXECUTOR_MCP_FILE_BYTES {
        bail!("executor MCP binding payload exceeds {MAX_EXECUTOR_MCP_FILE_BYTES} bytes");
    }
    let mut bindings: Vec<ExecutorMcpBinding> =
        serde_json::from_slice(bytes).context("invalid executor MCP binding JSON")?;
    if bindings.is_empty() || bindings.len() > MAX_EXECUTOR_MCP_BINDINGS {
        bail!("executor MCP binding files require 1 to {MAX_EXECUTOR_MCP_BINDINGS} entries");
    }
    let mut names = HashSet::new();
    for binding in &mut bindings {
        binding.name = binding.name.trim().to_owned();
        binding.command = binding.command.trim().to_owned();
        binding.url = binding.url.trim().to_owned();
        if binding.name.is_empty()
            || binding.name.len() > 256
            || binding.name.chars().any(char::is_control)
            || !names.insert(binding.name.clone())
        {
            bail!("executor MCP binding names must be unique valid strings of at most 256 bytes");
        }
        match binding.transport {
            ExecutorMcpTransport::Stdio => {
                if !binding.url.is_empty() || !binding.headers.is_empty() {
                    bail!("stdio executor MCP bindings may not contain an HTTP URL or headers");
                }
                let normalized_command = binding.command.to_ascii_lowercase();
                if binding.command.is_empty()
                    || binding.command.len() > 32 * 1024
                    || binding.command.chars().any(|character| character == '\0')
                    || !(normalized_command.ends_with(".exe")
                        || normalized_command.ends_with(".com"))
                {
                    bail!("executor MCP binding commands must be direct .exe or .com executables");
                }
                if binding.args.len() > 256
                    || binding
                        .args
                        .iter()
                        .any(|argument| argument.len() > 64 * 1024 || argument.contains('\0'))
                {
                    bail!("executor MCP binding arguments exceed the safety limits");
                }
                if binding.environment.len() > 64
                    || binding.environment.iter().any(|(key, value)| {
                        key.is_empty()
                            || key.len() > 256
                            || key.chars().any(|character| {
                                character == '=' || character == '\0' || character.is_control()
                            })
                            || value.len() > 64 * 1024
                            || value.contains('\0')
                    })
                {
                    bail!("executor MCP binding environment exceeds the safety limits");
                }
            }
            ExecutorMcpTransport::StreamableHttp => {
                if !binding.command.is_empty()
                    || !binding.args.is_empty()
                    || !binding.environment.is_empty()
                {
                    bail!("streamable HTTP executor MCP bindings may not contain stdio fields");
                }
                validate_executor_http_endpoint(&binding.url, false)?;
                validate_executor_http_headers(&binding.headers)?;
            }
        }
    }
    bindings.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(bindings)
}

fn validate_executor_http_endpoint(endpoint: &str, allow_insecure_test: bool) -> Result<Url> {
    let url = Url::parse(endpoint).context("invalid executor MCP HTTP endpoint")?;
    let valid_scheme = url.scheme() == "https" || (allow_insecure_test && url.scheme() == "http");
    if !valid_scheme
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || (!allow_insecure_test && url.port_or_known_default() != Some(443))
    {
        bail!("executor MCP HTTP endpoints require HTTPS port 443 without userinfo, query, or fragment");
    }
    if !allow_insecure_test {
        let host = url.host_str().unwrap_or_default().trim_end_matches('.');
        let lower_host = host.to_ascii_lowercase();
        if host.parse::<IpAddr>().is_ok()
            || matches!(lower_host.as_str(), "localhost" | "localhost.localdomain")
            || [".localhost", ".local", ".internal", ".home.arpa"]
                .iter()
                .any(|suffix| lower_host.ends_with(suffix))
        {
            bail!("executor MCP HTTP endpoints must use a public DNS hostname");
        }
    }
    Ok(url)
}

fn validate_executor_http_headers(headers: &BTreeMap<String, String>) -> Result<HeaderMap> {
    if headers.len() > 64 {
        bail!("executor MCP HTTP bindings support at most 64 headers");
    }
    let mut result = HeaderMap::new();
    let mut names = HashSet::new();
    for (name, value) in headers {
        let lower_name = name.to_ascii_lowercase();
        if name.is_empty()
            || name.len() > 256
            || value.len() > 64 * 1024
            || !names.insert(lower_name.clone())
            || is_reserved_executor_mcp_header(&lower_name)
        {
            bail!("executor MCP HTTP header is invalid, duplicated, or reserved");
        }
        let name = HeaderName::from_bytes(name.as_bytes())
            .context("executor MCP HTTP header name is invalid")?;
        let value = ReqwestHeaderValue::from_str(value)
            .context("executor MCP HTTP header value is invalid")?;
        result.insert(name, value);
    }
    Ok(result)
}

fn is_reserved_executor_mcp_header(name: &str) -> bool {
    matches!(
        name,
        "accept"
            | "connection"
            | "content-length"
            | "content-type"
            | "host"
            | "http-proxy"
            | "https-proxy"
            | "mcp-protocol-version"
            | "mcp-session-id"
            | "origin"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

fn required(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("missing required environment variable {name}"))
}

fn required_secret(name: &str) -> Result<String> {
    if let Some(value) = optional(name) {
        return Ok(value);
    }
    let file_name = format!("{name}_FILE");
    let path = required(&file_name)?;
    let value = fs::read_to_string(&path)
        .with_context(|| format!("failed to read {file_name} secret file {path}"))?;
    let value = value.trim().to_owned();
    if value.is_empty() {
        bail!("{file_name} points to an empty secret file");
    }
    Ok(value)
}

fn optional(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn value_or(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_owned())
}

fn parse_personal_remote_control_mode(value: &str) -> Result<PersonalDeviceRemoteControlMode> {
    match value {
        "off" => Ok(PersonalDeviceRemoteControlMode::Off),
        "confirm_each_session" => Ok(PersonalDeviceRemoteControlMode::ConfirmEachSession),
        "unattended" => Ok(PersonalDeviceRemoteControlMode::Unattended),
        other => bail!(
            "COWORK_PERSONAL_REMOTE_CONTROL must be off, confirm_each_session, or unattended; got {other}"
        ),
    }
}

fn personal_remote_control_mode_name(mode: PersonalDeviceRemoteControlMode) -> &'static str {
    match mode {
        PersonalDeviceRemoteControlMode::Off => "off",
        PersonalDeviceRemoteControlMode::ConfirmEachSession => "confirm_each_session",
        PersonalDeviceRemoteControlMode::Unattended => "unattended",
    }
}

fn validated_server_url(value: &str) -> Result<String> {
    let url = reqwest::Url::parse(value.trim()).context("invalid COWORK_SERVER_URL")?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"));
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        bail!("remote executor connections require an HTTPS server URL");
    }
    Ok(value.trim().trim_end_matches('/').to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_paths_reject_traversal_absolute_and_windows_separators() {
        let root = Path::new("workspace");
        assert_eq!(
            safe_run_path(root, "documents/input.docx").unwrap(),
            root.join("documents/input.docx")
        );
        for invalid in [
            "",
            "../secret",
            "/absolute",
            "documents\\input.docx",
            "./file",
        ] {
            assert!(safe_run_path(root, invalid).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn office_extensions_are_case_insensitive_and_active_content_is_blocked() {
        assert_eq!(
            normalized_extension(Path::new("REPORT.DOCX")).unwrap(),
            "docx"
        );
        for extension in ["docm", "DOTM", "xlsm", "xlam", "pptm", "ppam"] {
            assert!(is_active_office_extension(extension));
        }
        assert!(!is_active_office_extension("docx"));
        assert!(!is_active_office_extension("xlsx"));
        assert!(!is_active_office_extension("pptx"));
    }

    #[test]
    fn remote_servers_require_tls_except_on_loopback() {
        assert!(validated_server_url("https://cowork.example.test/").is_ok());
        assert!(validated_server_url("http://127.0.0.1:8080").is_ok());
        assert!(validated_server_url("http://cowork.example.test").is_err());
        assert!(validated_server_url("file:///tmp/cowork").is_err());
    }

    #[test]
    fn personal_remote_control_modes_are_strict_and_safe_by_default() {
        assert_eq!(
            PersonalDeviceRemoteControlMode::default(),
            PersonalDeviceRemoteControlMode::ConfirmEachSession
        );
        assert_eq!(
            parse_personal_remote_control_mode("off").unwrap(),
            PersonalDeviceRemoteControlMode::Off
        );
        assert_eq!(
            parse_personal_remote_control_mode("confirm_each_session").unwrap(),
            PersonalDeviceRemoteControlMode::ConfirmEachSession
        );
        assert_eq!(
            parse_personal_remote_control_mode("unattended").unwrap(),
            PersonalDeviceRemoteControlMode::Unattended
        );
        assert!(parse_personal_remote_control_mode("yes").is_err());
        assert_eq!(
            personal_remote_control_mode_name(PersonalDeviceRemoteControlMode::ConfirmEachSession),
            "confirm_each_session"
        );
    }

    #[test]
    fn metadata_outbox_operations_are_stable_and_revision_based() {
        let user_id = Uuid::new_v4();
        let device_id = Uuid::new_v4();
        assert_eq!(
            stable_sync_operation_id(device_id, 42),
            stable_sync_operation_id(device_id, 42)
        );
        assert_ne!(
            stable_sync_operation_id(device_id, 42),
            stable_sync_operation_id(device_id, 43)
        );
        let entity_id = Uuid::new_v4();
        let change = LocalSyncChange {
            cursor: 42,
            entity_type: "memory".to_owned(),
            entity_id: entity_id.to_string(),
            revision: 3,
            operation: "upsert".to_owned(),
            entity: LocalSyncEntity {
                payload: json!({"content": "durable"}),
                tombstone: false,
            },
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        let outgoing = local_change_for_server(user_id, device_id, &change).unwrap();
        assert_eq!(outgoing.entity_id, entity_id);
        assert_eq!(outgoing.base_revision, 2);
        assert_eq!(outgoing.operation, SyncOperation::Upsert);
        assert_eq!(outgoing.payload, Some(json!({"content": "durable"})));
    }

    #[test]
    fn metadata_sync_maps_legacy_ids_per_user_and_preserves_local_identity() {
        let user_id = Uuid::new_v4();
        let other_user_id = Uuid::new_v4();
        let device_id = Uuid::new_v4();
        let change = LocalSyncChange {
            cursor: 9,
            entity_type: "provider_profile".to_owned(),
            entity_id: "default-ollama".to_owned(),
            revision: 1,
            operation: "upsert".to_owned(),
            entity: LocalSyncEntity {
                payload: json!({"name": "Local Ollama", "model": "llama3.1:8b"}),
                tombstone: false,
            },
            created_at: chrono::Utc::now().to_rfc3339(),
        };

        let outgoing = local_change_for_server(user_id, device_id, &change).unwrap();
        assert_eq!(
            outgoing.entity_id,
            stable_sync_entity_id(user_id, "provider_profile", "default-ollama")
        );
        assert_ne!(
            outgoing.entity_id,
            stable_sync_entity_id(other_user_id, "provider_profile", "default-ollama")
        );
        assert_eq!(
            outgoing.payload.unwrap()["_cowork_local_entity_id"],
            "default-ollama"
        );
    }

    #[test]
    fn schedule_metadata_maps_a_legacy_provider_reference() {
        let user_id = Uuid::new_v4();
        let payload = sync_payload_for_server(
            user_id,
            "schedule",
            &Uuid::new_v4().to_string(),
            &json!({"model_profile_id": "default-openai-compatible"}),
        )
        .unwrap();
        assert_eq!(
            payload["_cowork_local_model_profile_id"],
            "default-openai-compatible"
        );
        assert_eq!(
            payload["model_profile_id"],
            stable_sync_entity_id(user_id, "provider_profile", "default-openai-compatible")
                .to_string()
        );
        assert_eq!(
            sync_payload_for_local("schedule", &payload),
            json!({"model_profile_id": "default-openai-compatible"})
        );
    }

    #[test]
    fn remote_metadata_strips_internal_legacy_identity() {
        assert_eq!(
            sync_payload_for_local(
                "provider_profile",
                &json!({
                    "name": "Local Ollama",
                    "model": "llama3.1:8b",
                    "_cowork_local_entity_id": "default-ollama"
                }),
            ),
            json!({"name": "Local Ollama", "model": "llama3.1:8b"})
        );
    }

    #[test]
    fn managed_windows_mcp_bindings_are_bounded_unique_and_secret_redacted() {
        let bindings = parse_executor_mcp_bindings(
            ExecutorKind::ManagedWindows,
            br#"[
                {"name":" Docs ","command":" C:\\MCP\\docs.exe ","args":["--stdio"],
                 "environment":{"MCP_TOKEN":"executor-secret-value"}}
            ]"#,
        )
        .unwrap();
        assert_eq!(bindings[0].name, "Docs");
        assert_eq!(bindings[0].command, "C:\\MCP\\docs.exe");
        assert_eq!(
            redact_executor_secrets(
                "token=executor-secret-value",
                &bindings[0]
                    .environment
                    .values()
                    .cloned()
                    .collect::<Vec<_>>(),
            ),
            "token=[REDACTED]"
        );
        assert!(parse_executor_mcp_bindings(
            ExecutorKind::PersonalDevice,
            br#"[{"name":"Docs","command":"docs.exe"}]"#,
        )
        .is_err());
        assert!(parse_executor_mcp_bindings(
            ExecutorKind::ManagedWindows,
            br#"[
                {"name":"Docs","command":"one.exe"},
                {"name":"Docs","command":"two.exe"}
            ]"#,
        )
        .is_err());
        assert!(parse_executor_mcp_bindings(
            ExecutorKind::ManagedWindows,
            br#"[{"name":"Docs","command":"C:\\MCP\\docs.cmd"}]"#,
        )
        .is_err());
    }

    #[test]
    fn managed_windows_streamable_http_bindings_are_transport_safe() {
        let bindings = parse_executor_mcp_bindings(
            ExecutorKind::ManagedWindows,
            br#"[{
                "name":"Remote Docs",
                "transport":"streamable_http",
                "url":"https://mcp.example.com/mcp",
                "headers":{"Authorization":"Bearer executor-http-secret"}
            }]"#,
        )
        .unwrap();
        assert_eq!(bindings[0].transport, ExecutorMcpTransport::StreamableHttp);
        assert_eq!(bindings[0].url, "https://mcp.example.com/mcp");
        assert_eq!(
            redact_executor_secrets(
                "credential=Bearer executor-http-secret",
                &bindings[0].secret_values().cloned().collect::<Vec<_>>(),
            ),
            "credential=[REDACTED]"
        );

        let invalid: &[&[u8]] = &[
            br#"[{"name":"Remote","transport":"streamable_http","url":"http://mcp.example.com/mcp"}]"#,
            br#"[{"name":"Remote","transport":"streamable_http","url":"https://127.0.0.1/mcp"}]"#,
            br#"[{"name":"Remote","transport":"streamable_http","url":"https://service.internal/mcp"}]"#,
            br#"[{"name":"Remote","transport":"streamable_http","url":"https://mcp.example.com:8443/mcp"}]"#,
            br#"[{"name":"Remote","transport":"streamable_http","url":"https://mcp.example.com/mcp?token=unsafe"}]"#,
            br#"[{"name":"Remote","transport":"streamable_http","url":"https://mcp.example.com/mcp","headers":{"MCP-Session-Id":"override"}}]"#,
        ];
        for invalid in invalid {
            assert!(parse_executor_mcp_bindings(ExecutorKind::ManagedWindows, invalid).is_err());
        }
    }

    #[test]
    fn managed_windows_sse_parser_matches_responses_and_rejects_server_requests() {
        let mut event_id = None;
        let mut retry_ms = 0;
        let response = parse_executor_sse_event(
            b"id: event-7\nretry: 120\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"content\":\"ok\"}}\n",
            2,
            &mut event_id,
            &mut retry_ms,
        )
        .unwrap()
        .unwrap();
        assert_eq!(response["result"]["content"], "ok");
        assert_eq!(event_id.as_deref(), Some("event-7"));
        assert_eq!(retry_ms, 120);
        assert!(parse_executor_sse_event(
            b"data: {\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"roots/list\",\"params\":{}}\n",
            2,
            &mut None,
            &mut 0,
        )
        .is_err());
        assert!(parse_executor_sse_event(b"id: invalid\x7f\n", 2, &mut None, &mut 0,).is_err());
    }

    #[tokio::test]
    async fn managed_windows_streamable_http_executes_session_bound_persistent_sse() {
        use std::sync::{Arc, Mutex as StdMutex};

        async fn handle_fixture(
            mut socket: tokio::net::TcpStream,
            records: Arc<StdMutex<Vec<Value>>>,
        ) {
            let mut request = Vec::new();
            let header_end = loop {
                if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    break index + 4;
                }
                let mut chunk = [0_u8; 4096];
                let read = socket.read(&mut chunk).await.unwrap();
                assert!(read > 0, "fixture connection closed before HTTP headers");
                request.extend_from_slice(&chunk[..read]);
                assert!(request.len() <= MAX_MCP_MESSAGE_BYTES);
            };
            let head = std::str::from_utf8(&request[..header_end]).unwrap();
            let mut lines = head.split("\r\n");
            let request_line = lines.next().unwrap();
            let http_method = request_line.split_whitespace().next().unwrap().to_owned();
            let mut headers = HashMap::new();
            for line in lines.filter(|line| !line.is_empty()) {
                if let Some((name, value)) = line.split_once(':') {
                    headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
                }
            }
            let content_length = headers
                .get("content-length")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(0);
            while request.len() < header_end + content_length {
                let mut chunk = [0_u8; 4096];
                let read = socket.read(&mut chunk).await.unwrap();
                assert!(read > 0, "fixture connection closed before HTTP body");
                request.extend_from_slice(&chunk[..read]);
            }
            let message = if content_length == 0 {
                Value::Null
            } else {
                serde_json::from_slice(&request[header_end..header_end + content_length]).unwrap()
            };
            let rpc_method = message
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or(http_method.as_str())
                .to_owned();
            records.lock().unwrap().push(json!({
                "method":rpc_method,
                "session":headers.get("mcp-session-id"),
                "protocol":headers.get("mcp-protocol-version"),
                "token":headers.get("x-test-token"),
            }));

            let (status, content_type, body, session, persistent_sse) = match rpc_method.as_str() {
                "initialize" => (
                    "200 OK",
                    "application/json",
                    serde_json::to_vec(&json!({
                        "jsonrpc":"2.0",
                        "id":message["id"],
                        "result":{
                            "protocolVersion":"2025-11-25",
                            "capabilities":{"tools":{}}
                        }
                    }))
                    .unwrap(),
                    true,
                    false,
                ),
                "notifications/initialized" => {
                    ("202 Accepted", "application/json", Vec::new(), false, false)
                }
                "tools/call" => (
                    "200 OK",
                    "text/event-stream",
                    format!(
                        "id: windows-event\ndata: {}\n\n",
                        json!({
                            "jsonrpc":"2.0",
                            "id":message["id"],
                            "result":{
                                "content":[{"type":"text","text":"Windows HTTP fixture"}],
                                "structuredContent":message["params"]["arguments"],
                                "isError":false
                            }
                        })
                    )
                    .into_bytes(),
                    false,
                    true,
                ),
                "DELETE" => ("200 OK", "application/json", b"{}".to_vec(), false, false),
                other => panic!("unexpected fixture method {other}"),
            };
            let mut response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nConnection: close\r\n"
            );
            if session {
                response.push_str("MCP-Session-Id: windows-fixture-session\r\n");
            }
            if !persistent_sse {
                response.push_str(&format!("Content-Length: {}\r\n", body.len()));
            }
            response.push_str("\r\n");
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.write_all(&body).await.unwrap();
            socket.flush().await.unwrap();
            if persistent_sse {
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let records = Arc::new(StdMutex::new(Vec::new()));
        let server_records = records.clone();
        let server = tokio::spawn(async move {
            let mut handlers = Vec::new();
            for _ in 0..4 {
                let (socket, _) = listener.accept().await.unwrap();
                let records = server_records.clone();
                handlers.push(tokio::spawn(handle_fixture(socket, records)));
            }
            for handler in handlers {
                handler.await.unwrap();
            }
        });
        let binding = ExecutorMcpBinding {
            name: "HTTP fixture".to_owned(),
            transport: ExecutorMcpTransport::StreamableHttp,
            command: String::new(),
            args: Vec::new(),
            environment: BTreeMap::new(),
            url: format!("http://127.0.0.1:{port}/mcp"),
            headers: BTreeMap::from([("X-Test-Token".to_owned(), "executor-secret".to_owned())]),
        };
        let started = std::time::Instant::now();
        let response = invoke_executor_http_mcp_with_policy(
            &binding,
            "lookup",
            json!({"query":"hello"}),
            true,
        )
        .await
        .unwrap();
        let elapsed = started.elapsed();
        server.await.unwrap();

        assert_eq!(response["protocol_version"], "2025-11-25");
        assert_eq!(
            response["result"]["structuredContent"],
            json!({"query":"hello"})
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "client waited for persistent SSE closure: {elapsed:?}"
        );
        let records = records.lock().unwrap();
        assert_eq!(
            records
                .iter()
                .map(|record| record["method"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "initialize",
                "notifications/initialized",
                "tools/call",
                "DELETE"
            ]
        );
        assert!(records[0]["session"].is_null());
        assert!(records[0]["protocol"].is_null());
        for record in records.iter().skip(1) {
            assert_eq!(record["session"], "windows-fixture-session");
            assert_eq!(record["protocol"], "2025-11-25");
            assert_eq!(record["token"], "executor-secret");
        }
    }

    #[test]
    fn selected_executor_mcp_names_are_exact_deduplicated_and_sorted() {
        assert_eq!(
            selected_executor_mcp_names(&json!({
                "frozen_runtime_context":{"mcp_metadata":[
                    {"definition":{"name":"Zebra"}},
                    {"definition":{"name":"Docs"}},
                    {"definition":{"name":"Docs"}}
                ]}
            }))
            .unwrap(),
            vec!["Docs", "Zebra"]
        );
        assert!(selected_executor_mcp_names(&json!({
            "frozen_runtime_context":{"mcp_metadata":[{"definition":{"name":"\n"}}]}
        }))
        .is_err());
    }

    #[tokio::test]
    async fn native_mcp_rpc_ignores_notifications_and_returns_matching_response() {
        let (client, server) = tokio::io::duplex(16 * 1024);
        let (client_read, mut client_write) = tokio::io::split(client);
        let (server_read, mut server_write) = tokio::io::split(server);
        let mut client_read = BufReader::new(client_read);
        let server_task = tokio::spawn(async move {
            let mut server_read = BufReader::new(server_read);
            let request = read_executor_mcp_line(&mut server_read)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(serde_json::from_slice::<Value>(&request).unwrap()["id"], 7);
            write_executor_mcp_message(
                &mut server_write,
                &json!({"jsonrpc":"2.0","method":"notifications/progress","params":{}}),
            )
            .await
            .unwrap();
            write_executor_mcp_message(
                &mut server_write,
                &json!({"jsonrpc":"2.0","id":7,"result":{"content":"ok"}}),
            )
            .await
            .unwrap();
        });

        let response = executor_mcp_request(
            &mut client_write,
            &mut client_read,
            7,
            "tools/call",
            json!({"name":"lookup","arguments":{}}),
        )
        .await
        .unwrap();

        assert_eq!(response, json!({"content":"ok"}));
        server_task.await.unwrap();
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn managed_windows_mcp_process_uses_stdio_binding_environment_and_workspace() {
        let workspace = env::temp_dir().join(format!("cowork-agent-mcp-{}", Uuid::new_v4()));
        fs::create_dir_all(&workspace).unwrap();
        let script = r#"
$ErrorActionPreference = 'Stop'
while (($line = [Console]::In.ReadLine()) -ne $null) {
  $request = $line | ConvertFrom-Json
  if ($request.method -eq 'initialize') {
    $response = @{ jsonrpc = '2.0'; id = $request.id; result = @{ protocolVersion = '2024-11-05' } }
    [Console]::Out.WriteLine(($response | ConvertTo-Json -Compress -Depth 10))
    [Console]::Out.Flush()
  } elseif ($request.method -eq 'tools/call') {
    $text = $env:MCP_TEST_SECRET + '|' + (Get-Location).Path
    $response = @{ jsonrpc = '2.0'; id = $request.id; result = @{ content = @(@{ type = 'text'; text = $text }); isError = $false } }
    [Console]::Out.WriteLine(($response | ConvertTo-Json -Compress -Depth 10))
    [Console]::Out.Flush()
    break
  }
}
"#;
        let binding = ExecutorMcpBinding {
            name: "fixture".to_owned(),
            transport: ExecutorMcpTransport::Stdio,
            command: "powershell.exe".to_owned(),
            args: vec![
                "-NoProfile".to_owned(),
                "-NonInteractive".to_owned(),
                "-Command".to_owned(),
                script.to_owned(),
            ],
            environment: BTreeMap::from([(
                "MCP_TEST_SECRET".to_owned(),
                "executor-only-value".to_owned(),
            )]),
            url: String::new(),
            headers: BTreeMap::new(),
        };

        let response = invoke_executor_mcp(&binding, "fixture_tool", json!({}), &workspace)
            .await
            .unwrap();
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("executor-only-value|"));
        assert!(text.ends_with(workspace.file_name().unwrap().to_str().unwrap()));
        fs::remove_dir_all(workspace).unwrap();
    }

    #[tokio::test]
    async fn workspace_inventory_enforces_coworkignore_and_reports_content_changes() {
        let root = env::temp_dir().join(format!("cowork-agent-inventory-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(
            root.join(".coworkignore"),
            "ignored.txt\nnested/private-*\n",
        )
        .unwrap();
        fs::write(root.join("kept.txt"), "before").unwrap();
        fs::write(root.join("ignored.txt"), "secret").unwrap();
        fs::write(root.join("nested/private-token"), "secret").unwrap();
        let before = inventory_workspace(&root).await.unwrap();
        assert!(before.fingerprints.contains_key("kept.txt"));
        assert!(!before.fingerprints.contains_key("ignored.txt"));
        assert!(!before.fingerprints.contains_key("nested/private-token"));

        fs::write(root.join("kept.txt"), "after").unwrap();
        fs::write(root.join("created.txt"), "created").unwrap();
        let after = inventory_workspace(&root).await.unwrap();
        let diff = workspace_diff_summary(Some(&before), &after);
        assert_eq!(diff["modified_count"], 1);
        assert_eq!(diff["added_count"], 1);
        assert_eq!(diff["deleted_count"], 0);
        fs::remove_dir_all(root).unwrap();
    }
}
