use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env, fs,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use anyhow::{bail, Context, Result};
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
use cowork_runtime::crew::{prepare_crew_request, CrewModelConfig};
use futures_util::{SinkExt, StreamExt};
use reqwest::{Client, Method};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
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

mod windows_desktop;

const SNAPSHOT_CHUNK_BYTES: usize = 16 * 1024 * 1024;

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

#[derive(Debug, Clone)]
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
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ChatResponseMessage {
    content: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "cowork_device_agent=info".into()),
        )
        .init();
    let config = Config::from_env()?;
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
        let capabilities: Vec<CapabilityDescriptor> =
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
        && lease.run.spec.input.get("windows_office").is_some()
    {
        return Ok(LeaseExecution {
            result: execute_windows_office(client, config, lease).await?,
            result_snapshot_manifest_id: None,
            result_diff_summary: Value::Null,
        });
    }
    if let Some(daemon) = &config.local_daemon {
        return execute_via_local_daemon(client, config, daemon, lease).await;
    }
    let content = call_model(config, &lease.run.spec.input).await?;
    Ok(LeaseExecution {
        result: json!({"content": content}),
        result_snapshot_manifest_id: None,
        result_diff_summary: Value::Null,
    })
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

async fn call_model(config: &Config, input: &Value) -> Result<String> {
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
    response
        .choices
        .into_iter()
        .next()
        .and_then(|choice| choice.message.content)
        .context("model response did not contain message content")
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
