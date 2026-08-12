use std::{
    env, fs,
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    process::Stdio,
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::{DateTime, TimeDelta, Utc};
use chrono_tz::Tz;
use cowork_contracts::{
    ensure_compatible, Capability, CreateRunRequest, ExecutorTarget, FrozenReference,
    ListRunsResponse, RunError, RunEvent, RunEventKind, RunRecord, RunSpec, RunState,
    SCHEMA_VERSION,
};
use cowork_runtime::{
    AgentRuntime, ModelConfig, RuntimeHost, ToolDefinition, ToolInvocation, ToolOutput,
};
use cron::Schedule;
use fs2::FileExt;
use rand::{rngs::OsRng, RngCore};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    sync::{watch, Mutex},
};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[cfg(windows)]
mod shutdown_windows;

// MCP is deliberately hosted by the user daemon instead of the WebView so a
// stdio server stays available after the desktop window has been closed.  The
// implementation is shared with the desktop shell until the remaining native
// tools have moved into their own workspace crate.
mod codex;
mod desktop;
#[path = "../../../app/src-tauri/src/developer_browser.rs"]
#[allow(dead_code)]
mod developer_browser;
#[path = "../../../app/src-tauri/src/mcp.rs"]
#[allow(dead_code)]
mod mcp;
#[path = "../../../app/src-tauri/src/network_safety.rs"]
mod network_safety;
mod office;
mod sync_ipc;

const MAX_IPC_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const LEGACY_LOCAL_USER_ID: &str = "00000000-0000-0000-0000-000000000001";

#[derive(Debug, Clone)]
struct Config {
    database_path: PathBuf,
    ipc_endpoint: String,
    ipc_token: String,
    user_id: Uuid,
    device_id: Uuid,
    model_secret_key: [u8; 32],
    fallback_model: PersistedModelConfig,
    runtime_paths: RuntimePaths,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RuntimePaths {
    #[serde(default)]
    crew_python: PathBuf,
    #[serde(default)]
    crew_script: PathBuf,
    #[serde(default)]
    codex_root: PathBuf,
    #[serde(default)]
    codex_profiles: PathBuf,
}

#[derive(Clone)]
struct Daemon {
    config: Arc<Config>,
    database: Arc<Mutex<Connection>>,
    shutdown: watch::Sender<bool>,
    browser: Arc<developer_browser::DeveloperBrowserState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedModelConfig {
    base_url: String,
    api_key: Option<String>,
    model: String,
    timeout_ms: u64,
    max_steps: usize,
    #[serde(default = "default_verify_tls")]
    verify_tls_certificates: bool,
    #[serde(default)]
    mcp_servers: Vec<PersistedMcpServer>,
    #[serde(default)]
    crew_request: Option<Value>,
    #[serde(default)]
    codex_request: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PersistedMcpServer {
    name: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedScheduleTemplate {
    request: CreateRunRequest,
    model_config: PersistedModelConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ProviderDeviceBinding {
    base_url: String,
    api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProviderBindingUpsertRequest {
    profile_id: String,
    base_url: String,
    api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct McpBindingUpsertRequest {
    server_id: String,
    name: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: std::collections::HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct ScheduleUpsertRequest {
    id: Uuid,
    expression: String,
    timezone: String,
    enabled: bool,
    run_request: CreateRunRequest,
    model_config: PersistedModelConfig,
}

#[derive(Debug, Deserialize)]
struct ImportServerRunRequest {
    run_spec: RunSpec,
    #[serde(default)]
    model_config: Option<PersistedModelConfig>,
    #[serde(default)]
    workspace_path: Option<PathBuf>,
    #[serde(default)]
    defer_start: bool,
}

#[derive(Debug)]
struct DueSchedule {
    id: Uuid,
    expression: String,
    timezone: String,
    due_at: DateTime<Utc>,
    encrypted_template: String,
}

fn default_verify_tls() -> bool {
    true
}

impl PersistedModelConfig {
    fn validate(&self) -> Result<()> {
        let url = reqwest::Url::parse(&self.base_url).context("invalid model base URL")?;
        if !matches!(url.scheme(), "http" | "https") {
            bail!("model base URL must use HTTP or HTTPS");
        }
        if self.model.trim().is_empty() || self.model.len() > 512 {
            bail!("model name is missing or too long");
        }
        if !(1_000..=24 * 60 * 60 * 1_000).contains(&self.timeout_ms) {
            bail!("model timeout must be between one second and 24 hours");
        }
        if self.max_steps == 0 || self.max_steps > 512 {
            bail!("model max_steps must be between 1 and 512");
        }
        if self
            .api_key
            .as_ref()
            .is_some_and(|key| key.len() > 64 * 1024)
        {
            bail!("model API key is too long");
        }
        if self.mcp_servers.len() > 64 {
            bail!("too many MCP servers configured for one run");
        }
        for server in &self.mcp_servers {
            validate_mcp_binding(server)?;
        }
        if let Some(request) = &self.crew_request {
            if !request.is_object() {
                bail!("Crew runtime request must be an object");
            }
            if serde_json::to_vec(request)?.len() > 8 * 1024 * 1024 {
                bail!("Crew runtime request exceeds 8 MiB");
            }
        }
        if let Some(request) = &self.codex_request {
            if !request.is_object() {
                bail!("Codex runtime request must be an object");
            }
            if serde_json::to_vec(request)?.len() > 8 * 1024 * 1024 {
                bail!("Codex runtime request exceeds 8 MiB");
            }
        }
        if self.crew_request.is_some() && self.codex_request.is_some() {
            bail!("a run cannot use CrewAI and Codex adapters together");
        }
        Ok(())
    }

    fn runtime_config(&self) -> ModelConfig {
        ModelConfig {
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            model: self.model.clone(),
            timeout: Duration::from_millis(self.timeout_ms),
            max_steps: self.max_steps,
            verify_tls_certificates: self.verify_tls_certificates,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct IpcRequest {
    id: Value,
    token: String,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct IpcResponse {
    id: Value,
    result: Option<Value>,
    error: Option<IpcError>,
}

#[derive(Debug, Serialize)]
struct IpcError {
    code: &'static str,
    message: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "cowork_local_daemon=info".into()),
        )
        .init();
    let config = Arc::new(Config::from_env()?);
    if let Some(parent) = config.database_path.parent() {
        create_private_dir(parent)?;
    }
    let data_dir = config
        .database_path
        .parent()
        .context("daemon database has no parent directory")?;
    let replace_existing = env::args().skip(1).any(|argument| argument == "--replace");
    if replace_existing {
        let _ = request_existing_shutdown(&config).await;
    }
    let _instance_lock = if replace_existing {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            match acquire_instance_lock(data_dir) {
                Ok(lock) => break lock,
                Err(error) if tokio::time::Instant::now() < deadline => {
                    tracing::debug!(?error, "waiting for the previous daemon to checkpoint");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(error) => return Err(error.context("timed out replacing the prior daemon")),
            }
        }
    } else {
        acquire_instance_lock(data_dir)?
    };
    let connection = Connection::open(&config.database_path)?;
    initialize_database(&connection)?;
    let migrated_runs = migrate_legacy_creator_user_id(&connection, config.user_id)?;
    if migrated_runs > 0 {
        tracing::info!(migrated_runs, "migrated legacy local run creator IDs");
    }
    interrupt_active_runs(&connection, "daemon_restarted")?;
    config.fallback_model.validate()?;
    let (shutdown, mut shutdown_receiver) = watch::channel(false);
    let daemon = Daemon {
        config,
        database: Arc::new(Mutex::new(connection)),
        shutdown,
        browser: Arc::new(developer_browser::DeveloperBrowserState::default()),
    };

    let worker = daemon.clone();
    let worker_task = tokio::spawn(async move { worker_loop(worker).await });
    let scheduler = daemon.clone();
    let scheduler_task = tokio::spawn(async move { scheduler_loop(scheduler).await });
    let ipc_daemon = daemon.clone();
    let mut ipc_task = tokio::spawn(async move { serve_ipc(ipc_daemon).await });
    let ipc_result = tokio::select! {
        result = &mut ipc_task => Some(result.context("local IPC task panicked")?),
        _ = wait_for_ipc_shutdown(&mut shutdown_receiver) => None,
        _ = platform_shutdown_signal() => None,
    };
    if ipc_result.is_none() {
        ipc_task.abort();
        let _ = ipc_task.await;
    }
    worker_task.abort();
    let _ = worker_task.await;
    scheduler_task.abort();
    let _ = scheduler_task.await;
    {
        let database = daemon.database.lock().await;
        interrupt_active_runs(&database, "daemon_shutdown")?;
    }
    tracing::info!("local daemon shutdown checkpoint completed");
    ipc_result.unwrap_or(Ok(()))
}

async fn wait_for_ipc_shutdown(receiver: &mut watch::Receiver<bool>) {
    loop {
        if *receiver.borrow() {
            return;
        }
        if receiver.changed().await.is_err() {
            return;
        }
    }
}

async fn request_existing_shutdown(config: &Config) -> Result<()> {
    let request = IpcRequest {
        id: json!(Uuid::new_v4()),
        token: config.ipc_token.clone(),
        method: "daemon.shutdown".to_owned(),
        params: Value::Null,
    };
    #[cfg(windows)]
    {
        use tokio::net::windows::named_pipe::ClientOptions;
        let stream = ClientOptions::new()
            .open(&config.ipc_endpoint)
            .context("previous daemon pipe is unavailable")?;
        send_shutdown_request(stream, request).await
    }
    #[cfg(unix)]
    {
        let stream = tokio::net::UnixStream::connect(&config.ipc_endpoint)
            .await
            .context("previous daemon socket is unavailable")?;
        send_shutdown_request(stream, request).await
    }
}

async fn send_shutdown_request<T>(stream: T, request: IpcRequest) -> Result<()>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    let (reader, mut writer) = tokio::io::split(stream);
    writer.write_all(&serde_json::to_vec(&request)?).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    let mut line = String::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        BufReader::new(reader).read_line(&mut line),
    )
    .await
    .context("previous daemon did not acknowledge shutdown")??;
    let response: Value = serde_json::from_str(line.trim())?;
    if !response.get("error").is_none_or(Value::is_null)
        || response.get("result").is_none_or(Value::is_null)
    {
        bail!("previous daemon rejected the replacement shutdown request");
    }
    Ok(())
}

#[cfg(unix)]
async fn platform_shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};

    let mut terminate = signal(SignalKind::terminate()).expect("failed to register SIGTERM");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = terminate.recv() => {},
    }
}

#[cfg(windows)]
async fn platform_shutdown_signal() {
    use tokio::signal::windows;

    let mut close = windows::ctrl_close().expect("failed to register CTRL_CLOSE_EVENT");
    let mut logoff = windows::ctrl_logoff().expect("failed to register CTRL_LOGOFF_EVENT");
    let mut shutdown = windows::ctrl_shutdown().expect("failed to register CTRL_SHUTDOWN_EVENT");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = close.recv() => {},
        _ = logoff.recv() => {},
        _ = shutdown.recv() => {},
        _ = shutdown_windows::session_end_signal() => {},
    }
}

impl Config {
    fn from_env() -> Result<Self> {
        let data_dir = env::var("COWORK_DAEMON_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| default_data_dir());
        create_private_dir(&data_dir)?;
        let ipc_token =
            secret_or_create("COWORK_DAEMON_IPC_TOKEN", &data_dir.join("ipc-token.txt"))?;
        if ipc_token.len() < 32 {
            bail!("COWORK_DAEMON_IPC_TOKEN must contain at least 32 characters");
        }
        Ok(Self {
            database_path: data_dir.join("daemon.sqlite3"),
            ipc_endpoint: env::var("COWORK_DAEMON_IPC_ENDPOINT")
                .unwrap_or_else(|_| default_ipc_endpoint(&data_dir)),
            ipc_token,
            user_id: persistent_uuid("COWORK_DAEMON_USER_ID", &data_dir.join("user-id.txt"))?,
            device_id: persistent_uuid("COWORK_DAEMON_DEVICE_ID", &data_dir.join("device-id.txt"))?,
            model_secret_key: model_secret_key(&data_dir)?,
            fallback_model: PersistedModelConfig {
                base_url: env::var("COWORK_MODEL_BASE_URL")
                    .unwrap_or_else(|_| "http://127.0.0.1:11434/v1".to_owned()),
                api_key: env::var("COWORK_MODEL_API_KEY")
                    .ok()
                    .filter(|value| !value.trim().is_empty()),
                model: env::var("COWORK_MODEL_NAME").unwrap_or_else(|_| "qwen3:8b".to_owned()),
                timeout_ms: 20 * 60 * 1_000,
                max_steps: 64,
                verify_tls_certificates: true,
                mcp_servers: Vec::new(),
                crew_request: None,
                codex_request: None,
            },
            runtime_paths: load_runtime_paths(&data_dir)?,
        })
    }
}

fn load_runtime_paths(data_dir: &Path) -> Result<RuntimePaths> {
    let path = data_dir.join("runtime-paths.json");
    let mut paths = if path.is_file() {
        serde_json::from_slice::<RuntimePaths>(&fs::read(&path)?)
            .with_context(|| format!("invalid runtime paths file {}", path.display()))?
    } else {
        RuntimePaths::default()
    };
    if let Some(value) = env::var_os("COWORK_CREW_PYTHON") {
        paths.crew_python = PathBuf::from(value);
    }
    if let Some(value) = env::var_os("COWORK_CREW_SCRIPT") {
        paths.crew_script = PathBuf::from(value);
    }
    if let Some(value) = env::var_os("COWORK_CODEX_ROOT") {
        paths.codex_root = PathBuf::from(value);
    }
    if let Some(value) = env::var_os("COWORK_CODEX_PROFILES") {
        paths.codex_profiles = PathBuf::from(value);
    }
    Ok(paths)
}

fn initialize_database(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        PRAGMA foreign_keys = ON;
        PRAGMA synchronous = NORMAL;

        CREATE TABLE IF NOT EXISTS daemon_runs (
            id TEXT PRIMARY KEY,
            thread_id TEXT NOT NULL,
            state TEXT NOT NULL,
            revision INTEGER NOT NULL,
            record_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            idempotency_key TEXT NOT NULL UNIQUE
        );
        CREATE INDEX IF NOT EXISTS daemon_runs_queue
            ON daemon_runs(state, created_at);
        CREATE INDEX IF NOT EXISTS daemon_runs_thread
            ON daemon_runs(thread_id, created_at);

        CREATE TABLE IF NOT EXISTS daemon_run_events (
            run_id TEXT NOT NULL REFERENCES daemon_runs(id) ON DELETE CASCADE,
            sequence INTEGER NOT NULL,
            event_json TEXT NOT NULL,
            PRIMARY KEY (run_id, sequence)
        );

        CREATE TABLE IF NOT EXISTS daemon_project_bindings (
            project_id TEXT PRIMARY KEY,
            workspace_path TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS daemon_provider_bindings (
            profile_id TEXT PRIMARY KEY,
            encrypted_binding TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS daemon_mcp_bindings (
            server_id TEXT PRIMARY KEY,
            encrypted_binding TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS daemon_run_model_configs (
            run_id TEXT PRIMARY KEY REFERENCES daemon_runs(id) ON DELETE CASCADE,
            encrypted_config TEXT NOT NULL,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS daemon_run_workspaces (
            run_id TEXT PRIMARY KEY REFERENCES daemon_runs(id) ON DELETE CASCADE,
            workspace_path TEXT NOT NULL,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS daemon_schedules (
            id TEXT PRIMARY KEY,
            expression TEXT NOT NULL,
            timezone TEXT NOT NULL,
            enabled INTEGER NOT NULL,
            encrypted_template TEXT NOT NULL,
            next_run_at TEXT,
            last_triggered_at TEXT,
            last_error TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS daemon_run_checkpoints (
            id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL REFERENCES daemon_runs(id) ON DELETE CASCADE,
            sequence INTEGER NOT NULL,
            safe_to_resume INTEGER NOT NULL,
            executor_state TEXT NOT NULL,
            created_at TEXT NOT NULL,
            UNIQUE (run_id, sequence)
        );
        CREATE INDEX IF NOT EXISTS daemon_checkpoints_run
            ON daemon_run_checkpoints(run_id, sequence DESC);

        CREATE TABLE IF NOT EXISTS daemon_approval_requests (
            id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL REFERENCES daemon_runs(id) ON DELETE CASCADE,
            state TEXT NOT NULL,
            request_json TEXT NOT NULL,
            response_json TEXT,
            expires_at TEXT NOT NULL,
            created_at TEXT NOT NULL,
            resolved_at TEXT
        );
        CREATE UNIQUE INDEX IF NOT EXISTS daemon_approval_pending
            ON daemon_approval_requests(run_id) WHERE state = 'pending';

        CREATE TABLE IF NOT EXISTS daemon_input_requests (
            id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL REFERENCES daemon_runs(id) ON DELETE CASCADE,
            state TEXT NOT NULL,
            request_json TEXT NOT NULL,
            response_json TEXT,
            expires_at TEXT NOT NULL,
            created_at TEXT NOT NULL,
            resolved_at TEXT
        );
        CREATE UNIQUE INDEX IF NOT EXISTS daemon_input_pending
            ON daemon_input_requests(run_id) WHERE state = 'pending';

        CREATE TABLE IF NOT EXISTS daemon_entities (
            entity_type TEXT NOT NULL,
            id TEXT NOT NULL,
            revision INTEGER NOT NULL,
            etag TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            tombstone INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (entity_type, id)
        );
        CREATE INDEX IF NOT EXISTS daemon_entities_live
            ON daemon_entities(entity_type, tombstone, updated_at DESC);

        CREATE TABLE IF NOT EXISTS daemon_sync_changes (
            cursor INTEGER PRIMARY KEY AUTOINCREMENT,
            entity_type TEXT NOT NULL,
            entity_id TEXT NOT NULL,
            revision INTEGER NOT NULL,
            operation TEXT NOT NULL,
            entity_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            UNIQUE (entity_type, entity_id, revision)
        );
        "#,
    )?;
    Ok(())
}

fn migrate_legacy_creator_user_id(connection: &Connection, user_id: Uuid) -> Result<usize> {
    let legacy_user_id: Uuid = LEGACY_LOCAL_USER_ID
        .parse()
        .expect("the legacy local user ID must remain a UUID");
    if user_id == legacy_user_id {
        return Ok(0);
    }
    let mut statement = connection.prepare("SELECT record_json FROM daemon_runs")?;
    let encoded_records = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(statement);
    let mut migrated = 0;
    for encoded in encoded_records {
        let mut record: RunRecord = serde_json::from_str(&encoded)?;
        if record.spec.creator_user_id != legacy_user_id {
            continue;
        }
        record.spec.creator_user_id = user_id;
        save_record(connection, &record)?;
        migrated += 1;
    }
    Ok(migrated)
}

fn interrupt_active_runs(connection: &Connection, safe_reason: &str) -> Result<()> {
    let mut statement = connection.prepare(
        "SELECT record_json FROM daemon_runs WHERE state IN ('running', 'waiting_approval', 'waiting_input')",
    )?;
    let records = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(statement);
    for encoded in records {
        let mut record: RunRecord = serde_json::from_str(&encoded)?;
        let safe_to_resume = connection
            .query_row(
                "SELECT safe_to_resume FROM daemon_run_checkpoints WHERE run_id = ?1 ORDER BY sequence DESC LIMIT 1",
                [record.spec.id.to_string()],
                |row| row.get::<_, bool>(0),
            )
            .optional()?
            .unwrap_or(true);
        let from = record.state;
        record.state = RunState::Interrupted;
        record.revision += 1;
        refresh_etag(&mut record);
        record.updated_at = Utc::now();
        record.lease_expires_at = None;
        record.error = Some(RunError {
            code: if safe_to_resume {
                safe_reason
            } else {
                "unsafe_tool_interrupted"
            }
            .to_owned(),
            message: if safe_to_resume && safe_reason == "daemon_shutdown" {
                "The local daemon stopped at a safe checkpoint."
            } else if safe_to_resume {
                "The local daemon restarted after an unclean exit."
            } else {
                "The local daemon restarted during an unsafe action. The action was not repeated."
            }
            .to_owned(),
            retryable: false,
            details: json!({
                "safe_to_resume": safe_to_resume,
                "manual_review_required": !safe_to_resume,
            }),
        });
        save_record(connection, &record)?;
        append_event(
            connection,
            record.spec.id,
            RunEventKind::StateChanged,
            json!({
                "from": state_name(from),
                "to": "interrupted",
                "reason": if safe_to_resume { safe_reason } else { "unsafe_tool_interrupted" },
                "safe_to_resume": safe_to_resume,
                "manual_review_required": !safe_to_resume,
            }),
        )?;
    }
    Ok(())
}

async fn scheduler_loop(daemon: Daemon) {
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    loop {
        interval.tick().await;
        let due = match claim_due_schedules(&daemon, Utc::now()).await {
            Ok(due) => due,
            Err(error) => {
                tracing::error!(?error, "failed to claim local schedules");
                continue;
            }
        };
        for schedule in due {
            if let Err(error) = trigger_schedule(&daemon, &schedule).await {
                tracing::error!(schedule_id = %schedule.id, ?error, "failed to trigger local schedule");
                let database = daemon.database.lock().await;
                let _ = database.execute(
                    "UPDATE daemon_schedules SET last_error = ?2, updated_at = ?3 WHERE id = ?1",
                    params![
                        schedule.id.to_string(),
                        error.to_string(),
                        Utc::now().to_rfc3339(),
                    ],
                );
            }
        }
    }
}

async fn claim_due_schedules(daemon: &Daemon, now: DateTime<Utc>) -> Result<Vec<DueSchedule>> {
    let database = daemon.database.lock().await;
    let mut statement = database.prepare(
        "SELECT id, expression, timezone, next_run_at, encrypted_template FROM daemon_schedules WHERE enabled = 1 AND next_run_at IS NOT NULL AND next_run_at <= ?1 ORDER BY next_run_at, id LIMIT 100",
    )?;
    let rows = statement
        .query_map([now.to_rfc3339()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(statement);
    let mut due = Vec::with_capacity(rows.len());
    for (id, expression, timezone, due_at, encrypted_template) in rows {
        let id: Uuid = id.parse().context("invalid persisted schedule ID")?;
        let due_at = DateTime::parse_from_rfc3339(&due_at)?.with_timezone(&Utc);
        let next = next_schedule_at(&expression, &timezone, now)?;
        database.execute(
            "UPDATE daemon_schedules SET next_run_at = ?2, last_triggered_at = ?3, last_error = NULL, updated_at = ?3 WHERE id = ?1 AND next_run_at <= ?3",
            params![id.to_string(), next.to_rfc3339(), now.to_rfc3339()],
        )?;
        due.push(DueSchedule {
            id,
            expression,
            timezone,
            due_at,
            encrypted_template,
        });
    }
    Ok(due)
}

async fn trigger_schedule(daemon: &Daemon, schedule: &DueSchedule) -> Result<RunRecord> {
    let mut template = decrypt_schedule_template(
        &schedule.encrypted_template,
        &daemon.config.model_secret_key,
    )?;
    {
        let database = daemon.database.lock().await;
        refresh_schedule_template_from_entities(
            &database,
            &mut template,
            &daemon.config.model_secret_key,
        )?;
    }
    let now = Utc::now();
    let (missed_intervals, catch_up_truncated) = count_schedule_occurrences(
        &schedule.expression,
        &schedule.timezone,
        schedule.due_at,
        now,
    )?;
    let assistant_id = Uuid::new_v4().to_string();
    let user_message_id = Uuid::new_v4().to_string();
    let input = template
        .request
        .input
        .as_object_mut()
        .context("scheduled run input must be an object")?;
    input.insert(
        "client_assistant_message_id".to_owned(),
        json!(assistant_id),
    );
    input.insert("client_user_message_id".to_owned(), json!(user_message_id));
    input.insert("scheduled".to_owned(), json!(true));
    input.insert("schedule_id".to_owned(), json!(schedule.id));
    input.insert("schedule_due_at".to_owned(), json!(schedule.due_at));
    input.insert("missed_intervals".to_owned(), json!(missed_intervals));
    input.insert("catch_up_truncated".to_owned(), json!(catch_up_truncated));
    if input
        .get("source")
        .and_then(Value::as_str)
        .is_some_and(|source| source.starts_with("crew_"))
    {
        let monitor_id = Uuid::new_v4().to_string();
        let stream_id = format!("crew-{}", Uuid::new_v4());
        input.insert("client_crew_live_message_id".to_owned(), json!(monitor_id));
        input.insert("crew_stream_id".to_owned(), json!(stream_id.clone()));
        if let Some(request) = template
            .model_config
            .crew_request
            .as_mut()
            .and_then(Value::as_object_mut)
        {
            request.insert("streamId".to_owned(), json!(stream_id));
        }
    }
    template.request.idempotency_key = format!(
        "local-schedule:{}:{}",
        schedule.id,
        schedule.due_at.timestamp_millis()
    );
    let mut params = serde_json::to_value(&template.request)?;
    params
        .as_object_mut()
        .context("serialized scheduled run request must be an object")?
        .insert(
            "model_config".to_owned(),
            serde_json::to_value(&template.model_config)?,
        );
    let run: RunRecord = serde_json::from_value(create_run(daemon, params).await?)?;
    Ok(run)
}

fn live_daemon_entity(
    connection: &Connection,
    entity_type: &str,
    id: &str,
) -> Result<Option<(i64, Value)>> {
    let row = connection
        .query_row(
            "SELECT revision, payload_json, tombstone FROM daemon_entities WHERE entity_type = ?1 AND id = ?2",
            params![entity_type, id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                ))
            },
        )
        .optional()?;
    match row {
        Some((_, _, true)) | None => Ok(None),
        Some((revision, payload, false)) => Ok(Some((revision, serde_json::from_str(&payload)?))),
    }
}

fn schedule_entity(
    connection: &Connection,
    entity_type: &str,
    id: &str,
    required: bool,
) -> Result<Option<(i64, Value)>> {
    let entity = live_daemon_entity(connection, entity_type, id)?;
    if required && entity.is_none() {
        bail!("schedule is waiting for current {entity_type} metadata ({id})");
    }
    Ok(entity)
}

fn refresh_schedule_template_from_entities(
    connection: &Connection,
    template: &mut PersistedScheduleTemplate,
    secret_key: &[u8; 32],
) -> Result<()> {
    let input = template
        .request
        .input
        .as_object()
        .context("scheduled run input must be an object")?;
    let require_current = input
        .get("resolve_current_versions")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let require_current_binding = input
        .get("resolve_current_provider_binding")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let require_current_mcp_bindings = input
        .get("resolve_current_mcp_bindings")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let require_current_crew_provider_bindings = input
        .get("resolve_current_crew_provider_bindings")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mcp_server_ids = input
        .get("client_mcp_server_ids")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(ToOwned::to_owned)
                        .context("client_mcp_server_ids must contain only strings")
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    let client_project_id = input
        .get("client_project_id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let client_task_id = input
        .get("client_task_id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let provider_profile_id = input
        .get("client_provider_profile_id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let crew_id = input
        .get("crew_id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let mut resolved = serde_json::Map::new();

    if let Some(id) = client_project_id {
        if let Some((revision, payload)) =
            schedule_entity(connection, "project", &id, require_current)?
        {
            template.request.project_revision = revision;
            resolved.insert("project".to_owned(), json!(revision));
            if let Some(instructions) = payload
                .get("instructions")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                template
                    .request
                    .input
                    .as_object_mut()
                    .expect("input was validated as an object")
                    .insert(
                        "current_project_instructions".to_owned(),
                        json!(instructions),
                    );
            }
        }
    }

    if let Some(id) = client_task_id {
        if let Some((revision, payload)) =
            schedule_entity(connection, "task", &id, require_current)?
        {
            if let Some(task) = template.request.task.as_mut() {
                task.revision = revision;
            }
            resolved.insert("task".to_owned(), json!(revision));
            let input = template
                .request
                .input
                .as_object_mut()
                .expect("input was validated as an object");
            if let Some(prompt) = payload
                .get("description")
                .or_else(|| payload.get("prompt"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                input.insert("prompt".to_owned(), json!(prompt));
            }
            if let Some(expected) = payload
                .get("expected_output")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                input.insert("expected_output".to_owned(), json!(expected));
            }
            if let Some(model) = payload
                .get("model")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                template.model_config.model = model.to_owned();
            }
        }
    }

    if let Some(id) = provider_profile_id {
        if let Some((revision, payload)) =
            schedule_entity(connection, "provider_profile", &id, require_current)?
        {
            resolved.insert("provider_profile".to_owned(), json!(revision));
            if let Some(model) = payload
                .get("model")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                template.model_config.model = model.to_owned();
            }
            if let Some(timeout) = payload.get("timeout_ms").and_then(Value::as_u64) {
                template.model_config.timeout_ms = timeout;
            }
            if let Some(verify) = payload
                .get("verify_tls_certificates")
                .and_then(Value::as_bool)
            {
                template.model_config.verify_tls_certificates = verify;
            }
        }
        match load_provider_binding(connection, &id, secret_key)? {
            Some(binding) => {
                template.model_config.base_url = binding.base_url;
                template.model_config.api_key = binding.api_key;
                template
                    .request
                    .input
                    .as_object_mut()
                    .expect("input was validated as an object")
                    .insert("resolved_device_provider_binding".to_owned(), json!(true));
            }
            None if require_current_binding => {
                bail!("schedule is waiting for the per-device provider binding ({id})")
            }
            None => {}
        }
    }

    if require_current_mcp_bindings {
        let mut bindings = Vec::with_capacity(mcp_server_ids.len());
        for id in &mcp_server_ids {
            validate_mcp_server_id(id)?;
            let Some(binding) = load_mcp_binding(connection, id, secret_key)? else {
                bail!("schedule is waiting for the per-device MCP binding ({id})");
            };
            bindings.push(binding);
        }
        template.model_config.mcp_servers = bindings;
        let input = template
            .request
            .input
            .as_object_mut()
            .expect("input was validated as an object");
        input.insert("resolved_device_mcp_bindings".to_owned(), json!(true));
        input.insert("resolved_mcp_server_ids".to_owned(), json!(mcp_server_ids));
    }

    if let Some(id) = crew_id {
        if let Some((revision, payload)) =
            schedule_entity(connection, "crew", &id, require_current)?
        {
            resolved.insert("crew".to_owned(), json!(revision));
            if let (Some(definition), Some(request)) = (
                payload.get("definition").and_then(Value::as_object),
                template
                    .model_config
                    .crew_request
                    .as_mut()
                    .and_then(Value::as_object_mut),
            ) {
                for key in [
                    "name",
                    "description",
                    "executionSubject",
                    "executionGuidelines",
                    "knowledgeFocus",
                    "governanceMode",
                    "outputMode",
                    "stopOnFailure",
                    "retryCount",
                    "managerReviewEnabled",
                    "managerReviewGuidelines",
                    "shareAllTaskOutputs",
                    "sharedOutputCharLimit",
                    "agents",
                    "tasks",
                    "process",
                    "managerAgentId",
                    "verbose",
                    "maxRpm",
                    "maxParallelTasks",
                ] {
                    if let Some(value) = definition.get(key) {
                        request.insert(key.to_owned(), value.clone());
                    }
                }
            }
        }
        if require_current_crew_provider_bindings {
            let mut resolved_profiles = Vec::new();
            for property in ["openAICompatible", "openRouter"] {
                let profile_id = template
                    .model_config
                    .crew_request
                    .as_ref()
                    .and_then(|request| request.get("providerConfigs"))
                    .and_then(|providers| providers.get(property))
                    .and_then(|config| config.get("profileId"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned);
                let Some(profile_id) = profile_id else {
                    continue;
                };
                validate_provider_profile_id(&profile_id)?;
                let (revision, metadata) =
                    schedule_entity(connection, "provider_profile", &profile_id, true)?
                        .expect("required provider metadata was checked");
                let binding = load_provider_binding(connection, &profile_id, secret_key)?
                    .with_context(|| {
                        format!(
                            "schedule is waiting for the per-device Crew provider binding ({profile_id})"
                        )
                    })?;
                let config = template
                    .model_config
                    .crew_request
                    .as_mut()
                    .and_then(|request| request.get_mut("providerConfigs"))
                    .and_then(|providers| providers.get_mut(property))
                    .and_then(Value::as_object_mut)
                    .context("Crew provider configuration must be an object")?;
                config.insert("baseUrl".to_owned(), json!(binding.base_url));
                config.insert(
                    "apiKey".to_owned(),
                    json!(binding.api_key.unwrap_or_default()),
                );
                if let Some(model) = metadata
                    .get("model")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    config.insert("model".to_owned(), json!(model));
                }
                if let Some(timeout) = metadata.get("timeout_ms").and_then(Value::as_u64) {
                    config.insert("timeoutMs".to_owned(), json!(timeout));
                }
                if let Some(verify) = metadata
                    .get("verify_tls_certificates")
                    .and_then(Value::as_bool)
                {
                    config.insert("verifyTlsCertificates".to_owned(), json!(verify));
                }
                resolved.insert(
                    format!("crew_provider_profile:{profile_id}"),
                    json!(revision),
                );
                resolved_profiles.push(profile_id);
            }
            let input = template
                .request
                .input
                .as_object_mut()
                .expect("input was validated as an object");
            input.insert(
                "resolved_device_crew_provider_bindings".to_owned(),
                json!(true),
            );
            input.insert(
                "resolved_crew_provider_profile_ids".to_owned(),
                json!(resolved_profiles),
            );
        }
    }

    template
        .request
        .input
        .as_object_mut()
        .expect("input was validated as an object")
        .insert(
            "resolved_entity_revisions".to_owned(),
            Value::Object(resolved),
        );
    template.model_config.validate()?;
    Ok(())
}

async fn worker_loop(daemon: Daemon) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        interval.tick().await;
        let claimed = match claim_run(&daemon).await {
            Ok(value) => value,
            Err(error) => {
                tracing::error!(?error, "failed to claim local run");
                continue;
            }
        };
        let Some(run) = claimed else { continue };
        if let Err(error) = execute_run(&daemon, run).await {
            tracing::error!(?error, "failed to persist local run result");
        }
    }
}

async fn claim_run(daemon: &Daemon) -> Result<Option<RunRecord>> {
    let database = daemon.database.lock().await;
    let encoded = database
        .query_row(
            r#"
            SELECT candidate.record_json
            FROM daemon_runs candidate
            WHERE candidate.state = 'queued'
              AND NOT EXISTS (
                SELECT 1 FROM daemon_runs prior
                WHERE prior.thread_id = candidate.thread_id
                  AND prior.created_at < candidate.created_at
                  AND prior.state NOT IN ('completed', 'failed', 'canceled', 'expired')
              )
            ORDER BY candidate.created_at ASC
            LIMIT 1
            "#,
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let Some(encoded) = encoded else {
        return Ok(None);
    };
    let mut record: RunRecord = serde_json::from_str(&encoded)?;
    record.state = RunState::Running;
    record.revision += 1;
    refresh_etag(&mut record);
    record.started_at = Some(Utc::now());
    record.updated_at = Utc::now();
    save_record(&database, &record)?;
    append_event(
        &database,
        record.spec.id,
        RunEventKind::StateChanged,
        json!({"from": "queued", "to": "running"}),
    )?;
    Ok(Some(record))
}

async fn execute_run(daemon: &Daemon, mut run: RunRecord) -> Result<()> {
    let workspace = match run_workspace(daemon, run.spec.id).await? {
        Some(workspace) => Some(workspace),
        None => project_workspace(daemon, run.spec.project_id).await?,
    };
    let model_config = load_run_model_config(daemon, run.spec.id)
        .await?
        .unwrap_or_else(|| daemon.config.fallback_model.clone());
    let host = LocalRuntimeHost {
        daemon,
        run_id: run.spec.id,
        workspace: workspace.clone(),
        tool_policy: run
            .spec
            .input
            .get("tool_policy")
            .and_then(Value::as_str)
            .unwrap_or("autonomous")
            .to_owned(),
        mcp_servers: model_config.mcp_servers.clone(),
    };
    let outcome: Result<Value> = if let Some(request) = model_config.crew_request.clone() {
        execute_crew_adapter(
            daemon,
            run.spec.id,
            request,
            Duration::from_millis(model_config.timeout_ms),
        )
        .await
    } else if let Some(request) = model_config.codex_request.clone() {
        codex::execute_codex_adapter(
            daemon,
            &host,
            run.spec.id,
            request,
            workspace.clone(),
            Duration::from_millis(model_config.timeout_ms),
        )
        .await
    } else {
        let agent = AgentRuntime::new(model_config.runtime_config())?;
        agent
            .execute(&run.spec, &host)
            .await
            .and_then(|result| serde_json::to_value(result).map_err(Into::into))
    };
    let _ = developer_browser::local_browser_stop(&daemon.browser).await;
    let database = daemon.database.lock().await;
    let persisted: String = database.query_row(
        "SELECT record_json FROM daemon_runs WHERE id = ?1",
        [run.spec.id.to_string()],
        |row| row.get(0),
    )?;
    run = serde_json::from_str(&persisted)?;
    if run.state == RunState::Canceled {
        return Ok(());
    }
    run.revision += 1;
    refresh_etag(&mut run);
    run.updated_at = Utc::now();
    run.finished_at = Some(Utc::now());
    match outcome {
        Ok(result) => {
            run.state = RunState::Completed;
            run.result = Some(result.clone());
            append_event(&database, run.spec.id, RunEventKind::Completed, result)?;
        }
        Err(error) => {
            let safe_to_resume = database
                .query_row(
                    "SELECT safe_to_resume FROM daemon_run_checkpoints WHERE run_id = ?1 ORDER BY sequence DESC LIMIT 1",
                    [run.spec.id.to_string()],
                    |row| row.get::<_, bool>(0),
                )
                .optional()?
                .unwrap_or(true);
            run.state = if safe_to_resume {
                RunState::Failed
            } else {
                RunState::Interrupted
            };
            run.error = Some(RunError {
                code: if safe_to_resume {
                    "local_agent_failed"
                } else {
                    "unsafe_tool_interrupted"
                }
                .to_owned(),
                message: error.to_string(),
                retryable: false,
                details: Value::Null,
            });
            append_event(
                &database,
                run.spec.id,
                if safe_to_resume {
                    RunEventKind::Failed
                } else {
                    RunEventKind::Warning
                },
                json!({"message": error.to_string(), "safe_to_resume": safe_to_resume}),
            )?;
        }
    }
    save_record(&database, &run)?;
    if run.state.is_terminal() {
        database.execute(
            "DELETE FROM daemon_run_model_configs WHERE run_id = ?1",
            [run.spec.id.to_string()],
        )?;
        database.execute(
            "DELETE FROM daemon_run_workspaces WHERE run_id = ?1",
            [run.spec.id.to_string()],
        )?;
    }
    Ok(())
}

async fn execute_crew_adapter(
    daemon: &Daemon,
    run_id: Uuid,
    mut request: Value,
    timeout: Duration,
) -> Result<Value> {
    let python = &daemon.config.runtime_paths.crew_python;
    let script = &daemon.config.runtime_paths.crew_script;
    if !python.is_file() || !script.is_file() {
        bail!(
            "the bundled CrewAI runtime is not prepared (python: {}, script: {}); initialize it once from the desktop settings",
            python.display(),
            script.display()
        );
    }
    append_event_async(
        daemon,
        run_id,
        RunEventKind::ModelStarted,
        json!({"adapter":"crewai","runtime":"python"}),
    )
    .await?;
    if let Some(request) = request.as_object_mut() {
        request.insert("runId".to_owned(), Value::String(run_id.to_string()));
    }
    let mut command = tokio::process::Command::new(python);
    command
        .arg(script)
        .arg("execute")
        .env("LITELLM_LOCAL_MODEL_COST_MAP", "True")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to start Crew runtime {}", python.display()))?;
    let process_tree = ManagedProcessTree::attach(&child)?;
    let mut stdin = child
        .stdin
        .take()
        .context("Crew runtime stdin is missing")?;
    stdin.write_all(&serde_json::to_vec(&request)?).await?;
    stdin.shutdown().await?;
    drop(stdin);
    let stdout = child
        .stdout
        .take()
        .context("Crew runtime stdout is missing")?;
    let mut lines = BufReader::new(stdout).lines();
    let mut stderr = child
        .stderr
        .take()
        .context("Crew runtime stderr is missing")?;
    let stderr_task = tokio::spawn(async move {
        let mut value = String::new();
        let _ = stderr.read_to_string(&mut value).await;
        value
    });
    let mut output_lines = Vec::new();
    let mut output_bytes = 0_usize;
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line? else { break; };
                if let Ok(event) = serde_json::from_str::<Value>(&line) {
                    if event.get("localAiCoworkEvent").is_some() {
                        if serde_json::to_vec(&event)?.len() <= 1024 * 1024 {
                            append_event_async(
                                daemon,
                                run_id,
                                RunEventKind::ModelDelta,
                                json!({"adapter":"crewai","crew_event":event}),
                            ).await?;
                        }
                        continue;
                    }
                }
                output_bytes = output_bytes.saturating_add(line.len());
                if output_bytes > 32 * 1024 * 1024 {
                    process_tree.terminate();
                    let _ = child.kill().await;
                    bail!("Crew runtime output exceeds 32 MiB");
                }
                output_lines.push(line);
            }
            _ = tokio::time::sleep(Duration::from_millis(250)) => {
                if local_run_is_canceled(daemon, run_id).await? {
                    process_tree.terminate();
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    bail!("Crew run was canceled");
                }
            }
            _ = &mut deadline => {
                process_tree.terminate();
                let _ = child.kill().await;
                let _ = child.wait().await;
                bail!("Crew runtime exceeded its configured timeout of {} seconds", timeout.as_secs());
            }
        }
    }
    let status = child.wait().await?;
    let stderr = stderr_task.await.unwrap_or_default();
    if !status.success() {
        bail!(
            "Crew runtime failed with {status}: {}",
            truncate_chars(stderr.trim(), 8_000)
        );
    }
    let response: Value = serde_json::from_str(output_lines.join("\n").trim())
        .context("Crew runtime returned invalid JSON")?;
    let content = response
        .get("taskResults")
        .or_else(|| response.get("task_results"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("output").and_then(Value::as_str))
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let status_name = response
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("failed");
    if status_name != "completed" {
        bail!(
            "Crew runtime ended in state {status_name}: {}",
            response
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown CrewAI error")
        );
    }
    append_event_async(
        daemon,
        run_id,
        RunEventKind::ModelCompleted,
        json!({"adapter":"crewai","content":content,"response":response}),
    )
    .await?;
    Ok(json!({"content":content,"crew_response":response}))
}

pub(crate) struct ManagedProcessTree {
    #[cfg(windows)]
    job: isize,
    #[cfg(unix)]
    process_group: i32,
}

impl ManagedProcessTree {
    pub(crate) fn attach(child: &tokio::process::Child) -> Result<Self> {
        let process_id = child.id().context("Crew runtime has no process ID")?;
        #[cfg(windows)]
        {
            use std::{ffi::c_void, mem::size_of, ptr};
            use windows_sys::Win32::{
                Foundation::{CloseHandle, HANDLE},
                System::{
                    JobObjects::{
                        AssignProcessToJobObject, CreateJobObjectW,
                        JobObjectExtendedLimitInformation, SetInformationJobObject,
                        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                    },
                    Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE},
                },
            };
            unsafe {
                let job = CreateJobObjectW(ptr::null(), ptr::null());
                if job.is_null() {
                    bail!("failed to create the Crew process job");
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
                    bail!("failed to configure the Crew process job");
                }
                let process: HANDLE =
                    OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, process_id);
                if process.is_null() {
                    CloseHandle(job);
                    bail!("failed to open the Crew process for job assignment");
                }
                let assigned = AssignProcessToJobObject(job, process);
                CloseHandle(process);
                if assigned == 0 {
                    CloseHandle(job);
                    bail!("failed to assign the Crew process to its lifecycle job");
                }
                Ok(Self { job: job as isize })
            }
        }
        #[cfg(unix)]
        {
            Ok(Self {
                process_group: process_id as i32,
            })
        }
    }

    pub(crate) fn terminate(&self) {
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::System::JobObjects::TerminateJobObject;
            let _ = TerminateJobObject(self.job as _, 1);
        }
        #[cfg(unix)]
        unsafe {
            let _ = libc::kill(-self.process_group, libc::SIGKILL);
        }
    }
}

#[cfg(windows)]
impl Drop for ManagedProcessTree {
    fn drop(&mut self) {
        unsafe {
            let _ = windows_sys::Win32::Foundation::CloseHandle(self.job as _);
        }
    }
}

#[cfg(unix)]
impl Drop for ManagedProcessTree {
    fn drop(&mut self) {
        self.terminate();
    }
}

async fn append_event_async(
    daemon: &Daemon,
    run_id: Uuid,
    kind: RunEventKind,
    payload: Value,
) -> Result<()> {
    let database = daemon.database.lock().await;
    append_event(&database, run_id, kind, payload)
}

async fn local_run_is_canceled(daemon: &Daemon, run_id: Uuid) -> Result<bool> {
    let database = daemon.database.lock().await;
    let state: String = database.query_row(
        "SELECT state FROM daemon_runs WHERE id = ?1",
        [run_id.to_string()],
        |row| row.get(0),
    )?;
    Ok(state == "canceled")
}

struct LocalRuntimeHost<'a> {
    daemon: &'a Daemon,
    run_id: Uuid,
    workspace: Option<PathBuf>,
    tool_policy: String,
    mcp_servers: Vec<PersistedMcpServer>,
}

#[async_trait]
impl RuntimeHost for LocalRuntimeHost<'_> {
    fn tools(&self) -> Vec<ToolDefinition> {
        let mut tools = vec![
            local_tool(
                "Think",
                "Record a short plan without changing the project.",
                json!({"type":"object","properties":{"thought":{"type":"string"}},"required":["thought"],"additionalProperties":false}),
                None,
                false,
            ),
            local_tool(
                "AskUser",
                "Pause this durable run and request structured user input.",
                json!({"type":"object","properties":{"question":{"type":"string"}},"required":["question"],"additionalProperties":true}),
                None,
                false,
            ),
            local_tool(
                "TaskCreate",
                "Create a durable local task that remains available after the UI closes.",
                json!({"type":"object","properties":{"title":{"type":"string","minLength":1,"maxLength":500},"description":{"type":"string","maxLength":200000}},"required":["title","description"],"additionalProperties":false}),
                Some("task.manage"),
                true,
            ),
            local_tool(
                "TaskList",
                "List durable local tasks, optionally filtered by status.",
                json!({"type":"object","properties":{"status":{"type":"string","enum":["pending","running","completed","failed","canceled"]}},"additionalProperties":false}),
                Some("task.manage"),
                false,
            ),
            local_tool(
                "TaskUpdate",
                "Update a durable local task status and optional note.",
                json!({"type":"object","properties":{"task_id":{"type":"string"},"status":{"type":"string","enum":["pending","running","completed","failed","canceled"]},"note":{"type":"string","maxLength":200000}},"required":["task_id"],"additionalProperties":false}),
                Some("task.manage"),
                true,
            ),
            local_tool(
                "MemoryRead",
                "Read durable local memory by scope and optional key fragment.",
                json!({"type":"object","properties":{"scope":{"type":"string","enum":["agent","user","chat","shared"]},"key":{"type":"string"}},"additionalProperties":false}),
                Some("memory.read"),
                false,
            ),
            local_tool(
                "MemoryWrite",
                "Add, uniquely replace, or remove durable local memory.",
                json!({"type":"object","properties":{"action":{"type":"string","enum":["add","replace","remove"]},"target":{"type":"string","enum":["memory","user"]},"old_text":{"type":"string"},"content":{"type":"string","maxLength":200000},"scope":{"type":"string","enum":["agent","user","chat","shared"]},"key":{"type":"string","maxLength":500}},"required":["action","target"],"additionalProperties":false}),
                Some("memory.write"),
                true,
            ),
            local_tool(
                "ChatSearch",
                "Search messages and results from durable local runs.",
                json!({"type":"object","properties":{"query":{"type":"string","minLength":1},"limit":{"type":"integer","minimum":1,"maximum":50}},"required":["query"],"additionalProperties":false}),
                Some("memory.read"),
                false,
            ),
            local_tool(
                "WebFetch",
                "Fetch bounded text from a public HTTP(S) origin; local, private, metadata and unsafe-port targets are blocked.",
                json!({"type":"object","properties":{"url":{"type":"string"},"max_chars":{"type":"integer","minimum":1,"maximum":200000}},"required":["url"],"additionalProperties":false}),
                Some("web.fetch"),
                false,
            ),
            local_tool(
                "WebSearch",
                "Search the public web and return titles, URLs and snippets.",
                json!({"type":"object","properties":{"query":{"type":"string","minLength":1},"max_results":{"type":"integer","minimum":1,"maximum":10}},"required":["query"],"additionalProperties":false}),
                Some("web.search"),
                false,
            ),
            local_tool(
                "Skill",
                "Load and render a centrally stored reusable skill for this run.",
                json!({"type":"object","properties":{"skill_name":{"type":"string"},"input":{"type":"string"}},"required":["skill_name","input"],"additionalProperties":false}),
                Some("skill.read"),
                false,
            ),
            local_tool(
                "SaveSkill",
                "Create or update a reusable skill in the daemon store without writing workspace files.",
                json!({"type":"object","properties":{"name":{"type":"string","minLength":1,"maxLength":500},"description":{"type":"string","maxLength":5000},"prompt_template":{"type":"string","minLength":1,"maxLength":200000},"trigger_pattern":{"type":"string","maxLength":5000},"run_mode":{"type":"string","enum":["execute","plan","hybrid"]}},"required":["name","description","prompt_template"],"additionalProperties":false}),
                Some("skill.write"),
                true,
            ),
        ];
        if self.workspace.is_some() {
            tools.extend([
                local_tool("Read", "Read a UTF-8 project file.", json!({"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer","minimum":1},"limit":{"type":"integer","minimum":1,"maximum":20000}},"required":["path"],"additionalProperties":false}), Some("files"), false),
                local_tool("Write", "Create or replace a project file.", json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false}), Some("files"), true),
                local_tool("Append", "Append UTF-8 text to a project file, creating it if necessary.", json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false}), Some("files"), true),
                local_tool("Edit", "Replace an exact string in a UTF-8 project file.", json!({"type":"object","properties":{"path":{"type":"string"},"old_string":{"type":"string"},"new_string":{"type":"string"},"replace_all":{"type":"boolean"}},"required":["path","old_string","new_string"],"additionalProperties":false}), Some("files"), true),
                local_tool("MultiEdit", "Apply multiple exact replacements to one UTF-8 project file in a single write.", json!({"type":"object","properties":{"path":{"type":"string"},"edits":{"type":"array","minItems":1,"maxItems":1000,"items":{"type":"object","properties":{"old_string":{"type":"string"},"new_string":{"type":"string"},"replace_all":{"type":"boolean"}},"required":["old_string","new_string"],"additionalProperties":false}}},"required":["path","edits"],"additionalProperties":false}), Some("files"), true),
                local_tool("CreateDirectory", "Create a project directory and missing parents.", json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}), Some("files"), true),
                local_tool("MovePath", "Move a project file or directory without overwriting an existing destination.", json!({"type":"object","properties":{"source":{"type":"string"},"destination":{"type":"string"}},"required":["source","destination"],"additionalProperties":false}), Some("files"), true),
                local_tool("CopyPath", "Copy a project file or directory without following symlinks or overwriting an existing destination.", json!({"type":"object","properties":{"source":{"type":"string"},"destination":{"type":"string"}},"required":["source","destination"],"additionalProperties":false}), Some("files"), true),
                local_tool("DeleteFile", "Delete one project file after writing a recoverable daemon-side backup.", json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}), Some("files"), true),
                local_tool("FileInfo", "Return project file or directory metadata without following symlinks.", json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}), Some("files"), false),
                local_tool("ListDir", "List directory entries and metadata.", json!({"type":"object","properties":{"path":{"type":"string"}},"additionalProperties":false}), Some("files"), false),
                local_tool("Glob", "Find project paths matching a glob.", json!({"type":"object","properties":{"pattern":{"type":"string"}},"required":["pattern"],"additionalProperties":false}), Some("files"), false),
                local_tool("Grep", "Search UTF-8 project files.", json!({"type":"object","properties":{"pattern":{"type":"string"},"case_sensitive":{"type":"boolean"}},"required":["pattern"],"additionalProperties":false}), Some("files"), false),
                local_tool("Bash", "Run a shell command with the project as working directory.", json!({"type":"object","properties":{"command":{"type":"string"},"timeout_seconds":{"type":"integer","minimum":1,"maximum":3600}},"required":["command"],"additionalProperties":false}), Some("shell"), true),
            ]);
            if developer_browser::local_browser_available() {
                tools.extend([
                    local_tool("BrowserNavigate", "Navigate a persistent Chromium profile to an HTTP(S) URL; set visible=true to use the interactive browser on this personal device.", json!({"type":"object","properties":{"url":{"type":"string"},"visible":{"type":"boolean"},"wait_until":{"type":"string","enum":["load","domcontentloaded","networkidle","commit"]},"timeout_ms":{"type":"integer","minimum":1,"maximum":120000}},"required":["url"],"additionalProperties":false}), Some("browser.headless"), false),
                    local_tool("BrowserClick", "Click the first element matching a CSS selector; optionally wait for a download into the project workspace.", json!({"type":"object","properties":{"selector":{"type":"string"},"expect_download":{"type":"boolean"},"download_path":{"type":"string"},"visible":{"type":"boolean"},"timeout_ms":{"type":"integer","minimum":1,"maximum":120000},"wait_ms":{"type":"integer","minimum":0,"maximum":30000}},"required":["selector"],"additionalProperties":false}), Some("browser.headless"), true),
                    local_tool("BrowserFill", "Wait for and fill an input matching a CSS selector, then dispatch input/change events.", json!({"type":"object","properties":{"selector":{"type":"string"},"value":{"type":"string"},"visible":{"type":"boolean"},"timeout_ms":{"type":"integer","minimum":1,"maximum":120000}},"required":["selector","value"],"additionalProperties":false}), Some("browser.headless"), true),
                    local_tool("BrowserUpload", "Wait for a browser file input and attach one or more project files.", json!({"type":"object","properties":{"selector":{"type":"string"},"path":{"type":"string"},"paths":{"type":"array","items":{"type":"string"},"maxItems":50},"visible":{"type":"boolean"},"timeout_ms":{"type":"integer","minimum":1,"maximum":120000}},"required":["selector"],"additionalProperties":false}), Some("browser.headless"), true),
                    local_tool("BrowserScreenshot", "Capture the current Chromium page as a PNG run artifact.", json!({"type":"object","properties":{"path":{"type":"string"},"full_page":{"type":"boolean"},"visible":{"type":"boolean"}},"additionalProperties":false}), Some("browser.headless"), false),
                    local_tool("BrowserTraceStart", "Start a bounded Chrome DevTools performance trace for the current persistent browser session.", json!({"type":"object","properties":{"visible":{"type":"boolean"}},"additionalProperties":false}), Some("browser.headless"), false),
                    local_tool("BrowserTraceStop", "Stop the active Chrome DevTools trace and store it as a versioned JSON run artifact.", json!({"type":"object","properties":{"path":{"type":"string"},"visible":{"type":"boolean"}},"additionalProperties":false}), Some("browser.headless"), false),
                    local_tool("BrowserInspect", "Return the current page title, URL, visible text, links, console entries and observed fetch/XHR network entries.", json!({"type":"object","properties":{"max_chars":{"type":"integer","minimum":1,"maximum":200000},"visible":{"type":"boolean"}},"additionalProperties":false}), Some("browser.headless"), false),
                    local_tool("BrowserTabs", "List the tabs and DevTools targets in the current Chromium session.", json!({"type":"object","properties":{"visible":{"type":"boolean"}},"additionalProperties":false}), Some("browser.headless"), false),
                ]);
            }
            tools.extend([
                local_tool("OfficeInspect", "Inspect DOCX, XLSX, PPTX or PDF structure and text without executing macros, add-ins or active content.", json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}), Some("office.ooxml"), false),
                local_tool("OfficeReplaceText", "Deterministically replace text in DOCX, XLSX or PPTX while preserving the OOXML package. Macro-enabled content is rejected.", json!({"type":"object","properties":{"path":{"type":"string"},"output_path":{"type":"string"},"old_text":{"type":"string"},"new_text":{"type":"string"},"replace_all":{"type":"boolean"}},"required":["path","old_text","new_text"],"additionalProperties":false}), Some("office.ooxml"), true),
                local_tool("OfficeExportPdf", "Export an Office document to PDF with installed Microsoft Office or LibreOffice in macro-disabled mode.", json!({"type":"object","properties":{"path":{"type":"string"},"output_path":{"type":"string"}},"required":["path"],"additionalProperties":false}), Some("office.native"), false),
                local_tool("OfficePreview", "Render an Office document or PDF to versioned PNG review artifacts using PDFium.", json!({"type":"object","properties":{"path":{"type":"string"},"all_pages":{"type":"boolean"},"dpi":{"type":"integer","minimum":72,"maximum":200}},"required":["path"],"additionalProperties":false}), Some("office.native"), false),
                local_tool("DesktopOpenOffice", "Open an Office document in the installed interactive desktop application for observation and local takeover.", json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}), Some("desktop.local"), true),
            ]);
            if desktop::available() {
                tools.extend([
                    local_tool("Desktopscreenshot", "Capture the primary interactive desktop as a PNG artifact for visual analysis.", json!({"type":"object","properties":{},"additionalProperties":false}), Some("desktop.screen.view"), false),
                    local_tool("DesktopPrimaryDisplay", "Return the bounds and scale of the primary interactive display.", json!({"type":"object","properties":{},"additionalProperties":false}), Some("desktop.screen.view"), false),
                    local_tool("DesktopListWindows", "List visible interactive desktop windows.", json!({"type":"object","properties":{},"additionalProperties":false}), Some("desktop.window.focus"), false),
                    local_tool("DesktopFocusWindow", "Focus a visible desktop window by title, process name or process ID.", json!({"type":"object","properties":{"title":{"type":"string"},"process_name":{"type":"string"},"process_id":{"type":"integer"}},"additionalProperties":false}), Some("desktop.window.focus"), true),
                    local_tool("DesktopLaunchApp", "Launch an application in the current interactive user session.", json!({"type":"object","properties":{"app_path":{"type":"string"},"args":{"type":"array","items":{"type":"string"},"maxItems":128},"cwd":{"type":"string"}},"required":["app_path"],"additionalProperties":false}), Some("desktop.input.control"), true),
                    local_tool("DesktopMoveMouse", "Move the interactive desktop pointer.", json!({"type":"object","properties":{"x":{"type":"integer"},"y":{"type":"integer"},"coordinate_space":{"type":"string","enum":["display","screen"]}},"required":["x","y"],"additionalProperties":false}), Some("desktop.input.control"), true),
                    local_tool("DesktopClick", "Click at interactive desktop coordinates.", json!({"type":"object","properties":{"x":{"type":"integer"},"y":{"type":"integer"},"button":{"type":"string","enum":["left","right"]},"double_click":{"type":"boolean"},"coordinate_space":{"type":"string","enum":["display","screen"]}},"required":["x","y"],"additionalProperties":false}), Some("desktop.input.control"), true),
                    local_tool("DesktopTypeText", "Type literal text into the focused interactive desktop application.", json!({"type":"object","properties":{"text":{"type":"string","maxLength":100000}},"required":["text"],"additionalProperties":false}), Some("desktop.input.control"), true),
                    local_tool("DesktopKeypress", "Send a validated keyboard chord to the focused interactive desktop application.", json!({"type":"object","properties":{"keys":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":16}},"required":["keys"],"additionalProperties":false}), Some("desktop.input.control"), true),
                    local_tool("DesktopScroll", "Scroll the interactive desktop at the current or requested pointer position.", json!({"type":"object","properties":{"scroll_y":{"type":"number"},"x":{"type":"integer"},"y":{"type":"integer"}},"required":["scroll_y"],"additionalProperties":false}), Some("desktop.input.control"), true),
                ]);
            }
        }
        if !self.mcp_servers.is_empty() {
            let names = self
                .mcp_servers
                .iter()
                .map(|server| server.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            tools.push(local_tool(
                "MCPTool",
                &format!(
                    "Call a tool on a configured local MCP stdio server. Available servers: {names}"
                ),
                json!({"type":"object","properties":{"server_name":{"type":"string"},"tool_name":{"type":"string"},"arguments":{"type":"object"}},"required":["server_name","tool_name"],"additionalProperties":false}),
                Some("tool.mcp.invoke"),
                true,
            ));
        }
        tools
    }

    async fn emit(&self, kind: RunEventKind, payload: Value) -> Result<()> {
        let database = self.daemon.database.lock().await;
        let state: String = database.query_row(
            "SELECT state FROM daemon_runs WHERE id = ?1",
            [self.run_id.to_string()],
            |row| row.get(0),
        )?;
        if state == "canceled" {
            bail!("run was canceled");
        }
        append_event(&database, self.run_id, kind, payload)
    }

    async fn execute_tool(&self, invocation: ToolInvocation) -> Result<ToolOutput> {
        if invocation.name == "Think" {
            return Ok(ToolOutput {
                content: invocation
                    .arguments
                    .get("thought")
                    .and_then(Value::as_str)
                    .unwrap_or("Thought recorded.")
                    .to_owned(),
                is_error: false,
                safe_to_resume: true,
                metadata: Value::Null,
            });
        }
        if invocation.name == "AskUser" {
            let response =
                await_local_input(self.daemon, self.run_id, invocation.arguments.clone()).await?;
            return Ok(ToolOutput {
                content: response.to_string(),
                is_error: false,
                safe_to_resume: true,
                metadata: Value::Null,
            });
        }
        let mutating = matches!(
            invocation.name.as_str(),
            "Write"
                | "Append"
                | "Edit"
                | "MultiEdit"
                | "CreateDirectory"
                | "MovePath"
                | "CopyPath"
                | "DeleteFile"
                | "TaskCreate"
                | "TaskUpdate"
                | "MemoryWrite"
                | "SaveSkill"
                | "Bash"
                | "MCPTool"
                | "BrowserClick"
                | "BrowserFill"
                | "BrowserUpload"
                | "OfficeReplaceText"
                | "DesktopOpenOffice"
                | "DesktopFocusWindow"
                | "DesktopLaunchApp"
                | "DesktopMoveMouse"
                | "DesktopClick"
                | "DesktopTypeText"
                | "DesktopKeypress"
                | "DesktopScroll"
        );
        if mutating && self.tool_policy == "read_only" {
            return Ok(ToolOutput {
                content: "Denied by the project's read-only tool policy.".to_owned(),
                is_error: true,
                safe_to_resume: true,
                metadata: json!({"policy": self.tool_policy}),
            });
        }
        if mutating && self.tool_policy == "confirm_mutations" {
            let approved = await_local_approval(
                self.daemon,
                self.run_id,
                json!({
                    "tool": invocation.name,
                    "arguments": invocation.arguments,
                    "tool_call_id": invocation.id,
                }),
            )
            .await?;
            if !approved {
                return Ok(ToolOutput {
                    content: "The local user rejected the tool request.".to_owned(),
                    is_error: true,
                    safe_to_resume: true,
                    metadata: json!({"approval": "rejected"}),
                });
            }
        }
        if mutating {
            self.checkpoint(
                json!({
                    "phase": "tool_dispatched",
                    "tool_call_id": invocation.id,
                    "tool": invocation.name,
                    "arguments": invocation.arguments,
                }),
                false,
            )
            .await?;
        }
        if invocation.name == "MCPTool" {
            return self.execute_mcp_tool(&invocation).await;
        }
        if matches!(
            invocation.name.as_str(),
            "TaskCreate"
                | "TaskList"
                | "TaskUpdate"
                | "MemoryRead"
                | "MemoryWrite"
                | "ChatSearch"
                | "Skill"
                | "SaveSkill"
        ) {
            return self.execute_state_tool(&invocation).await;
        }
        if matches!(invocation.name.as_str(), "WebFetch" | "WebSearch") {
            return self.execute_web_tool(&invocation).await;
        }
        if invocation.name.starts_with("Browser") {
            return self.execute_browser_tool(&invocation).await;
        }
        if invocation.name.starts_with("Office") || invocation.name == "DesktopOpenOffice" {
            return self.execute_office_tool(&invocation).await;
        }
        if invocation.name.starts_with("Desktop") || invocation.name == "Desktopscreenshot" {
            return self.execute_desktop_tool(&invocation).await;
        }
        self.execute_workspace_tool(&invocation).await
    }

    async fn checkpoint(&self, state: Value, safe_to_resume: bool) -> Result<()> {
        let database = self.daemon.database.lock().await;
        let sequence: i64 = database.query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM daemon_run_checkpoints WHERE run_id = ?1",
            [self.run_id.to_string()],
            |row| row.get(0),
        )?;
        let id = Uuid::new_v4();
        database.execute(
            "INSERT INTO daemon_run_checkpoints (id, run_id, sequence, safe_to_resume, executor_state, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id.to_string(), self.run_id.to_string(), sequence, safe_to_resume, serde_json::to_string(&state)?, Utc::now().to_rfc3339()],
        )?;
        append_event(
            &database,
            self.run_id,
            RunEventKind::CheckpointCreated,
            json!({"id": id, "sequence": sequence, "safe_to_resume": safe_to_resume}),
        )
    }
}

impl LocalRuntimeHost<'_> {
    async fn execute_web_tool(&self, invocation: &ToolInvocation) -> Result<ToolOutput> {
        let string_argument = |name: &str| -> Result<&str> {
            invocation
                .arguments
                .get(name)
                .and_then(Value::as_str)
                .with_context(|| format!("{} requires string argument {name}", invocation.name))
        };
        match invocation.name.as_str() {
            "WebFetch" => {
                let max_chars = invocation
                    .arguments
                    .get("max_chars")
                    .and_then(Value::as_u64)
                    .unwrap_or(50_000)
                    .clamp(1, 200_000) as usize;
                let requested_url = string_argument("url")?;
                let requested_origin = network_safety::origin_for_audit(requested_url);
                let response = network_safety::fetch_public_text(
                    requested_url,
                    network_safety::MAX_TEXT_RESPONSE_BYTES,
                )
                .await
                .map_err(anyhow::Error::msg)?;
                let title = extract_html_title(&response.body);
                let normalized = if response.content_type == "text/html"
                    || response.content_type == "application/xhtml+xml"
                {
                    decode_html_entities(&strip_html_like_content(&response.body))
                } else {
                    response.body
                };
                let truncated = response.truncated || normalized.chars().count() > max_chars;
                Ok(ToolOutput {
                    content: truncate_chars(normalized.trim(), max_chars),
                    is_error: !response.status.is_success(),
                    safe_to_resume: true,
                    metadata: json!({
                        "url": response.final_url,
                        "requested_origin": requested_origin,
                        "status": response.status.as_u16(),
                        "content_type": response.content_type,
                        "title": title,
                        "truncated": truncated,
                    }),
                })
            }
            "WebSearch" => {
                let query = string_argument("query")?.trim();
                if query.is_empty() {
                    bail!("WebSearch query must not be empty");
                }
                let max_results = invocation
                    .arguments
                    .get("max_results")
                    .and_then(Value::as_u64)
                    .unwrap_or(5)
                    .clamp(1, 10) as usize;
                let encoded =
                    url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>();
                let response = network_safety::fetch_public_text(
                    &format!("https://html.duckduckgo.com/html/?q={encoded}"),
                    network_safety::MAX_TEXT_RESPONSE_BYTES,
                )
                .await
                .map_err(anyhow::Error::msg)?;
                if !response.status.is_success() {
                    bail!("web search returned HTTP {}", response.status.as_u16());
                }
                let results = parse_duckduckgo_results(&response.body, max_results);
                let content = if results.is_empty() {
                    format!("No results for \"{query}\"")
                } else {
                    results
                        .iter()
                        .enumerate()
                        .map(|(index, result)| {
                            format!(
                                "{}. {}\n{}{}",
                                index + 1,
                                result.title,
                                result.url,
                                if result.snippet.is_empty() {
                                    String::new()
                                } else {
                                    format!("\n{}", result.snippet)
                                }
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n")
                };
                Ok(ToolOutput {
                    content,
                    is_error: false,
                    safe_to_resume: true,
                    metadata: json!({"query": query, "result_count": results.len()}),
                })
            }
            other => bail!("unsupported web tool {other}"),
        }
    }

    async fn execute_desktop_tool(&self, invocation: &ToolInvocation) -> Result<ToolOutput> {
        let action = match invocation.name.as_str() {
            "DesktopPrimaryDisplay" => "display",
            "Desktopscreenshot" => "screenshot",
            "DesktopListWindows" => "list_windows",
            "DesktopFocusWindow" => "focus_window",
            "DesktopLaunchApp" => "launch",
            "DesktopMoveMouse" => "move_mouse",
            "DesktopClick" => "click",
            "DesktopTypeText" => "type_text",
            "DesktopKeypress" => "keypress",
            "DesktopScroll" => "scroll",
            other => bail!("unsupported desktop tool {other}"),
        };
        let workspace = self
            .workspace
            .as_ref()
            .context("desktop tools require a project workspace for audit artifacts")?;
        let mut arguments = invocation.arguments.clone();
        if invocation.name == "DesktopTypeText" {
            let text = arguments
                .get("text")
                .and_then(Value::as_str)
                .context("DesktopTypeText requires text")?;
            let send_keys = desktop::send_keys_text(text);
            arguments
                .as_object_mut()
                .context("desktop arguments must be an object")?
                .insert("send_keys".to_owned(), Value::String(send_keys));
        } else if invocation.name == "DesktopKeypress" {
            let keys = arguments
                .get("keys")
                .and_then(Value::as_array)
                .context("DesktopKeypress requires keys")?
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .context("desktop keys must be strings")
                })
                .collect::<Result<Vec<_>>>()?;
            let send_keys = desktop::send_keys_chord(&keys)?;
            arguments
                .as_object_mut()
                .context("desktop arguments must be an object")?
                .insert("send_keys".to_owned(), Value::String(send_keys));
        }
        let screenshot_relative = format!(
            "artifacts/desktop/{}-{}.png",
            self.run_id,
            Utc::now().format("%Y%m%dT%H%M%S%3fZ")
        );
        let screenshot_path = safe_workspace_path(workspace, &screenshot_relative, false)?;
        let mut output = desktop::execute(action, &arguments, &screenshot_path).await?;
        if invocation.name == "Desktopscreenshot" {
            self.emit(
                RunEventKind::ArtifactCreated,
                json!({"path":screenshot_relative,"source":invocation.name,"storage":"project_workspace"}),
            )
            .await?;
            if let Some(object) = output.as_object_mut() {
                object.insert("path".to_owned(), Value::String(screenshot_relative));
            }
        }
        Ok(ToolOutput {
            content: serde_json::to_string_pretty(&output)?,
            is_error: false,
            safe_to_resume: true,
            metadata: output,
        })
    }

    async fn execute_office_tool(&self, invocation: &ToolInvocation) -> Result<ToolOutput> {
        let workspace = self
            .workspace
            .as_ref()
            .context("local Office tools require a project workspace")?;
        let argument = |name: &str| {
            invocation
                .arguments
                .get(name)
                .and_then(Value::as_str)
                .with_context(|| format!("{} requires {name}", invocation.name))
        };
        let source = safe_workspace_path(workspace, argument("path")?, true)?;
        let stamp = Utc::now().format("%Y%m%dT%H%M%S%3fZ").to_string();
        let stem = source
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("office");
        let mut output = match invocation.name.as_str() {
            "OfficeInspect" => {
                let temporary_root = self
                    .daemon
                    .config
                    .database_path
                    .parent()
                    .context("daemon database has no parent directory")?
                    .join("office-inspection")
                    .join(self.run_id.to_string());
                let inspected = office::inspect_document(&source, &temporary_root).await;
                let _ = fs::remove_dir(&temporary_root);
                inspected?
            }
            "OfficeReplaceText" => {
                let target = safe_workspace_path(
                    workspace,
                    invocation
                        .arguments
                        .get("output_path")
                        .and_then(Value::as_str)
                        .unwrap_or(argument("path")?),
                    false,
                )?;
                let old_text = argument("old_text")?.to_owned();
                let new_text = argument("new_text")?.to_owned();
                let replace_all = invocation
                    .arguments
                    .get("replace_all")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                tokio::task::spawn_blocking(move || {
                    office::replace_text(&source, &target, &old_text, &new_text, replace_all)
                })
                .await
                .context("Office replacement task panicked")??
            }
            "OfficeExportPdf" => {
                let relative = invocation
                    .arguments
                    .get("output_path")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("artifacts/office/{stem}-{stamp}.pdf"));
                let target = safe_workspace_path(workspace, &relative, false)?;
                office::export_pdf(&source, &target).await?
            }
            "OfficePreview" => {
                let pdf = if source
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case("pdf"))
                {
                    source.clone()
                } else {
                    let target = safe_workspace_path(
                        workspace,
                        &format!("artifacts/office/{stem}-{stamp}.pdf"),
                        false,
                    )?;
                    office::export_pdf(&source, &target).await?;
                    target
                };
                let preview_dir = safe_workspace_path(
                    workspace,
                    &format!("artifacts/office/{stem}-{stamp}-preview"),
                    false,
                )?;
                let all_pages = invocation
                    .arguments
                    .get("all_pages")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let dpi = invocation
                    .arguments
                    .get("dpi")
                    .and_then(Value::as_u64)
                    .unwrap_or(120)
                    .clamp(72, 200) as u16;
                tokio::task::spawn_blocking(move || {
                    office::preview_pdf(&pdf, &preview_dir, all_pages, dpi)
                })
                .await
                .context("Office preview task panicked")??
            }
            "DesktopOpenOffice" => office::open_visible(&source).await?,
            other => bail!("unsupported local Office tool {other}"),
        };
        let artifact_values = output
            .get("artifacts")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut relative_artifacts = Vec::new();
        for value in artifact_values {
            let Some(path) = value.as_str() else { continue };
            let path = PathBuf::from(path);
            let relative = relative_path(workspace, &path)?;
            relative_artifacts.push(Value::String(relative.clone()));
            self.emit(
                RunEventKind::ArtifactCreated,
                json!({"path":relative,"source":invocation.name,"storage":"project_workspace"}),
            )
            .await?;
        }
        if let Some(object) = output.as_object_mut() {
            object.insert("artifacts".to_owned(), Value::Array(relative_artifacts));
        }
        Ok(ToolOutput {
            content: serde_json::to_string_pretty(&output)?,
            is_error: false,
            safe_to_resume: true,
            metadata: output,
        })
    }

    async fn execute_browser_tool(&self, invocation: &ToolInvocation) -> Result<ToolOutput> {
        let action = match invocation.name.as_str() {
            "BrowserNavigate" => "navigate",
            "BrowserClick" => "click",
            "BrowserFill" => "fill",
            "BrowserUpload" => "upload",
            "BrowserScreenshot" => "screenshot",
            "BrowserTraceStart" => "trace_start",
            "BrowserTraceStop" => "trace_stop",
            "BrowserInspect" => "inspect",
            "BrowserTabs" => "tabs",
            other => bail!("unsupported local browser tool {other}"),
        };
        let workspace = self
            .workspace
            .as_ref()
            .context("local browser tools require a project workspace")?;
        let mut payload = invocation.arguments.clone();
        payload
            .as_object_mut()
            .context("browser tool arguments must be an object")?
            .insert("action".to_owned(), Value::String(action.to_owned()));
        let profile = workspace
            .join(".cowork")
            .join("browser-runs")
            .join(self.run_id.to_string())
            .join("profile");
        let output = developer_browser::local_browser_execute(
            &self.daemon.browser,
            &profile,
            workspace,
            &payload,
        )
        .await
        .map_err(anyhow::Error::msg)?;
        if let Some(artifacts) = output.get("artifacts").and_then(Value::as_array) {
            for path in artifacts.iter().filter_map(Value::as_str) {
                self.emit(
                    RunEventKind::ArtifactCreated,
                    json!({"path":path,"source":invocation.name,"storage":"project_workspace"}),
                )
                .await?;
            }
        }
        Ok(ToolOutput {
            content: serde_json::to_string_pretty(&output)?,
            is_error: false,
            safe_to_resume: true,
            metadata: output,
        })
    }

    async fn execute_mcp_tool(&self, invocation: &ToolInvocation) -> Result<ToolOutput> {
        let server_name = invocation
            .arguments
            .get("server_name")
            .and_then(Value::as_str)
            .context("MCPTool server_name is required")?;
        let tool_name = invocation
            .arguments
            .get("tool_name")
            .and_then(Value::as_str)
            .context("MCPTool tool_name is required")?;
        if tool_name.trim().is_empty() || tool_name.len() > 1024 {
            bail!("MCP tool name is missing or too long");
        }
        let server = self
            .mcp_servers
            .iter()
            .find(|server| server.name == server_name)
            .cloned()
            .with_context(|| {
                format!("MCP server {server_name:?} is not configured for this run")
            })?;
        let tool_args = invocation
            .arguments
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let tool_args = tool_args
            .as_object()
            .context("MCPTool arguments must be an object")?
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let request = mcp::McpCallRequest {
            name: server.name,
            command: server.command,
            args: server.args,
            env: server.env,
            tool_name: tool_name.to_owned(),
            tool_args,
        };
        let response = tokio::task::spawn_blocking(move || mcp::call_tool(request))
            .await
            .context("MCP worker task panicked")?
            .map_err(|error| anyhow::anyhow!(error))?;
        let metadata = serde_json::to_value(&response)?;
        Ok(ToolOutput {
            content: if response.success {
                response.result
            } else {
                response.error.unwrap_or_else(|| response.result.clone())
            },
            is_error: !response.success,
            safe_to_resume: true,
            metadata,
        })
    }

    async fn execute_state_tool(&self, invocation: &ToolInvocation) -> Result<ToolOutput> {
        let string_argument = |name: &str| -> Result<&str> {
            invocation
                .arguments
                .get(name)
                .and_then(Value::as_str)
                .with_context(|| format!("{} requires string argument {name}", invocation.name))
        };
        let mut database = self.daemon.database.lock().await;
        let thread_id = run_thread_id(&database, self.run_id)?;
        let client_thread_id = run_client_thread_id(&database, self.run_id)?;
        let content = match invocation.name.as_str() {
            "TaskCreate" => {
                let title = string_argument("title")?.trim();
                if title.is_empty() || title.chars().count() > 500 {
                    bail!("TaskCreate title must contain between one and 500 characters");
                }
                let description = string_argument("description")?;
                if description.chars().count() > 200_000 {
                    bail!("TaskCreate description exceeds 200000 characters");
                }
                let id = Uuid::new_v4().to_string();
                let entity = write_daemon_entity(
                    &mut database,
                    "task",
                    &id,
                    json!({
                        "title": title,
                        "description": description,
                        "status": "pending",
                        "note": null,
                        "thread_id": client_thread_id,
                        "source_run_id": self.run_id,
                    }),
                    None,
                )?;
                format!("Task created: {title} (ID: {})", entity["id"])
            }
            "TaskList" => {
                let status = invocation.arguments.get("status").and_then(Value::as_str);
                let entities = list_daemon_entities(&database, "task", false)?;
                let tasks = entities
                    .into_iter()
                    .filter(|entity| {
                        status.is_none_or(|expected| {
                            entity["payload"]["status"].as_str() == Some(expected)
                        })
                    })
                    .collect::<Vec<_>>();
                if tasks.is_empty() {
                    "No tasks found.".to_owned()
                } else {
                    let list = tasks
                        .iter()
                        .map(|entity| {
                            format!(
                                "- [{}] {} ({})",
                                entity["payload"]["status"].as_str().unwrap_or("pending"),
                                entity["payload"]["title"].as_str().unwrap_or("Untitled"),
                                entity["id"].as_str().unwrap_or_default(),
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    format!("Tasks ({}):\n{list}", tasks.len())
                }
            }
            "TaskUpdate" => {
                let requested_id = string_argument("task_id")?.trim();
                let matching = list_daemon_entities(&database, "task", false)?
                    .into_iter()
                    .filter(|entity| {
                        entity["id"]
                            .as_str()
                            .is_some_and(|id| id == requested_id || id.starts_with(requested_id))
                    })
                    .collect::<Vec<_>>();
                if matching.len() != 1 {
                    bail!(
                        "TaskUpdate task_id must identify exactly one task; matches={}",
                        matching.len()
                    );
                }
                let current = &matching[0];
                let id = current["id"].as_str().context("task id is missing")?;
                let revision = current["revision"]
                    .as_i64()
                    .context("task revision is missing")?;
                let mut payload = current["payload"]
                    .as_object()
                    .context("task payload is invalid")?
                    .clone();
                if let Some(status) = invocation.arguments.get("status").and_then(Value::as_str) {
                    if !matches!(
                        status,
                        "pending" | "running" | "completed" | "failed" | "canceled"
                    ) {
                        bail!("TaskUpdate status is invalid");
                    }
                    payload.insert("status".to_owned(), Value::String(status.to_owned()));
                }
                if let Some(note) = invocation.arguments.get("note").and_then(Value::as_str) {
                    payload.insert("note".to_owned(), Value::String(note.to_owned()));
                }
                payload.insert(
                    "updated_by_run_id".to_owned(),
                    Value::String(self.run_id.to_string()),
                );
                let entity = write_daemon_entity(
                    &mut database,
                    "task",
                    id,
                    Value::Object(payload),
                    Some(revision),
                )?;
                format!(
                    "Task {} updated: {}",
                    entity["id"].as_str().unwrap_or(id),
                    entity["payload"]["status"].as_str().unwrap_or("pending")
                )
            }
            "MemoryRead" => {
                let scope = invocation
                    .arguments
                    .get("scope")
                    .and_then(Value::as_str)
                    .unwrap_or("agent");
                let key = invocation.arguments.get("key").and_then(Value::as_str);
                let memories = list_daemon_entities(&database, "memory", false)?
                    .into_iter()
                    .filter(|entity| entity["payload"]["scope"].as_str() == Some(scope))
                    .filter(|entity| {
                        scope != "chat"
                            || entity["payload"]["scope_ref"].as_str() == Some(&thread_id)
                    })
                    .filter(|entity| {
                        key.is_none_or(|fragment| {
                            entity["payload"]["key"]
                                .as_str()
                                .is_some_and(|value| value.contains(fragment))
                        })
                    })
                    .collect::<Vec<_>>();
                if memories.is_empty() {
                    "No memories found.".to_owned()
                } else {
                    memories
                        .iter()
                        .map(|entity| {
                            format!(
                                "[{}/{}]: {}",
                                entity["payload"]["scope"].as_str().unwrap_or("agent"),
                                entity["payload"]["key"].as_str().unwrap_or("memory"),
                                entity["payload"]["content"].as_str().unwrap_or_default(),
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n")
                }
            }
            "MemoryWrite" => {
                let action = string_argument("action")?;
                let target = string_argument("target")?;
                let scope = invocation
                    .arguments
                    .get("scope")
                    .and_then(Value::as_str)
                    .unwrap_or(if target == "user" { "user" } else { "agent" });
                if !matches!(scope, "agent" | "user" | "chat" | "shared") {
                    bail!("MemoryWrite scope is invalid");
                }
                let scope_ref = (scope == "chat").then_some(thread_id.as_str());
                let candidates = list_daemon_entities(&database, "memory", false)?
                    .into_iter()
                    .filter(|entity| entity["payload"]["scope"].as_str() == Some(scope))
                    .filter(|entity| {
                        scope_ref.is_none_or(|expected| {
                            entity["payload"]["scope_ref"].as_str() == Some(expected)
                        })
                    })
                    .collect::<Vec<_>>();
                match action {
                    "add" => {
                        let value = invocation
                            .arguments
                            .get("content")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .context("MemoryWrite add requires non-empty content")?;
                        if let Some(existing) = candidates
                            .iter()
                            .find(|entity| entity["payload"]["content"].as_str() == Some(value))
                        {
                            format!(
                                "Memory already exists: {}",
                                existing["payload"]["key"].as_str().unwrap_or("memory")
                            )
                        } else {
                            let id = Uuid::new_v4().to_string();
                            let key = invocation
                                .arguments
                                .get("key")
                                .and_then(Value::as_str)
                                .map(str::trim)
                                .filter(|value| !value.is_empty())
                                .map(str::to_owned)
                                .unwrap_or_else(|| format!("memory-{}", &id[..8]));
                            write_daemon_entity(
                                &mut database,
                                "memory",
                                &id,
                                json!({
                                    "scope": scope,
                                    "scope_ref": scope_ref,
                                    "key": key,
                                    "content": value,
                                    "target": target,
                                    "source_run_id": self.run_id,
                                }),
                                None,
                            )?;
                            format!("Memory saved: [{scope}/{key}]")
                        }
                    }
                    "replace" | "remove" => {
                        let old_text = invocation
                            .arguments
                            .get("old_text")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .context("MemoryWrite replace/remove requires old_text")?;
                        let matching = candidates
                            .iter()
                            .filter(|entity| {
                                entity["payload"]["content"]
                                    .as_str()
                                    .is_some_and(|content| content.contains(old_text))
                            })
                            .collect::<Vec<_>>();
                        if matching.len() != 1 {
                            bail!(
                                "old_text must identify exactly one memory; matches={}",
                                matching.len()
                            );
                        }
                        let current = matching[0];
                        let id = current["id"].as_str().context("memory id is missing")?;
                        let revision = current["revision"]
                            .as_i64()
                            .context("memory revision is missing")?;
                        if action == "remove" {
                            tombstone_daemon_entity(&mut database, "memory", id, Some(revision))?;
                            format!("Memory removed: {id}")
                        } else {
                            let replacement = invocation
                                .arguments
                                .get("content")
                                .and_then(Value::as_str)
                                .context("MemoryWrite replace requires content")?;
                            let mut payload = current["payload"]
                                .as_object()
                                .context("memory payload is invalid")?
                                .clone();
                            let updated = payload["content"].as_str().unwrap_or_default().replacen(
                                old_text,
                                replacement,
                                1,
                            );
                            payload.insert("content".to_owned(), Value::String(updated));
                            payload.insert(
                                "source_run_id".to_owned(),
                                Value::String(self.run_id.to_string()),
                            );
                            write_daemon_entity(
                                &mut database,
                                "memory",
                                id,
                                Value::Object(payload),
                                Some(revision),
                            )?;
                            format!("Memory replaced: {id}")
                        }
                    }
                    _ => bail!("MemoryWrite action is invalid"),
                }
            }
            "ChatSearch" => {
                let query = string_argument("query")?.trim();
                if query.is_empty() {
                    bail!("ChatSearch query must not be empty");
                }
                let limit = invocation
                    .arguments
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(10)
                    .clamp(1, 50) as usize;
                search_durable_chats(&database, query, limit)?
            }
            "Skill" => {
                let requested = string_argument("skill_name")?.trim();
                let input = string_argument("input")?;
                let matching = list_daemon_entities(&database, "skill", false)?
                    .into_iter()
                    .filter(|entity| {
                        entity["payload"]["name"]
                            .as_str()
                            .is_some_and(|name| name.trim().eq_ignore_ascii_case(requested))
                    })
                    .collect::<Vec<_>>();
                if matching.len() != 1 {
                    let available = list_daemon_entities(&database, "skill", false)?
                        .iter()
                        .filter_map(|entity| entity["payload"]["name"].as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    bail!(
                        "Skill \"{requested}\" is not uniquely registered.{}",
                        if available.is_empty() {
                            String::new()
                        } else {
                            format!(" Available skills: {available}")
                        }
                    );
                }
                let skill = &matching[0]["payload"];
                let name = skill["name"].as_str().unwrap_or(requested);
                let mode = skill["run_mode"].as_str().unwrap_or("execute");
                let rendered = render_skill_template(
                    skill["prompt_template"].as_str().unwrap_or_default(),
                    name,
                    input,
                );
                format!(
                    "Run the centrally registered skill \"{name}\" in {mode} mode.\n\n{rendered}"
                )
            }
            "SaveSkill" => {
                let name = string_argument("name")?.trim();
                let description = string_argument("description")?.trim();
                let prompt_template = string_argument("prompt_template")?.trim();
                if name.is_empty() || prompt_template.is_empty() {
                    bail!("SaveSkill name and prompt_template must not be empty");
                }
                let run_mode = invocation
                    .arguments
                    .get("run_mode")
                    .and_then(Value::as_str)
                    .unwrap_or("execute");
                if !matches!(run_mode, "execute" | "plan" | "hybrid") {
                    bail!("SaveSkill run_mode is invalid");
                }
                let matching = list_daemon_entities(&database, "skill", false)?
                    .into_iter()
                    .filter(|entity| {
                        entity["payload"]["name"]
                            .as_str()
                            .is_some_and(|existing| existing.trim().eq_ignore_ascii_case(name))
                    })
                    .collect::<Vec<_>>();
                if matching.len() > 1 {
                    bail!("SaveSkill found duplicate existing skill names");
                }
                let (id, expected_revision) = matching
                    .first()
                    .map(|entity| -> Result<(String, i64)> {
                        Ok((
                            entity["id"]
                                .as_str()
                                .context("skill id is missing")?
                                .to_owned(),
                            entity["revision"]
                                .as_i64()
                                .context("skill revision is missing")?,
                        ))
                    })
                    .transpose()?
                    .map(|(id, revision)| (id, Some(revision)))
                    .unwrap_or_else(|| (Uuid::new_v4().to_string(), None));
                write_daemon_entity(
                    &mut database,
                    "skill",
                    &id,
                    json!({
                        "name": name,
                        "description": description,
                        "prompt_template": prompt_template,
                        "trigger_pattern": invocation.arguments.get("trigger_pattern").and_then(Value::as_str).map(str::trim).filter(|value| !value.is_empty()),
                        "run_mode": run_mode,
                        "auto_generated": true,
                        "source_run_id": self.run_id,
                    }),
                    expected_revision,
                )?;
                format!("Skill \"{name}\" was saved in the central daemon skill store.")
            }
            other => bail!("unsupported state tool {other}"),
        };
        Ok(ToolOutput {
            content,
            is_error: false,
            safe_to_resume: true,
            metadata: Value::Null,
        })
    }

    async fn execute_workspace_tool(&self, invocation: &ToolInvocation) -> Result<ToolOutput> {
        let root = self
            .workspace
            .as_deref()
            .context("this project has no local workspace binding")?;
        let string_argument = |name: &str| -> Result<&str> {
            invocation
                .arguments
                .get(name)
                .and_then(Value::as_str)
                .with_context(|| format!("{} requires string argument {name}", invocation.name))
        };
        let (content, is_error, safe_to_resume, metadata) = match invocation.name.as_str() {
            "Read" => {
                let path = safe_workspace_path(root, string_argument("path")?, true)?;
                let text = tokio::fs::read_to_string(&path).await?;
                let offset = invocation
                    .arguments
                    .get("offset")
                    .and_then(Value::as_u64)
                    .unwrap_or(1) as usize;
                let limit = invocation
                    .arguments
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(2000)
                    .min(20_000) as usize;
                let selected = text
                    .lines()
                    .skip(offset.saturating_sub(1))
                    .take(limit)
                    .collect::<Vec<_>>()
                    .join("\n");
                (
                    selected,
                    false,
                    true,
                    json!({"path": relative_path(root, &path)?}),
                )
            }
            "Write" => {
                let path = safe_workspace_path(root, string_argument("path")?, false)?;
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::write(&path, string_argument("content")?).await?;
                (
                    format!("wrote {}", relative_path(root, &path)?),
                    false,
                    true,
                    Value::Null,
                )
            }
            "Append" => {
                let path = safe_workspace_path_no_symlinks(root, string_argument("path")?, false)?;
                if path.exists() {
                    reject_symlink(&path)?;
                    if !path.is_file() {
                        bail!("Append requires a file path");
                    }
                }
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                let mut file = tokio::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .await?;
                file.write_all(string_argument("content")?.as_bytes())
                    .await?;
                file.flush().await?;
                (
                    format!("appended {}", relative_path(root, &path)?),
                    false,
                    true,
                    Value::Null,
                )
            }
            "Edit" => {
                let path = safe_workspace_path(root, string_argument("path")?, true)?;
                let text = tokio::fs::read_to_string(&path).await?;
                let old = string_argument("old_string")?;
                let new = string_argument("new_string")?;
                let count = text.matches(old).count();
                if count == 0 {
                    bail!("old_string was not found");
                }
                let all = invocation
                    .arguments
                    .get("replace_all")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !all && count != 1 {
                    bail!(
                        "old_string occurs {count} times; set replace_all or provide more context"
                    );
                }
                let updated = if all {
                    text.replace(old, new)
                } else {
                    text.replacen(old, new, 1)
                };
                tokio::fs::write(&path, updated).await?;
                (
                    format!("replacements={}", if all { count } else { 1 }),
                    false,
                    true,
                    Value::Null,
                )
            }
            "MultiEdit" => {
                let path = safe_workspace_path_no_symlinks(root, string_argument("path")?, true)?;
                let mut text = tokio::fs::read_to_string(&path).await?;
                let edits = invocation
                    .arguments
                    .get("edits")
                    .and_then(Value::as_array)
                    .context("MultiEdit requires an edits array")?;
                if edits.is_empty() || edits.len() > 1000 {
                    bail!("MultiEdit requires between one and 1000 edits");
                }
                let mut replacements = 0usize;
                for (index, edit) in edits.iter().enumerate() {
                    let old = edit
                        .get("old_string")
                        .and_then(Value::as_str)
                        .with_context(|| format!("edit {index} requires old_string"))?;
                    let new = edit
                        .get("new_string")
                        .and_then(Value::as_str)
                        .with_context(|| format!("edit {index} requires new_string"))?;
                    if old.is_empty() {
                        bail!("edit {index} old_string must not be empty");
                    }
                    let count = text.matches(old).count();
                    if count == 0 {
                        bail!("edit {index} old_string was not found");
                    }
                    let all = edit
                        .get("replace_all")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    if !all && count != 1 {
                        bail!("edit {index} old_string occurs {count} times; set replace_all or provide more context");
                    }
                    if all {
                        text = text.replace(old, new);
                        replacements += count;
                    } else {
                        text = text.replacen(old, new, 1);
                        replacements += 1;
                    }
                }
                tokio::fs::write(&path, text).await?;
                (
                    format!("replacements={replacements}"),
                    false,
                    true,
                    json!({"edits": edits.len(), "replacements": replacements}),
                )
            }
            "CreateDirectory" => {
                let path = safe_workspace_path_no_symlinks(root, string_argument("path")?, false)?;
                tokio::fs::create_dir_all(&path).await?;
                (
                    format!("created {}", relative_path(root, &path)?),
                    false,
                    true,
                    Value::Null,
                )
            }
            "MovePath" => {
                let source =
                    safe_workspace_path_no_symlinks(root, string_argument("source")?, true)?;
                let destination =
                    safe_workspace_path_no_symlinks(root, string_argument("destination")?, false)?;
                if destination.exists() {
                    bail!("MovePath destination already exists");
                }
                if source.is_dir() && destination.starts_with(&source) {
                    bail!("MovePath cannot move a directory into itself");
                }
                if let Some(parent) = destination.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::rename(&source, &destination).await?;
                (
                    format!(
                        "moved {} to {}",
                        relative_path(root, &source)?,
                        relative_path(root, &destination)?
                    ),
                    false,
                    true,
                    Value::Null,
                )
            }
            "CopyPath" => {
                let source =
                    safe_workspace_path_no_symlinks(root, string_argument("source")?, true)?;
                let destination =
                    safe_workspace_path_no_symlinks(root, string_argument("destination")?, false)?;
                copy_workspace_path(&source, &destination)?;
                (
                    format!(
                        "copied {} to {}",
                        relative_path(root, &source)?,
                        relative_path(root, &destination)?
                    ),
                    false,
                    true,
                    Value::Null,
                )
            }
            "DeleteFile" => {
                let path = safe_workspace_path_no_symlinks(root, string_argument("path")?, true)?;
                if !path.is_file() {
                    bail!("DeleteFile only deletes files");
                }
                let relative = relative_path(root, &path)?;
                let data_root = self
                    .daemon
                    .config
                    .database_path
                    .parent()
                    .context("daemon database has no parent directory")?;
                let backup = data_root
                    .join("deleted-file-backups")
                    .join(self.run_id.to_string())
                    .join(Uuid::new_v4().to_string())
                    .join(Path::new(&relative));
                if let Some(parent) = backup.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(&path, &backup)?;
                tokio::fs::remove_file(&path).await?;
                (
                    format!("deleted {relative}; recoverable backup created"),
                    false,
                    true,
                    json!({"path": relative, "backup_path": backup}),
                )
            }
            "FileInfo" => {
                let path = safe_workspace_path_no_symlinks(root, string_argument("path")?, true)?;
                let metadata = fs::symlink_metadata(&path)?;
                if metadata.file_type().is_symlink() {
                    bail!("FileInfo does not follow symlinks");
                }
                let modified = metadata
                    .modified()
                    .ok()
                    .map(DateTime::<Utc>::from)
                    .map(|value| value.to_rfc3339());
                let value = json!({
                    "path": relative_path(root, &path)?,
                    "type": if metadata.is_file() { "file" } else if metadata.is_dir() { "directory" } else { "other" },
                    "size": metadata.is_file().then_some(metadata.len()),
                    "readonly": metadata.permissions().readonly(),
                    "modified_at": modified,
                });
                (serde_json::to_string(&value)?, false, true, value)
            }
            "ListDir" => {
                let path = safe_workspace_path(
                    root,
                    invocation
                        .arguments
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or("."),
                    true,
                )?;
                let mut reader = tokio::fs::read_dir(&path).await?;
                let mut items = Vec::new();
                while let Some(entry) = reader.next_entry().await? {
                    let metadata = entry.metadata().await?;
                    items.push(json!({"name": entry.file_name().to_string_lossy(), "type": if metadata.is_dir() {"dir"} else {"file"}, "size": metadata.is_file().then_some(metadata.len())}));
                }
                items.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
                (serde_json::to_string(&items)?, false, true, Value::Null)
            }
            "Glob" => {
                let pattern = string_argument("pattern")?;
                validate_relative_pattern(pattern)?;
                let absolute = root.join(pattern).to_string_lossy().into_owned();
                let mut items = Vec::new();
                for entry in glob::glob(&absolute)? {
                    let path = entry?;
                    if let Ok(path) = safe_existing_workspace_path(root, &path) {
                        items.push(relative_path(root, &path)?);
                    }
                    if items.len() >= 20_000 {
                        break;
                    }
                }
                items.sort();
                (items.join("\n"), false, true, Value::Null)
            }
            "Grep" => {
                let needle = string_argument("pattern")?;
                let case_sensitive = invocation
                    .arguments
                    .get("case_sensitive")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                let compared = if case_sensitive {
                    needle.to_owned()
                } else {
                    needle.to_lowercase()
                };
                let mut output = String::new();
                for entry in walkdir::WalkDir::new(root)
                    .follow_links(false)
                    .into_iter()
                    .filter_map(std::result::Result::ok)
                {
                    if !entry.file_type().is_file()
                        || entry
                            .metadata()
                            .map(|item| item.len() > 4 * 1024 * 1024)
                            .unwrap_or(true)
                    {
                        continue;
                    }
                    let Ok(text) = fs::read_to_string(entry.path()) else {
                        continue;
                    };
                    for (line_index, line) in text.lines().enumerate() {
                        let matches = if case_sensitive {
                            line.contains(&compared)
                        } else {
                            line.to_lowercase().contains(&compared)
                        };
                        if matches {
                            output.push_str(&format!(
                                "{}:{}:{}\n",
                                relative_path(root, entry.path())?,
                                line_index + 1,
                                line
                            ));
                            if output.len() >= 4 * 1024 * 1024 {
                                break;
                            }
                        }
                    }
                    if output.len() >= 4 * 1024 * 1024 {
                        break;
                    }
                }
                (output, false, true, Value::Null)
            }
            "Bash" => {
                let timeout = invocation
                    .arguments
                    .get("timeout_seconds")
                    .and_then(Value::as_u64)
                    .unwrap_or(900)
                    .clamp(1, 3600);
                let mut command = if cfg!(windows) {
                    let mut value = tokio::process::Command::new("powershell.exe");
                    value.args([
                        "-NoLogo",
                        "-NoProfile",
                        "-NonInteractive",
                        "-Command",
                        string_argument("command")?,
                    ]);
                    value
                } else {
                    let mut value = tokio::process::Command::new("/bin/bash");
                    value.args(["-lc", string_argument("command")?]);
                    value
                };
                command.current_dir(root).kill_on_drop(true);
                let result = tokio::time::timeout(Duration::from_secs(timeout), command.output())
                    .await
                    .context("shell command timed out")??;
                let stdout = String::from_utf8_lossy(&result.stdout);
                let stderr = String::from_utf8_lossy(&result.stderr);
                let combined = format!(
                    "{}{}{}",
                    truncate_chars(&stdout, 4 * 1024 * 1024),
                    if stderr.is_empty() {
                        ""
                    } else {
                        "\n[stderr]\n"
                    },
                    truncate_chars(&stderr, 4 * 1024 * 1024)
                );
                (
                    combined,
                    !result.status.success(),
                    result.status.success(),
                    json!({"exit_code": result.status.code()}),
                )
            }
            other => bail!("unsupported local tool {other}"),
        };
        Ok(ToolOutput {
            content,
            is_error,
            safe_to_resume,
            metadata,
        })
    }
}

fn local_tool(
    name: &str,
    description: &str,
    input_schema: Value,
    capability: Option<&str>,
    mutating: bool,
) -> ToolDefinition {
    ToolDefinition {
        name: name.to_owned(),
        description: description.to_owned(),
        input_schema,
        required_capability: capability.map(Capability::from),
        mutating,
    }
}

async fn await_local_approval(daemon: &Daemon, run_id: Uuid, request: Value) -> Result<bool> {
    let approval_id = Uuid::new_v4();
    let now = Utc::now();
    let expires_at = now + chrono::Duration::days(7);
    {
        let database = daemon.database.lock().await;
        database.execute(
            "INSERT INTO daemon_approval_requests (id, run_id, state, request_json, expires_at, created_at) VALUES (?1, ?2, 'pending', ?3, ?4, ?5)",
            params![approval_id.to_string(), run_id.to_string(), serde_json::to_string(&request)?, expires_at.to_rfc3339(), now.to_rfc3339()],
        )?;
        set_local_run_state_locked(&database, run_id, RunState::WaitingApproval)?;
        append_event(
            &database,
            run_id,
            RunEventKind::ApprovalRequested,
            json!({"id": approval_id, "request": request, "expires_at": expires_at}),
        )?;
    }
    loop {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let database = daemon.database.lock().await;
        let run_state: String = database.query_row(
            "SELECT state FROM daemon_runs WHERE id = ?1",
            [run_id.to_string()],
            |row| row.get(0),
        )?;
        if run_state == "canceled" {
            bail!("run was canceled while waiting for approval");
        }
        let state: String = database.query_row(
            "SELECT state FROM daemon_approval_requests WHERE id = ?1",
            [approval_id.to_string()],
            |row| row.get(0),
        )?;
        match state.as_str() {
            "approved" | "rejected" => {
                set_local_run_state_locked(&database, run_id, RunState::Running)?;
                return Ok(state == "approved");
            }
            "pending" if Utc::now() < expires_at => {}
            "pending" => {
                database.execute(
                    "UPDATE daemon_approval_requests SET state = 'expired', resolved_at = ?2 WHERE id = ?1 AND state = 'pending'",
                    params![approval_id.to_string(), Utc::now().to_rfc3339()],
                )?;
                set_local_run_state_locked(&database, run_id, RunState::Running)?;
                return Ok(false);
            }
            other => bail!("approval entered invalid state {other}"),
        }
    }
}

async fn await_local_input(daemon: &Daemon, run_id: Uuid, request: Value) -> Result<Value> {
    let input_id = Uuid::new_v4();
    let now = Utc::now();
    let expires_at = now + chrono::Duration::days(7);
    {
        let database = daemon.database.lock().await;
        database.execute(
            "INSERT INTO daemon_input_requests (id, run_id, state, request_json, expires_at, created_at) VALUES (?1, ?2, 'pending', ?3, ?4, ?5)",
            params![input_id.to_string(), run_id.to_string(), serde_json::to_string(&request)?, expires_at.to_rfc3339(), now.to_rfc3339()],
        )?;
        set_local_run_state_locked(&database, run_id, RunState::WaitingInput)?;
        append_event(
            &database,
            run_id,
            RunEventKind::InputRequested,
            json!({"id": input_id, "request": request, "expires_at": expires_at}),
        )?;
    }
    loop {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let database = daemon.database.lock().await;
        let run_state: String = database.query_row(
            "SELECT state FROM daemon_runs WHERE id = ?1",
            [run_id.to_string()],
            |row| row.get(0),
        )?;
        if run_state == "canceled" {
            bail!("run was canceled while waiting for input");
        }
        let row = database.query_row(
            "SELECT state, response_json FROM daemon_input_requests WHERE id = ?1",
            [input_id.to_string()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )?;
        match row.0.as_str() {
            "answered" => {
                set_local_run_state_locked(&database, run_id, RunState::Running)?;
                return row
                    .1
                    .map(|value| serde_json::from_str(&value))
                    .transpose()?
                    .context("answered input is missing its response");
            }
            "pending" if Utc::now() < expires_at => {}
            "pending" => {
                database.execute(
                    "UPDATE daemon_input_requests SET state = 'expired', resolved_at = ?2 WHERE id = ?1 AND state = 'pending'",
                    params![input_id.to_string(), Utc::now().to_rfc3339()],
                )?;
                bail!("input request expired");
            }
            other => bail!("input request entered invalid state {other}"),
        }
    }
}

fn set_local_run_state_locked(database: &Connection, run_id: Uuid, next: RunState) -> Result<()> {
    let encoded: String = database.query_row(
        "SELECT record_json FROM daemon_runs WHERE id = ?1",
        [run_id.to_string()],
        |row| row.get(0),
    )?;
    let mut record: RunRecord = serde_json::from_str(&encoded)?;
    if record.state == next {
        return Ok(());
    }
    if record.state.is_terminal() {
        bail!("cannot change a terminal run to {}", state_name(next));
    }
    let previous = record.state;
    record.state = next;
    record.revision += 1;
    record.updated_at = Utc::now();
    refresh_etag(&mut record);
    save_record(database, &record)?;
    append_event(
        database,
        run_id,
        RunEventKind::StateChanged,
        json!({"from": state_name(previous), "to": state_name(next)}),
    )
}

async fn run_workspace(daemon: &Daemon, run_id: Uuid) -> Result<Option<PathBuf>> {
    let database = daemon.database.lock().await;
    let path = database
        .query_row(
            "SELECT workspace_path FROM daemon_run_workspaces WHERE run_id = ?1",
            [run_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    path.map(PathBuf::from)
        .map(|path| {
            path.canonicalize()
                .context("server-run workspace is unavailable")
        })
        .transpose()
}

async fn project_workspace(daemon: &Daemon, project_id: Uuid) -> Result<Option<PathBuf>> {
    let database = daemon.database.lock().await;
    let path = database
        .query_row(
            "SELECT workspace_path FROM daemon_project_bindings WHERE project_id = ?1",
            [project_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    path.map(PathBuf::from)
        .map(|path| {
            path.canonicalize()
                .context("bound project workspace is unavailable")
        })
        .transpose()
}

async fn load_run_model_config(
    daemon: &Daemon,
    run_id: Uuid,
) -> Result<Option<PersistedModelConfig>> {
    let database = daemon.database.lock().await;
    let encrypted = database
        .query_row(
            "SELECT encrypted_config FROM daemon_run_model_configs WHERE run_id = ?1",
            [run_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    encrypted
        .map(|value| decrypt_model_config(&value, &daemon.config.model_secret_key))
        .transpose()
}

fn validate_relative_pattern(pattern: &str) -> Result<()> {
    if pattern.is_empty()
        || pattern.len() > 4096
        || Path::new(pattern).is_absolute()
        || pattern.contains('\0')
        || Path::new(pattern).components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir
                    | std::path::Component::Prefix(_)
                    | std::path::Component::RootDir
            )
        })
    {
        bail!("glob pattern must stay inside the project workspace");
    }
    Ok(())
}

fn run_thread_id(connection: &Connection, run_id: Uuid) -> Result<String> {
    connection
        .query_row(
            "SELECT thread_id FROM daemon_runs WHERE id = ?1",
            [run_id.to_string()],
            |row| row.get(0),
        )
        .context("run thread binding is missing")
}

fn run_client_thread_id(connection: &Connection, run_id: Uuid) -> Result<String> {
    let encoded: String = connection
        .query_row(
            "SELECT record_json FROM daemon_runs WHERE id = ?1",
            [run_id.to_string()],
            |row| row.get(0),
        )
        .context("run record is missing")?;
    let record: RunRecord = serde_json::from_str(&encoded)?;
    Ok(record
        .spec
        .input
        .get("client_thread_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| record.spec.thread_id.to_string()))
}

struct DaemonEntityRow<'a> {
    entity_type: &'a str,
    id: &'a str,
    revision: i64,
    etag: &'a str,
    payload_json: &'a str,
    tombstone: bool,
    created_at: &'a str,
    updated_at: &'a str,
}

fn daemon_entity_json(row: DaemonEntityRow<'_>) -> Result<Value> {
    Ok(json!({
        "entity_type": row.entity_type,
        "id": row.id,
        "revision": row.revision,
        "etag": row.etag,
        "payload": serde_json::from_str::<Value>(row.payload_json)?,
        "tombstone": row.tombstone,
        "created_at": row.created_at,
        "updated_at": row.updated_at,
    }))
}

fn validate_daemon_entity_identity(entity_type: &str, id: &str) -> Result<()> {
    if !matches!(
        entity_type,
        "project"
            | "thread"
            | "message"
            | "task"
            | "schedule"
            | "crew"
            | "skill"
            | "memory"
            | "provider_profile"
            | "secret_metadata"
            | "mcp_metadata"
    ) {
        bail!("unsupported daemon entity type {entity_type}");
    }
    if id.trim().is_empty() || id.len() > 500 || id.contains('\0') {
        bail!("daemon entity id is invalid");
    }
    Ok(())
}

fn write_daemon_entity(
    connection: &mut Connection,
    entity_type: &str,
    id: &str,
    payload: Value,
    expected_revision: Option<i64>,
) -> Result<Value> {
    validate_daemon_entity_identity(entity_type, id)?;
    if !payload.is_object() {
        bail!("daemon entity payload must be an object");
    }
    let payload_json = serde_json::to_string(&payload)?;
    let transaction = connection.transaction()?;
    let current = transaction
        .query_row(
            "SELECT revision, created_at, payload_json, tombstone, etag, updated_at FROM daemon_entities WHERE entity_type = ?1 AND id = ?2",
            params![entity_type, id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, bool>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?;
    if let Some(expected) = expected_revision {
        let actual = current.as_ref().map(|item| item.0).unwrap_or(0);
        if actual != expected {
            bail!("entity revision conflict: expected {expected}, actual {actual}");
        }
    }
    if let Some((revision, created_at, stored_payload, false, etag, updated_at)) = &current {
        if serde_json::from_str::<Value>(stored_payload)? == payload {
            return daemon_entity_json(DaemonEntityRow {
                entity_type,
                id,
                revision: *revision,
                etag,
                payload_json: stored_payload,
                tombstone: false,
                created_at,
                updated_at,
            });
        }
    }
    let revision = current.as_ref().map(|item| item.0 + 1).unwrap_or(1);
    let now = Utc::now().to_rfc3339();
    let created_at = current
        .as_ref()
        .map(|item| item.1.clone())
        .unwrap_or_else(|| now.clone());
    let etag = format!("W/\"{entity_type}:{id}:{revision}\"");
    transaction.execute(
        r#"
        INSERT INTO daemon_entities (
            entity_type, id, revision, etag, payload_json, tombstone, created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7)
        ON CONFLICT(entity_type, id) DO UPDATE SET
            revision = excluded.revision,
            etag = excluded.etag,
            payload_json = excluded.payload_json,
            tombstone = 0,
            updated_at = excluded.updated_at
        "#,
        params![
            entity_type,
            id,
            revision,
            etag,
            payload_json,
            created_at,
            now,
        ],
    )?;
    let entity = daemon_entity_json(DaemonEntityRow {
        entity_type,
        id,
        revision,
        etag: &etag,
        payload_json: &payload_json,
        tombstone: false,
        created_at: &created_at,
        updated_at: &now,
    })?;
    transaction.execute(
        "INSERT INTO daemon_sync_changes (entity_type, entity_id, revision, operation, entity_json, created_at) VALUES (?1, ?2, ?3, 'upsert', ?4, ?5)",
        params![entity_type, id, revision, serde_json::to_string(&entity)?, now],
    )?;
    transaction.commit()?;
    Ok(entity)
}

fn tombstone_daemon_entity(
    connection: &mut Connection,
    entity_type: &str,
    id: &str,
    expected_revision: Option<i64>,
) -> Result<Value> {
    validate_daemon_entity_identity(entity_type, id)?;
    let transaction = connection.transaction()?;
    let (current_revision, payload_json, created_at): (i64, String, String) = transaction
        .query_row(
            "SELECT revision, payload_json, created_at FROM daemon_entities WHERE entity_type = ?1 AND id = ?2",
            params![entity_type, id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .context("entity does not exist")?;
    if expected_revision.is_some_and(|expected| expected != current_revision) {
        bail!(
            "entity revision conflict: expected {}, actual {current_revision}",
            expected_revision.unwrap_or_default()
        );
    }
    let revision = current_revision + 1;
    let now = Utc::now().to_rfc3339();
    let etag = format!("W/\"{entity_type}:{id}:{revision}\"");
    transaction.execute(
        "UPDATE daemon_entities SET revision = ?3, etag = ?4, tombstone = 1, updated_at = ?5 WHERE entity_type = ?1 AND id = ?2",
        params![entity_type, id, revision, etag, now],
    )?;
    let entity = daemon_entity_json(DaemonEntityRow {
        entity_type,
        id,
        revision,
        etag: &etag,
        payload_json: &payload_json,
        tombstone: true,
        created_at: &created_at,
        updated_at: &now,
    })?;
    transaction.execute(
        "INSERT INTO daemon_sync_changes (entity_type, entity_id, revision, operation, entity_json, created_at) VALUES (?1, ?2, ?3, 'delete', ?4, ?5)",
        params![entity_type, id, revision, serde_json::to_string(&entity)?, now],
    )?;
    transaction.commit()?;
    Ok(entity)
}

fn list_daemon_entities(
    connection: &Connection,
    entity_type: &str,
    include_tombstones: bool,
) -> Result<Vec<Value>> {
    validate_daemon_entity_identity(entity_type, "list")?;
    let mut statement = connection.prepare(
        "SELECT id, revision, etag, payload_json, tombstone, created_at, updated_at FROM daemon_entities WHERE entity_type = ?1 AND (?2 = 1 OR tombstone = 0) ORDER BY updated_at DESC",
    )?;
    let entities = statement
        .query_map(params![entity_type, include_tombstones], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, bool>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?
        .map(|row| {
            let (id, revision, etag, payload, tombstone, created_at, updated_at) = row?;
            daemon_entity_json(DaemonEntityRow {
                entity_type,
                id: &id,
                revision,
                etag: &etag,
                payload_json: &payload,
                tombstone,
                created_at: &created_at,
                updated_at: &updated_at,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(entities)
}

fn search_durable_chats(connection: &Connection, query: &str, limit: usize) -> Result<String> {
    let query_folded = query.to_lowercase();
    let mut statement =
        connection.prepare("SELECT record_json FROM daemon_runs ORDER BY created_at DESC")?;
    let records = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut matches = Vec::new();
    for encoded in records {
        let record: RunRecord = serde_json::from_str(&encoded)?;
        let mut fragments = Vec::new();
        if let Some(prompt) = record.spec.input.get("prompt").and_then(Value::as_str) {
            fragments.push(("user", prompt));
        }
        if let Some(messages) = record.spec.input.get("messages").and_then(Value::as_array) {
            for message in messages {
                if let Some(content) = message.get("content").and_then(Value::as_str) {
                    fragments.push((
                        message
                            .get("role")
                            .and_then(Value::as_str)
                            .unwrap_or("message"),
                        content,
                    ));
                }
            }
        }
        if let Some(content) = record
            .result
            .as_ref()
            .and_then(|result| result.get("content"))
            .and_then(Value::as_str)
        {
            fragments.push(("assistant", content));
        }
        for (role, content) in fragments {
            if content.to_lowercase().contains(&query_folded) {
                matches.push(format!(
                    "[{}] {} ({})\n{}: {}",
                    record.spec.thread_id,
                    record
                        .spec
                        .input
                        .get("client_thread_id")
                        .and_then(Value::as_str)
                        .unwrap_or("Durable local chat"),
                    record.updated_at.to_rfc3339(),
                    role,
                    truncate_chars(content, 2_000),
                ));
                if matches.len() >= limit {
                    return Ok(matches.join("\n\n"));
                }
            }
        }
    }
    if matches.is_empty() {
        Ok(format!("No past-chat matches for \"{query}\"."))
    } else {
        Ok(matches.join("\n\n"))
    }
}

fn render_skill_template(template: &str, name: &str, input: &str) -> String {
    let mut rendered = template.to_owned();
    for placeholder in ["{{input}}", "{{ input }}", "{{ input}}", "{{input }}"] {
        rendered = rendered.replace(placeholder, input);
    }
    for placeholder in [
        "{{skill_name}}",
        "{{ skill_name }}",
        "{{ skill_name}}",
        "{{skill_name }}",
    ] {
        rendered = rendered.replace(placeholder, name);
    }
    rendered
}

#[derive(Debug)]
struct WebSearchResultItem {
    title: String,
    url: String,
    snippet: String,
}

fn extract_html_title(input: &str) -> Option<String> {
    let lower = input.to_lowercase();
    let start = lower.find("<title>")? + "<title>".len();
    let end = lower[start..].find("</title>")? + start;
    Some(decode_html_entities(input[start..end].trim()))
}

fn strip_html_like_content(input: &str) -> String {
    let mut output = String::new();
    let mut inside_tag = false;
    let mut previous_was_space = false;
    for character in input.chars() {
        match character {
            '<' => inside_tag = true,
            '>' => inside_tag = false,
            _ if !inside_tag => {
                let normalized = if character.is_whitespace() {
                    ' '
                } else {
                    character
                };
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
    let href_index = fragment.find("href=\"")? + 6;
    let remainder = &fragment[href_index..];
    let end = remainder.find('"')?;
    Some(decode_html_entities(&remainder[..end]))
}

fn extract_anchor_text(fragment: &str) -> Option<String> {
    let start = fragment.find('>')? + 1;
    let end = fragment[start..].find("</a>")? + start;
    Some(decode_html_entities(
        strip_html_like_content(fragment[start..end].trim()).trim(),
    ))
}

fn parse_duckduckgo_results(body: &str, max_results: usize) -> Vec<WebSearchResultItem> {
    let mut results = Vec::new();
    let mut remainder = body;
    while results.len() < max_results {
        let Some(anchor_position) = remainder.find("result__a") else {
            break;
        };
        remainder = &remainder[anchor_position..];
        let Some(tag_end) = remainder.find("</a>") else {
            break;
        };
        let anchor = &remainder[..tag_end + 4];
        remainder = &remainder[tag_end + 4..];
        let Some(raw_href) = extract_anchor_href(anchor) else {
            continue;
        };
        let candidate_url = if let Some(index) = raw_href.find("uddg=") {
            let candidate = format!("https://duckduckgo.invalid/?{}", &raw_href[index..]);
            url::Url::parse(&candidate).ok().and_then(|parsed| {
                parsed
                    .query_pairs()
                    .find(|(key, _)| key == "uddg")
                    .map(|(_, value)| value.into_owned())
            })
        } else {
            Some(raw_href)
        };
        let Some(url) = candidate_url.filter(|candidate| {
            url::Url::parse(candidate)
                .ok()
                .is_some_and(|parsed| matches!(parsed.scheme(), "http" | "https"))
        }) else {
            continue;
        };
        let title = extract_anchor_text(anchor).unwrap_or_else(|| url.clone());
        let snippet = remainder
            .find("result__snippet")
            .and_then(|index| {
                let snippet = &remainder[index..];
                snippet
                    .find("</a>")
                    .or_else(|| snippet.find("</div>"))
                    .map(|end| {
                        decode_html_entities(strip_html_like_content(&snippet[..end]).trim())
                    })
            })
            .unwrap_or_default();
        results.push(WebSearchResultItem {
            title,
            url,
            snippet,
        });
    }
    results
}

fn safe_workspace_path(root: &Path, relative: &str, must_exist: bool) -> Result<PathBuf> {
    validate_relative_pattern(relative)?;
    let candidate = root.join(relative);
    if must_exist {
        safe_existing_workspace_path(root, &candidate)
    } else {
        let mut ancestor = candidate.parent().context("workspace path has no parent")?;
        while !ancestor.exists() {
            ancestor = ancestor
                .parent()
                .context("workspace path escaped its root")?;
        }
        let canonical_root = root.canonicalize()?;
        if !ancestor.canonicalize()?.starts_with(&canonical_root) {
            bail!("workspace path traverses a symlink outside the project");
        }
        Ok(candidate)
    }
}

fn safe_workspace_path_no_symlinks(
    root: &Path,
    relative: &str,
    must_exist: bool,
) -> Result<PathBuf> {
    validate_relative_pattern(relative)?;
    let mut candidate = root.canonicalize()?;
    for component in Path::new(relative).components() {
        let component = match component {
            std::path::Component::Normal(component) => component,
            std::path::Component::CurDir => continue,
            _ => bail!("workspace path must contain only normal relative components"),
        };
        candidate.push(component);
        match fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                bail!("workspace path must not traverse a symlink")
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !must_exist => {}
            Err(error) => return Err(error.into()),
        }
    }
    if must_exist && !candidate.exists() {
        bail!("workspace path does not exist");
    }
    Ok(candidate)
}

fn reject_symlink(path: &Path) -> Result<()> {
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        bail!("symlinks are not supported by this file operation");
    }
    Ok(())
}

fn copy_workspace_path(source: &Path, destination: &Path) -> Result<()> {
    reject_symlink(source)?;
    if destination.exists() {
        bail!("CopyPath destination already exists");
    }
    let metadata = fs::symlink_metadata(source)?;
    if metadata.is_file() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, destination)?;
        return Ok(());
    }
    if !metadata.is_dir() {
        bail!("CopyPath supports only regular files and directories");
    }
    if destination.starts_with(source) {
        bail!("CopyPath cannot copy a directory into itself");
    }

    let entries = walkdir::WalkDir::new(source)
        .follow_links(false)
        .into_iter()
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for entry in &entries {
        let file_type = entry.file_type();
        if file_type.is_symlink() {
            bail!("CopyPath does not copy directory trees containing symlinks");
        }
        if !file_type.is_file() && !file_type.is_dir() {
            bail!("CopyPath does not copy special files");
        }
    }
    for entry in entries {
        let relative = entry.path().strip_prefix(source)?;
        let target = destination.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn safe_existing_workspace_path(root: &Path, candidate: &Path) -> Result<PathBuf> {
    let canonical_root = root.canonicalize()?;
    let canonical = candidate.canonicalize()?;
    if !canonical.starts_with(&canonical_root) {
        bail!("workspace path traverses a symlink outside the project");
    }
    Ok(canonical)
}

fn relative_path(root: &Path, path: &Path) -> Result<String> {
    Ok(path
        .strip_prefix(root.canonicalize()?)?
        .to_string_lossy()
        .replace('\\', "/"))
}

fn truncate_chars(value: &str, maximum: usize) -> String {
    value.chars().take(maximum).collect()
}

async fn dispatch(daemon: &Daemon, request: IpcRequest) -> IpcResponse {
    if request.token != daemon.config.ipc_token {
        return error_response(request.id, "unauthorized", "invalid local IPC token");
    }
    let result = match request.method.as_str() {
        "health" => Ok(json!({
            "status": "ok",
            "schema_version": SCHEMA_VERSION,
            "user_id": daemon.config.user_id,
            "device_id": daemon.config.device_id,
            "daemon_version": env!("CARGO_PKG_VERSION"),
        })),
        "daemon.shutdown" => {
            let shutdown = daemon.shutdown.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let _ = shutdown.send(true);
            });
            Ok(json!({"accepted": true}))
        }
        "runs.create" => create_run(daemon, request.params).await,
        "server_runs.import" => import_server_run(daemon, request.params).await,
        "server_runs.get" => match run_id_param(&request.params) {
            Ok(id) => get_optional_server_run(daemon, id).await,
            Err(error) => Err(error),
        },
        "server_runs.start" => match run_id_param(&request.params) {
            Ok(id) => start_imported_server_run(daemon, id).await,
            Err(error) => Err(error),
        },
        "runs.list" => list_runs(daemon, request.params).await,
        "runs.list_active" => list_active_runs(daemon).await,
        "runs.get" => match run_id_param(&request.params) {
            Ok(id) => get_run(daemon, id).await,
            Err(error) => Err(error),
        },
        "runs.workspace" => match run_id_param(&request.params) {
            Ok(id) => get_run_workspace(daemon, id).await,
            Err(error) => Err(error),
        },
        "runs.cancel" => match run_id_param(&request.params) {
            Ok(id) => cancel_run(daemon, id).await,
            Err(error) => Err(error),
        },
        "runs.events" => match run_events_params(&request.params) {
            Ok((id, after)) => list_events(daemon, id, after).await,
            Err(error) => Err(error),
        },
        "runs.checkpoints" => match run_id_param(&request.params) {
            Ok(id) => list_checkpoints(daemon, id).await,
            Err(error) => Err(error),
        },
        "runs.approvals" => match run_id_param(&request.params) {
            Ok(id) => list_local_requests(daemon, id, "approval").await,
            Err(error) => Err(error),
        },
        "runs.approvals.resolve" => resolve_local_approval(daemon, request.params).await,
        "runs.input_requests" => match run_id_param(&request.params) {
            Ok(id) => list_local_requests(daemon, id, "input").await,
            Err(error) => Err(error),
        },
        "runs.input_requests.respond" => respond_local_input(daemon, request.params).await,
        "projects.bind_workspace" => bind_project_workspace(daemon, request.params).await,
        "projects.workspace" => get_project_workspace(daemon, request.params).await,
        "provider_bindings.upsert" => upsert_provider_binding(daemon, request.params).await,
        "provider_bindings.get" => get_provider_binding(daemon, request.params).await,
        "provider_bindings.delete" => delete_provider_binding(daemon, request.params).await,
        "mcp_bindings.upsert" => upsert_mcp_binding(daemon, request.params).await,
        "mcp_bindings.get" => get_mcp_binding(daemon, request.params).await,
        "mcp_bindings.delete" => delete_mcp_binding(daemon, request.params).await,
        "schedules.upsert" => upsert_schedule(daemon, request.params).await,
        "schedules.list" => list_schedules(daemon).await,
        "schedules.delete" => delete_schedule(daemon, request.params).await,
        "schedules.run_now" => run_schedule_now(daemon, request.params).await,
        "entities.upsert" => upsert_entity(daemon, request.params).await,
        "entities.list" => list_entities(daemon, request.params).await,
        "entities.delete" => delete_entity(daemon, request.params).await,
        "entities.changes" => list_entity_changes(daemon, request.params).await,
        "sync.state" => sync_ipc::state(daemon, request.params).await,
        "sync.ack_local" => sync_ipc::acknowledge_local(daemon, request.params).await,
        "sync.apply_remote" => sync_ipc::apply_remote(daemon, request.params).await,
        "sync.conflicts" => sync_ipc::list_conflicts(daemon, request.params).await,
        "sync.conflicts.resolve" => sync_ipc::resolve_conflict(daemon, request.params).await,
        _ => Err(anyhow::anyhow!("unknown IPC method {}", request.method)),
    };
    match result {
        Ok(result) => IpcResponse {
            id: request.id,
            result: Some(result),
            error: None,
        },
        Err(error) => error_response(request.id, "request_failed", &error.to_string()),
    }
}

async fn create_run(daemon: &Daemon, params: Value) -> Result<Value> {
    let mut model_config = params
        .get("model_config")
        .cloned()
        .map(serde_json::from_value::<PersistedModelConfig>)
        .transpose()?;
    let request: CreateRunRequest = serde_json::from_value(params)?;
    if request.executor_target
        != (ExecutorTarget::PersonalDevice {
            device_id: daemon.config.device_id,
        })
    {
        bail!("the local daemon only accepts its own personal_device target");
    }
    let mut database = daemon.database.lock().await;
    if request
        .input
        .get("resolve_current_provider_binding")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let profile_id = request
            .input
            .get("client_provider_profile_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("run requires a provider profile ID")?;
        validate_provider_profile_id(profile_id)?;
        let binding =
            load_provider_binding(&database, profile_id, &daemon.config.model_secret_key)?
                .with_context(|| {
                    format!("run is waiting for the per-device provider binding ({profile_id})")
                })?;
        let config = model_config
            .as_mut()
            .context("run requires a model configuration")?;
        config.base_url = binding.base_url;
        config.api_key = binding.api_key;
    }
    if let Some(config) = &model_config {
        config.validate()?;
    }
    let now = Utc::now();
    let spec = RunSpec {
        schema_version: SCHEMA_VERSION,
        id: Uuid::new_v4(),
        thread_id: request.thread_id,
        project_id: request.project_id,
        project: FrozenReference {
            id: request.project_id,
            revision: request.project_revision,
        },
        project_privacy: request.project_privacy,
        task: request.task,
        creator_user_id: daemon.config.user_id,
        executor_target: request.executor_target,
        required_capabilities: request.required_capabilities,
        input: request.input,
        model_profile_id: request.model_profile_id,
        snapshot_id: request.snapshot_id,
        idempotency_key: request.idempotency_key,
        created_at: now,
    };
    if let Some(existing) = database
        .query_row(
            "SELECT record_json FROM daemon_runs WHERE idempotency_key = ?1",
            [&spec.idempotency_key],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        return Ok(serde_json::from_str(&existing)?);
    }
    let record = RunRecord {
        etag: format!("W/\"{}:1\"", spec.id),
        spec,
        state: RunState::Queued,
        revision: 1,
        assigned_executor_id: Some(daemon.config.device_id),
        lease_expires_at: None,
        started_at: None,
        finished_at: None,
        result: None,
        error: None,
        updated_at: now,
    };
    let transaction = database.transaction()?;
    transaction.execute(
        "INSERT INTO daemon_runs (id, thread_id, state, revision, record_json, created_at, updated_at, idempotency_key) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7)",
        params![
            record.spec.id.to_string(),
            record.spec.thread_id.to_string(),
            state_name(record.state),
            record.revision,
            serde_json::to_string(&record)?,
            now.to_rfc3339(),
            record.spec.idempotency_key,
        ],
    )?;
    if let Some(model_config) = model_config {
        let encrypted = encrypt_model_config(&model_config, &daemon.config.model_secret_key)?;
        transaction.execute(
            "INSERT INTO daemon_run_model_configs (run_id, encrypted_config, created_at) VALUES (?1, ?2, ?3)",
            params![
                record.spec.id.to_string(),
                encrypted,
                now.to_rfc3339(),
            ],
        )?;
    }
    append_event(
        &transaction,
        record.spec.id,
        RunEventKind::Created,
        json!({"state": "queued"}),
    )?;
    transaction.commit()?;
    Ok(serde_json::to_value(record)?)
}

async fn import_server_run(daemon: &Daemon, params: Value) -> Result<Value> {
    let request: ImportServerRunRequest = serde_json::from_value(params)?;
    ensure_compatible(request.run_spec.schema_version)?;
    let model_config = request
        .model_config
        .unwrap_or_else(|| daemon.config.fallback_model.clone());
    model_config.validate()?;
    if request.run_spec.executor_target
        != (ExecutorTarget::PersonalDevice {
            device_id: daemon.config.device_id,
        })
    {
        bail!("the imported server run targets a different personal device");
    }
    let workspace = request
        .workspace_path
        .map(|path| {
            let canonical = path
                .canonicalize()
                .context("imported server-run workspace is unavailable")?;
            if !canonical.is_dir() {
                bail!("imported server-run workspace must be a directory");
            }
            Ok(canonical)
        })
        .transpose()?;
    let now = Utc::now();
    let mut database = daemon.database.lock().await;
    if let Some(existing) = database
        .query_row(
            "SELECT record_json FROM daemon_runs WHERE id = ?1",
            [request.run_spec.id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        let existing: RunRecord = serde_json::from_str(&existing)?;
        if existing.spec.idempotency_key != request.run_spec.idempotency_key {
            bail!("imported server run ID already exists with different immutable inputs");
        }
        return Ok(serde_json::to_value(existing)?);
    }
    let initial_state = if request.defer_start {
        RunState::WaitingForExecutor
    } else {
        RunState::Queued
    };
    let record = RunRecord {
        etag: format!("W/\"{}:1\"", request.run_spec.id),
        spec: request.run_spec,
        state: initial_state,
        revision: 1,
        assigned_executor_id: Some(daemon.config.device_id),
        lease_expires_at: None,
        started_at: None,
        finished_at: None,
        result: None,
        error: None,
        updated_at: now,
    };
    let encrypted = encrypt_model_config(&model_config, &daemon.config.model_secret_key)?;
    let transaction = database.transaction()?;
    transaction.execute(
        "INSERT INTO daemon_runs (id, thread_id, state, revision, record_json, created_at, updated_at, idempotency_key) VALUES (?1, ?2, ?3, 1, ?4, ?5, ?5, ?6)",
        params![
            record.spec.id.to_string(),
            record.spec.thread_id.to_string(),
            state_name(record.state),
            serde_json::to_string(&record)?,
            now.to_rfc3339(),
            record.spec.idempotency_key,
        ],
    )?;
    transaction.execute(
        "INSERT INTO daemon_run_model_configs (run_id, encrypted_config, created_at) VALUES (?1, ?2, ?3)",
        params![record.spec.id.to_string(), encrypted, now.to_rfc3339()],
    )?;
    if let Some(workspace) = workspace {
        transaction.execute(
            "INSERT INTO daemon_run_workspaces (run_id, workspace_path, created_at) VALUES (?1, ?2, ?3)",
            params![
                record.spec.id.to_string(),
                workspace.to_string_lossy(),
                now.to_rfc3339()
            ],
        )?;
    }
    append_event(
        &transaction,
        record.spec.id,
        RunEventKind::Created,
        json!({"state": state_name(record.state), "imported_server_run": true}),
    )?;
    transaction.commit()?;
    Ok(serde_json::to_value(record)?)
}

async fn start_imported_server_run(daemon: &Daemon, run_id: Uuid) -> Result<Value> {
    let database = daemon.database.lock().await;
    let persisted: String = database
        .query_row(
            "SELECT record_json FROM daemon_runs WHERE id = ?1",
            [run_id.to_string()],
            |row| row.get(0),
        )
        .context("imported server run was not found")?;
    let mut record: RunRecord = serde_json::from_str(&persisted)?;
    if record.spec.executor_target
        != (ExecutorTarget::PersonalDevice {
            device_id: daemon.config.device_id,
        })
    {
        bail!("the imported server run targets a different personal device");
    }
    if record.state == RunState::Queued || record.state == RunState::Running {
        return Ok(serde_json::to_value(record)?);
    }
    if record.state != RunState::WaitingForExecutor {
        bail!("only a deferred imported server run can be started");
    }
    record.state = RunState::Queued;
    record.revision += 1;
    record.updated_at = Utc::now();
    refresh_etag(&mut record);
    let transaction = database.unchecked_transaction()?;
    transaction.execute(
        "UPDATE daemon_runs SET state = 'queued', revision = ?2, record_json = ?3, updated_at = ?4 WHERE id = ?1",
        params![
            run_id.to_string(),
            record.revision,
            serde_json::to_string(&record)?,
            record.updated_at.to_rfc3339(),
        ],
    )?;
    append_event(
        &transaction,
        run_id,
        RunEventKind::StateChanged,
        json!({"from": "waiting_for_executor", "to": "queued"}),
    )?;
    transaction.commit()?;
    Ok(serde_json::to_value(record)?)
}

async fn get_optional_server_run(daemon: &Daemon, run_id: Uuid) -> Result<Value> {
    let database = daemon.database.lock().await;
    let persisted = database
        .query_row(
            "SELECT record_json FROM daemon_runs WHERE id = ?1",
            [run_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let run = persisted
        .map(|record| serde_json::from_str::<RunRecord>(&record))
        .transpose()?;
    Ok(json!({"run": run}))
}

async fn list_runs(daemon: &Daemon, params: Value) -> Result<Value> {
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(100)
        .clamp(1, 200) as i64;
    let database = daemon.database.lock().await;
    let mut statement = database
        .prepare("SELECT record_json FROM daemon_runs ORDER BY created_at DESC LIMIT ?1")?;
    let items = statement
        .query_map([limit], |row| row.get::<_, String>(0))?
        .map(|row| {
            row.map_err(Into::into)
                .and_then(|value| serde_json::from_str::<RunRecord>(&value).map_err(Into::into))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(serde_json::to_value(ListRunsResponse {
        items,
        next_cursor: None,
    })?)
}

async fn list_active_runs(daemon: &Daemon) -> Result<Value> {
    let database = daemon.database.lock().await;
    let mut statement = database.prepare(
        "SELECT record_json FROM daemon_runs WHERE state NOT IN ('completed', 'failed', 'canceled', 'expired') ORDER BY created_at ASC",
    )?;
    let items = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .map(|row| {
            row.map_err(Into::into)
                .and_then(|value| serde_json::from_str::<RunRecord>(&value).map_err(Into::into))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(serde_json::to_value(ListRunsResponse {
        items,
        next_cursor: None,
    })?)
}

async fn upsert_entity(daemon: &Daemon, params: Value) -> Result<Value> {
    let entity_type = params
        .get("entity_type")
        .and_then(Value::as_str)
        .context("entity_type is required")?;
    let id = params
        .get("id")
        .and_then(Value::as_str)
        .context("id is required")?;
    let payload = params
        .get("payload")
        .cloned()
        .context("payload is required")?;
    let expected_revision = params.get("expected_revision").and_then(Value::as_i64);
    let mut database = daemon.database.lock().await;
    write_daemon_entity(&mut database, entity_type, id, payload, expected_revision)
}

async fn list_entities(daemon: &Daemon, params: Value) -> Result<Value> {
    let entity_type = params
        .get("entity_type")
        .and_then(Value::as_str)
        .context("entity_type is required")?;
    let include_tombstones = params
        .get("include_tombstones")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let database = daemon.database.lock().await;
    Ok(Value::Array(list_daemon_entities(
        &database,
        entity_type,
        include_tombstones,
    )?))
}

async fn delete_entity(daemon: &Daemon, params: Value) -> Result<Value> {
    let entity_type = params
        .get("entity_type")
        .and_then(Value::as_str)
        .context("entity_type is required")?;
    let id = params
        .get("id")
        .and_then(Value::as_str)
        .context("id is required")?;
    let expected_revision = params.get("expected_revision").and_then(Value::as_i64);
    let mut database = daemon.database.lock().await;
    tombstone_daemon_entity(&mut database, entity_type, id, expected_revision)
}

async fn list_entity_changes(daemon: &Daemon, params: Value) -> Result<Value> {
    let after = params
        .get("after")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .max(0);
    let limit = params
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(200)
        .clamp(1, 1000);
    let database = daemon.database.lock().await;
    let mut statement = database.prepare(
        "SELECT cursor, entity_type, entity_id, revision, operation, entity_json, created_at FROM daemon_sync_changes WHERE cursor > ?1 ORDER BY cursor ASC LIMIT ?2",
    )?;
    let changes = statement
        .query_map(params![after, limit], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?
        .map(|row| {
            let (cursor, entity_type, entity_id, revision, operation, entity, created_at) = row?;
            Ok(json!({
                "cursor": cursor,
                "entity_type": entity_type,
                "entity_id": entity_id,
                "revision": revision,
                "operation": operation,
                "entity": serde_json::from_str::<Value>(&entity)?,
                "created_at": created_at,
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    let next_cursor = changes
        .last()
        .and_then(|change| change["cursor"].as_i64())
        .unwrap_or(after);
    Ok(json!({"changes": changes, "next_cursor": next_cursor}))
}

fn validate_provider_profile_id(profile_id: &str) -> Result<()> {
    if profile_id.trim().is_empty()
        || profile_id.len() > 512
        || profile_id.chars().any(char::is_control)
    {
        bail!("provider profile ID is missing or invalid");
    }
    Ok(())
}

fn validate_provider_binding(binding: &ProviderDeviceBinding) -> Result<()> {
    let endpoint = reqwest::Url::parse(&binding.base_url).context("invalid provider base URL")?;
    if !matches!(endpoint.scheme(), "http" | "https") {
        bail!("provider base URL must use HTTP or HTTPS");
    }
    if binding
        .api_key
        .as_ref()
        .is_some_and(|secret| secret.len() > 64 * 1024)
    {
        bail!("provider API key is too long");
    }
    Ok(())
}

fn load_provider_binding(
    connection: &Connection,
    profile_id: &str,
    secret_key: &[u8; 32],
) -> Result<Option<ProviderDeviceBinding>> {
    let encrypted = connection
        .query_row(
            "SELECT encrypted_binding FROM daemon_provider_bindings WHERE profile_id = ?1",
            [profile_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    encrypted
        .map(|payload| {
            let plaintext = decrypt_secret_payload(&payload, secret_key)?;
            let binding: ProviderDeviceBinding = serde_json::from_slice(&plaintext)?;
            validate_provider_binding(&binding)?;
            Ok(binding)
        })
        .transpose()
}

async fn upsert_provider_binding(daemon: &Daemon, params: Value) -> Result<Value> {
    let request: ProviderBindingUpsertRequest = serde_json::from_value(params)?;
    validate_provider_profile_id(&request.profile_id)?;
    let binding = ProviderDeviceBinding {
        base_url: request.base_url,
        api_key: request.api_key,
    };
    validate_provider_binding(&binding)?;
    let encrypted = encrypt_secret_payload(
        &serde_json::to_vec(&binding)?,
        &daemon.config.model_secret_key,
    )?;
    let updated_at = Utc::now().to_rfc3339();
    let database = daemon.database.lock().await;
    database.execute(
        r#"
        INSERT INTO daemon_provider_bindings (profile_id, encrypted_binding, updated_at)
        VALUES (?1, ?2, ?3)
        ON CONFLICT(profile_id) DO UPDATE SET
            encrypted_binding = excluded.encrypted_binding,
            updated_at = excluded.updated_at
        "#,
        params![request.profile_id, encrypted, updated_at],
    )?;
    Ok(json!({
        "profile_id": request.profile_id,
        "bound": true,
        "base_url": binding.base_url,
        "has_api_key": binding.api_key.is_some(),
        "updated_at": updated_at,
    }))
}

async fn get_provider_binding(daemon: &Daemon, params: Value) -> Result<Value> {
    let profile_id = params
        .get("profile_id")
        .and_then(Value::as_str)
        .context("profile_id is required")?;
    validate_provider_profile_id(profile_id)?;
    let database = daemon.database.lock().await;
    let record = database
        .query_row(
            "SELECT encrypted_binding, updated_at FROM daemon_provider_bindings WHERE profile_id = ?1",
            [profile_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((encrypted, updated_at)) = record else {
        return Ok(json!({"profile_id":profile_id,"bound":false}));
    };
    let plaintext = decrypt_secret_payload(&encrypted, &daemon.config.model_secret_key)?;
    let binding: ProviderDeviceBinding = serde_json::from_slice(&plaintext)?;
    validate_provider_binding(&binding)?;
    Ok(json!({
        "profile_id": profile_id,
        "bound": true,
        "base_url": binding.base_url,
        "has_api_key": binding.api_key.is_some(),
        "updated_at": updated_at,
    }))
}

async fn delete_provider_binding(daemon: &Daemon, params: Value) -> Result<Value> {
    let profile_id = params
        .get("profile_id")
        .and_then(Value::as_str)
        .context("profile_id is required")?;
    validate_provider_profile_id(profile_id)?;
    let database = daemon.database.lock().await;
    let deleted = database.execute(
        "DELETE FROM daemon_provider_bindings WHERE profile_id = ?1",
        [profile_id],
    )?;
    Ok(json!({"profile_id":profile_id,"deleted":deleted > 0}))
}

fn validate_mcp_server_id(server_id: &str) -> Result<()> {
    if server_id.trim().is_empty()
        || server_id.len() > 512
        || server_id.chars().any(char::is_control)
    {
        bail!("MCP server ID is missing or invalid");
    }
    Ok(())
}

fn validate_mcp_binding(binding: &PersistedMcpServer) -> Result<()> {
    if binding.name.trim().is_empty() || binding.name.len() > 256 {
        bail!("MCP server name is missing or too long");
    }
    if binding.command.trim().is_empty() || binding.command.len() > 32 * 1024 {
        bail!("MCP server command is missing or too long");
    }
    if binding.args.len() > 256
        || binding
            .args
            .iter()
            .any(|argument| argument.len() > 64 * 1024)
        || binding.env.len() > 256
        || binding.env.iter().any(|(key, value)| {
            key.is_empty()
                || key.len() > 4096
                || key
                    .chars()
                    .any(|character| character == '=' || character == '\0')
                || value.len() > 64 * 1024
                || value.contains('\0')
        })
    {
        bail!("MCP server configuration exceeds the local safety limits");
    }
    Ok(())
}

fn load_mcp_binding(
    connection: &Connection,
    server_id: &str,
    secret_key: &[u8; 32],
) -> Result<Option<PersistedMcpServer>> {
    let encrypted = connection
        .query_row(
            "SELECT encrypted_binding FROM daemon_mcp_bindings WHERE server_id = ?1",
            [server_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    encrypted
        .map(|payload| {
            let plaintext = decrypt_secret_payload(&payload, secret_key)?;
            let binding: PersistedMcpServer = serde_json::from_slice(&plaintext)?;
            validate_mcp_binding(&binding)?;
            Ok(binding)
        })
        .transpose()
}

fn mcp_binding_metadata(server_id: &str, binding: &PersistedMcpServer, updated_at: &str) -> Value {
    let mut environment_keys = binding.env.keys().cloned().collect::<Vec<_>>();
    environment_keys.sort();
    let executable_hint = binding
        .command
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default();
    json!({
        "server_id": server_id,
        "bound": true,
        "name": binding.name,
        "executable_hint": executable_hint,
        "argument_count": binding.args.len(),
        "environment_keys": environment_keys,
        "updated_at": updated_at,
    })
}

async fn upsert_mcp_binding(daemon: &Daemon, params: Value) -> Result<Value> {
    let request: McpBindingUpsertRequest = serde_json::from_value(params)?;
    validate_mcp_server_id(&request.server_id)?;
    let binding = PersistedMcpServer {
        name: request.name,
        command: request.command,
        args: request.args,
        env: request.env,
    };
    validate_mcp_binding(&binding)?;
    let encrypted = encrypt_secret_payload(
        &serde_json::to_vec(&binding)?,
        &daemon.config.model_secret_key,
    )?;
    let updated_at = Utc::now().to_rfc3339();
    let database = daemon.database.lock().await;
    database.execute(
        r#"
        INSERT INTO daemon_mcp_bindings (server_id, encrypted_binding, updated_at)
        VALUES (?1, ?2, ?3)
        ON CONFLICT(server_id) DO UPDATE SET
            encrypted_binding = excluded.encrypted_binding,
            updated_at = excluded.updated_at
        "#,
        params![request.server_id, encrypted, updated_at],
    )?;
    Ok(mcp_binding_metadata(
        &request.server_id,
        &binding,
        &updated_at,
    ))
}

async fn get_mcp_binding(daemon: &Daemon, params: Value) -> Result<Value> {
    let server_id = params
        .get("server_id")
        .and_then(Value::as_str)
        .context("server_id is required")?;
    validate_mcp_server_id(server_id)?;
    let database = daemon.database.lock().await;
    let record = database
        .query_row(
            "SELECT encrypted_binding, updated_at FROM daemon_mcp_bindings WHERE server_id = ?1",
            [server_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((encrypted, updated_at)) = record else {
        return Ok(json!({"server_id":server_id,"bound":false}));
    };
    let plaintext = decrypt_secret_payload(&encrypted, &daemon.config.model_secret_key)?;
    let binding: PersistedMcpServer = serde_json::from_slice(&plaintext)?;
    validate_mcp_binding(&binding)?;
    Ok(mcp_binding_metadata(server_id, &binding, &updated_at))
}

async fn delete_mcp_binding(daemon: &Daemon, params: Value) -> Result<Value> {
    let server_id = params
        .get("server_id")
        .and_then(Value::as_str)
        .context("server_id is required")?;
    validate_mcp_server_id(server_id)?;
    let database = daemon.database.lock().await;
    let deleted = database.execute(
        "DELETE FROM daemon_mcp_bindings WHERE server_id = ?1",
        [server_id],
    )?;
    Ok(json!({"server_id":server_id,"deleted":deleted > 0}))
}

async fn bind_project_workspace(daemon: &Daemon, params: Value) -> Result<Value> {
    let project_id: Uuid = params
        .get("project_id")
        .and_then(Value::as_str)
        .context("project_id is required")?
        .parse()
        .context("invalid project_id")?;
    let requested = PathBuf::from(
        params
            .get("workspace_path")
            .and_then(Value::as_str)
            .context("workspace_path is required")?,
    );
    let workspace = requested
        .canonicalize()
        .context("workspace_path does not exist")?;
    if !workspace.is_dir() {
        bail!("workspace_path must be a directory");
    }
    let database = daemon.database.lock().await;
    database.execute(
        "INSERT INTO daemon_project_bindings (project_id, workspace_path, updated_at) VALUES (?1, ?2, ?3) ON CONFLICT(project_id) DO UPDATE SET workspace_path = excluded.workspace_path, updated_at = excluded.updated_at",
        params![project_id.to_string(), workspace.to_string_lossy(), Utc::now().to_rfc3339()],
    )?;
    Ok(json!({"project_id": project_id, "bound": true}))
}

async fn get_project_workspace(daemon: &Daemon, params: Value) -> Result<Value> {
    let project_id: Uuid = params
        .get("project_id")
        .and_then(Value::as_str)
        .context("project_id is required")?
        .parse()
        .context("invalid project_id")?;
    let database = daemon.database.lock().await;
    let bound = database
        .query_row(
            "SELECT workspace_path, updated_at FROM daemon_project_bindings WHERE project_id = ?1",
            [project_id.to_string()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    Ok(match bound {
        Some((path, updated_at)) => {
            json!({"project_id": project_id, "bound": true, "available": Path::new(&path).is_dir(), "updated_at": updated_at})
        }
        None => json!({"project_id": project_id, "bound": false, "available": false}),
    })
}

async fn upsert_schedule(daemon: &Daemon, params: Value) -> Result<Value> {
    let request: ScheduleUpsertRequest = serde_json::from_value(params)?;
    if request.run_request.executor_target
        != (ExecutorTarget::PersonalDevice {
            device_id: daemon.config.device_id,
        })
    {
        bail!("the local daemon only schedules its own personal_device target");
    }
    request.model_config.validate()?;
    let now = Utc::now();
    let next_run_at = request
        .enabled
        .then(|| next_schedule_at(&request.expression, &request.timezone, now))
        .transpose()?;
    let template = PersistedScheduleTemplate {
        request: request.run_request,
        model_config: request.model_config,
    };
    let encrypted_template = encrypt_schedule_template(&template, &daemon.config.model_secret_key)?;
    let database = daemon.database.lock().await;
    database.execute(
        r#"
        INSERT INTO daemon_schedules (
            id, expression, timezone, enabled, encrypted_template,
            next_run_at, last_triggered_at, last_error, created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, ?7, ?7)
        ON CONFLICT(id) DO UPDATE SET
            expression = excluded.expression,
            timezone = excluded.timezone,
            enabled = excluded.enabled,
            encrypted_template = excluded.encrypted_template,
            next_run_at = excluded.next_run_at,
            last_error = NULL,
            updated_at = excluded.updated_at
        "#,
        params![
            request.id.to_string(),
            request.expression,
            request.timezone,
            request.enabled,
            encrypted_template,
            next_run_at.map(|value| value.to_rfc3339()),
            now.to_rfc3339(),
        ],
    )?;
    schedule_record(&database, request.id)
}

async fn list_schedules(daemon: &Daemon) -> Result<Value> {
    let database = daemon.database.lock().await;
    let mut statement = database.prepare(
        "SELECT id, expression, timezone, enabled, next_run_at, last_triggered_at, last_error, created_at, updated_at FROM daemon_schedules ORDER BY created_at DESC",
    )?;
    let rows = statement
        .query_map([], schedule_row_json)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(Value::Array(rows))
}

async fn delete_schedule(daemon: &Daemon, params: Value) -> Result<Value> {
    let schedule_id: Uuid = params
        .get("schedule_id")
        .and_then(Value::as_str)
        .context("schedule_id is required")?
        .parse()
        .context("invalid schedule_id")?;
    let database = daemon.database.lock().await;
    let deleted = database.execute(
        "DELETE FROM daemon_schedules WHERE id = ?1",
        [schedule_id.to_string()],
    )?;
    Ok(json!({"schedule_id": schedule_id, "deleted": deleted == 1}))
}

async fn run_schedule_now(daemon: &Daemon, params: Value) -> Result<Value> {
    let schedule_id: Uuid = params
        .get("schedule_id")
        .and_then(Value::as_str)
        .context("schedule_id is required")?
        .parse()
        .context("invalid schedule_id")?;
    let schedule = {
        let database = daemon.database.lock().await;
        database
            .query_row(
                "SELECT expression, timezone, encrypted_template FROM daemon_schedules WHERE id = ?1",
                [schedule_id.to_string()],
                |row| {
                    Ok(DueSchedule {
                        id: schedule_id,
                        expression: row.get(0)?,
                        timezone: row.get(1)?,
                        due_at: Utc::now(),
                        encrypted_template: row.get(2)?,
                    })
                },
            )
            .optional()?
            .context("local schedule was not found")?
    };
    let run = trigger_schedule(daemon, &schedule).await?;
    Ok(serde_json::to_value(run)?)
}

fn schedule_record(database: &Connection, schedule_id: Uuid) -> Result<Value> {
    database
        .query_row(
            "SELECT id, expression, timezone, enabled, next_run_at, last_triggered_at, last_error, created_at, updated_at FROM daemon_schedules WHERE id = ?1",
            [schedule_id.to_string()],
            schedule_row_json,
        )
        .map_err(Into::into)
}

fn schedule_row_json(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": row.get::<_, String>(0)?,
        "expression": row.get::<_, String>(1)?,
        "timezone": row.get::<_, String>(2)?,
        "enabled": row.get::<_, bool>(3)?,
        "next_run_at": row.get::<_, Option<String>>(4)?,
        "last_triggered_at": row.get::<_, Option<String>>(5)?,
        "last_error": row.get::<_, Option<String>>(6)?,
        "created_at": row.get::<_, String>(7)?,
        "updated_at": row.get::<_, String>(8)?,
    }))
}

async fn list_checkpoints(daemon: &Daemon, run_id: Uuid) -> Result<Value> {
    let database = daemon.database.lock().await;
    let mut statement = database.prepare(
        "SELECT id, sequence, safe_to_resume, executor_state, created_at FROM daemon_run_checkpoints WHERE run_id = ?1 ORDER BY sequence",
    )?;
    let rows = statement
        .query_map([run_id.to_string()], |row| {
            Ok(json!({
                "schema_version": SCHEMA_VERSION,
                "id": row.get::<_, String>(0)?,
                "run_id": run_id,
                "sequence": row.get::<_, i64>(1)?,
                "safe_to_resume": row.get::<_, bool>(2)?,
                "executor_state": serde_json::from_str::<Value>(&row.get::<_, String>(3)?).unwrap_or(Value::Null),
                "created_at": row.get::<_, String>(4)?,
            }))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(Value::Array(rows))
}

async fn list_local_requests(daemon: &Daemon, run_id: Uuid, kind: &str) -> Result<Value> {
    let table = match kind {
        "approval" => "daemon_approval_requests",
        "input" => "daemon_input_requests",
        _ => bail!("invalid local request kind"),
    };
    let database = daemon.database.lock().await;
    let query = format!(
        "SELECT id, state, request_json, response_json, expires_at, created_at, resolved_at FROM {table} WHERE run_id = ?1 ORDER BY created_at"
    );
    let mut statement = database.prepare(&query)?;
    let rows = statement
        .query_map([run_id.to_string()], |row| {
            let request: String = row.get(2)?;
            let response: Option<String> = row.get(3)?;
            Ok(json!({
                "schema_version": SCHEMA_VERSION,
                "id": row.get::<_, String>(0)?,
                "run_id": run_id,
                "state": row.get::<_, String>(1)?,
                "request": serde_json::from_str::<Value>(&request).unwrap_or(Value::Null),
                "response": response.and_then(|value| serde_json::from_str::<Value>(&value).ok()),
                "expires_at": row.get::<_, String>(4)?,
                "created_at": row.get::<_, String>(5)?,
                "resolved_at": row.get::<_, Option<String>>(6)?,
            }))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(Value::Array(rows))
}

async fn resolve_local_approval(daemon: &Daemon, params: Value) -> Result<Value> {
    let run_id = run_id_param(&params)?;
    let approval_id: Uuid = params
        .get("approval_id")
        .and_then(Value::as_str)
        .context("approval_id is required")?
        .parse()
        .context("invalid approval_id")?;
    let decision = params
        .get("decision")
        .and_then(Value::as_str)
        .context("decision is required")?;
    let state = match decision {
        "approve" | "approved" => "approved",
        "reject" | "rejected" => "rejected",
        _ => bail!("decision must be approve or reject"),
    };
    let response = params.get("response").cloned().unwrap_or(Value::Null);
    let database = daemon.database.lock().await;
    let changed = database.execute(
        "UPDATE daemon_approval_requests SET state = ?3, response_json = ?4, resolved_at = ?5 WHERE id = ?1 AND run_id = ?2 AND state = 'pending'",
        params![approval_id.to_string(), run_id.to_string(), state, serde_json::to_string(&response)?, Utc::now().to_rfc3339()],
    )?;
    if changed != 1 {
        bail!("pending approval was not found");
    }
    append_event(
        &database,
        run_id,
        RunEventKind::ApprovalResolved,
        json!({"id": approval_id, "state": state}),
    )?;
    Ok(json!({"id": approval_id, "state": state}))
}

async fn respond_local_input(daemon: &Daemon, params: Value) -> Result<Value> {
    let run_id = run_id_param(&params)?;
    let input_id: Uuid = params
        .get("input_id")
        .and_then(Value::as_str)
        .context("input_id is required")?
        .parse()
        .context("invalid input_id")?;
    let response = params
        .get("response")
        .cloned()
        .context("response is required")?;
    let database = daemon.database.lock().await;
    let changed = database.execute(
        "UPDATE daemon_input_requests SET state = 'answered', response_json = ?3, resolved_at = ?4 WHERE id = ?1 AND run_id = ?2 AND state = 'pending'",
        params![input_id.to_string(), run_id.to_string(), serde_json::to_string(&response)?, Utc::now().to_rfc3339()],
    )?;
    if changed != 1 {
        bail!("pending input request was not found");
    }
    append_event(
        &database,
        run_id,
        RunEventKind::InputReceived,
        json!({"id": input_id}),
    )?;
    Ok(json!({"id": input_id, "state": "answered"}))
}

async fn get_run(daemon: &Daemon, run_id: Uuid) -> Result<Value> {
    let database = daemon.database.lock().await;
    let encoded = database
        .query_row(
            "SELECT record_json FROM daemon_runs WHERE id = ?1",
            [run_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .with_context(|| format!("run {run_id} was not found"))?;
    Ok(serde_json::from_str(&encoded)?)
}

async fn get_run_workspace(daemon: &Daemon, run_id: Uuid) -> Result<Value> {
    let project_id = {
        let database = daemon.database.lock().await;
        let encoded = database
            .query_row(
                "SELECT record_json FROM daemon_runs WHERE id = ?1",
                [run_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .with_context(|| format!("run {run_id} was not found"))?;
        serde_json::from_str::<RunRecord>(&encoded)?.spec.project_id
    };
    let workspace = match run_workspace(daemon, run_id).await? {
        Some(workspace) => Some(workspace),
        None => project_workspace(daemon, project_id).await?,
    };
    Ok(json!({
        "run_id": run_id,
        "workspace_path": workspace.map(|path| path.to_string_lossy().into_owned()),
    }))
}

async fn cancel_run(daemon: &Daemon, run_id: Uuid) -> Result<Value> {
    let database = daemon.database.lock().await;
    let encoded = database
        .query_row(
            "SELECT record_json FROM daemon_runs WHERE id = ?1",
            [run_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .with_context(|| format!("run {run_id} was not found"))?;
    let mut record: RunRecord = serde_json::from_str(&encoded)?;
    if record.state.is_terminal() {
        return Ok(serde_json::to_value(record)?);
    }
    let previous = record.state;
    record.state = RunState::Canceled;
    record.revision += 1;
    refresh_etag(&mut record);
    record.updated_at = Utc::now();
    record.finished_at = Some(Utc::now());
    save_record(&database, &record)?;
    append_event(
        &database,
        run_id,
        RunEventKind::StateChanged,
        json!({"from": state_name(previous), "to": "canceled"}),
    )?;
    database.execute(
        "DELETE FROM daemon_run_model_configs WHERE run_id = ?1",
        [run_id.to_string()],
    )?;
    database.execute(
        "DELETE FROM daemon_run_workspaces WHERE run_id = ?1",
        [run_id.to_string()],
    )?;
    Ok(serde_json::to_value(record)?)
}

async fn list_events(daemon: &Daemon, run_id: Uuid, after: i64) -> Result<Value> {
    let database = daemon.database.lock().await;
    let mut statement = database.prepare("SELECT event_json FROM daemon_run_events WHERE run_id = ?1 AND sequence > ?2 ORDER BY sequence LIMIT 1000")?;
    let events = statement
        .query_map(params![run_id.to_string(), after], |row| {
            row.get::<_, String>(0)
        })?
        .map(|row| {
            row.map_err(Into::into)
                .and_then(|value| serde_json::from_str::<RunEvent>(&value).map_err(Into::into))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(serde_json::to_value(events)?)
}

fn save_record(database: &Connection, record: &RunRecord) -> Result<()> {
    database.execute(
        "UPDATE daemon_runs SET state = ?2, revision = ?3, record_json = ?4, updated_at = ?5 WHERE id = ?1",
        params![record.spec.id.to_string(), state_name(record.state), record.revision, serde_json::to_string(record)?, record.updated_at.to_rfc3339()],
    )?;
    Ok(())
}

fn refresh_etag(record: &mut RunRecord) {
    record.etag = format!("W/\"{}:{}\"", record.spec.id, record.revision);
}

fn append_event(
    database: &Connection,
    run_id: Uuid,
    kind: RunEventKind,
    payload: Value,
) -> Result<()> {
    let sequence: i64 = database.query_row(
        "SELECT COALESCE(MAX(sequence), 0) + 1 FROM daemon_run_events WHERE run_id = ?1",
        [run_id.to_string()],
        |row| row.get(0),
    )?;
    let event = RunEvent {
        schema_version: SCHEMA_VERSION,
        run_id,
        sequence,
        event_id: Uuid::new_v4(),
        kind,
        payload,
        created_at: Utc::now(),
    };
    database.execute(
        "INSERT INTO daemon_run_events (run_id, sequence, event_json) VALUES (?1, ?2, ?3)",
        params![run_id.to_string(), sequence, serde_json::to_string(&event)?],
    )?;
    Ok(())
}

fn run_id_param(params: &Value) -> Result<Uuid> {
    params
        .get("run_id")
        .and_then(Value::as_str)
        .context("run_id is required")?
        .parse()
        .context("invalid run_id")
}

fn run_events_params(params: &Value) -> Result<(Uuid, i64)> {
    Ok((
        run_id_param(params)?,
        params.get("after").and_then(Value::as_i64).unwrap_or(0),
    ))
}

fn error_response(id: Value, code: &'static str, message: &str) -> IpcResponse {
    IpcResponse {
        id,
        result: None,
        error: Some(IpcError {
            code,
            message: message.to_owned(),
        }),
    }
}

async fn handle_connection<T>(daemon: Daemon, stream: T) -> Result<()>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        let response = if line.len() > MAX_IPC_MESSAGE_BYTES {
            error_response(
                Value::Null,
                "message_too_large",
                "IPC message exceeds one MiB",
            )
        } else {
            match serde_json::from_str::<IpcRequest>(&line) {
                Ok(request) => dispatch(&daemon, request).await,
                Err(error) => error_response(Value::Null, "invalid_request", &error.to_string()),
            }
        };
        writer.write_all(&serde_json::to_vec(&response)?).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }
    Ok(())
}

#[cfg(unix)]
async fn serve_ipc(daemon: Daemon) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use tokio::net::UnixListener;

    let path = PathBuf::from(&daemon.config.ipc_endpoint);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    if path.exists() {
        fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    tracing::info!(endpoint = %path.display(), "local daemon IPC ready");
    loop {
        let (stream, _) = listener.accept().await?;
        let daemon = daemon.clone();
        tokio::spawn(async move {
            if let Err(error) = handle_connection(daemon, stream).await {
                tracing::warn!(?error, "local IPC connection failed");
            }
        });
    }
}

#[cfg(windows)]
async fn serve_ipc(daemon: Daemon) -> Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;

    let mut first = true;
    tracing::info!(endpoint = %daemon.config.ipc_endpoint, "local daemon IPC ready");
    loop {
        let mut options = ServerOptions::new();
        options
            .first_pipe_instance(first)
            .reject_remote_clients(true);
        let server = options.create(&daemon.config.ipc_endpoint)?;
        first = false;
        server.connect().await?;
        let daemon = daemon.clone();
        tokio::spawn(async move {
            if let Err(error) = handle_connection(daemon, server).await {
                tracing::warn!(?error, "local IPC connection failed");
            }
        });
    }
}

fn state_name(state: RunState) -> &'static str {
    match state {
        RunState::Queued => "queued",
        RunState::WaitingForExecutor => "waiting_for_executor",
        RunState::WaitingForSnapshot => "waiting_for_snapshot",
        RunState::Running => "running",
        RunState::WaitingApproval => "waiting_approval",
        RunState::WaitingInput => "waiting_input",
        RunState::Interrupted => "interrupted",
        RunState::Completed => "completed",
        RunState::Failed => "failed",
        RunState::Canceled => "canceled",
        RunState::Expired => "expired",
    }
}

fn required(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("missing required environment variable {name}"))
}

fn secret(name: &str) -> Result<String> {
    if let Ok(value) = env::var(name) {
        if !value.trim().is_empty() {
            return Ok(value);
        }
    }
    let file_var = format!("{name}_FILE");
    let path = required(&file_var)?;
    Ok(fs::read_to_string(&path)?.trim().to_owned())
}

fn secret_or_create(name: &str, default_path: &Path) -> Result<String> {
    if env::var(name).is_ok() || env::var(format!("{name}_FILE")).is_ok() {
        return secret(name);
    }
    load_or_create_private(default_path, || {
        format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
    })
}

fn model_secret_key(data_dir: &Path) -> Result<[u8; 32]> {
    let secret = secret_or_create(
        "COWORK_DAEMON_MODEL_SECRET_KEY",
        &data_dir.join("model-secret-key.txt"),
    )?;
    if secret.len() < 32 {
        bail!("COWORK_DAEMON_MODEL_SECRET_KEY must contain at least 32 characters");
    }
    Ok(Sha256::digest(secret.as_bytes()).into())
}

fn encrypt_model_config(config: &PersistedModelConfig, key: &[u8; 32]) -> Result<String> {
    let cipher = Aes256Gcm::new_from_slice(key).expect("AES-256 key has fixed length");
    let mut nonce_bytes = [0_u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let plaintext = serde_json::to_vec(config)?;
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), plaintext.as_ref())
        .map_err(|_| anyhow::anyhow!("failed to encrypt run model configuration"))?;
    let mut envelope = Vec::with_capacity(nonce_bytes.len() + ciphertext.len());
    envelope.extend_from_slice(&nonce_bytes);
    envelope.extend_from_slice(&ciphertext);
    Ok(BASE64.encode(envelope))
}

fn decrypt_model_config(encoded: &str, key: &[u8; 32]) -> Result<PersistedModelConfig> {
    let envelope = BASE64
        .decode(encoded)
        .context("invalid encrypted run model configuration")?;
    if envelope.len() <= 12 {
        bail!("encrypted run model configuration is truncated");
    }
    let cipher = Aes256Gcm::new_from_slice(key).expect("AES-256 key has fixed length");
    let plaintext = cipher
        .decrypt(Nonce::from_slice(&envelope[..12]), &envelope[12..])
        .map_err(|_| anyhow::anyhow!("failed to decrypt run model configuration"))?;
    let config: PersistedModelConfig = serde_json::from_slice(&plaintext)?;
    config.validate()?;
    Ok(config)
}

fn encrypt_schedule_template(
    template: &PersistedScheduleTemplate,
    key: &[u8; 32],
) -> Result<String> {
    encrypt_secret_payload(&serde_json::to_vec(template)?, key)
}

fn decrypt_schedule_template(encoded: &str, key: &[u8; 32]) -> Result<PersistedScheduleTemplate> {
    let plaintext = decrypt_secret_payload(encoded, key)?;
    let template: PersistedScheduleTemplate = serde_json::from_slice(&plaintext)?;
    template.model_config.validate()?;
    Ok(template)
}

fn encrypt_secret_payload(plaintext: &[u8], key: &[u8; 32]) -> Result<String> {
    let cipher = Aes256Gcm::new_from_slice(key).expect("AES-256 key has fixed length");
    let mut nonce_bytes = [0_u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), plaintext)
        .map_err(|_| anyhow::anyhow!("failed to encrypt local daemon secret payload"))?;
    let mut envelope = Vec::with_capacity(nonce_bytes.len() + ciphertext.len());
    envelope.extend_from_slice(&nonce_bytes);
    envelope.extend_from_slice(&ciphertext);
    Ok(BASE64.encode(envelope))
}

fn decrypt_secret_payload(encoded: &str, key: &[u8; 32]) -> Result<Vec<u8>> {
    let envelope = BASE64
        .decode(encoded)
        .context("invalid encrypted local daemon secret payload")?;
    if envelope.len() <= 12 {
        bail!("encrypted local daemon secret payload is truncated");
    }
    let cipher = Aes256Gcm::new_from_slice(key).expect("AES-256 key has fixed length");
    cipher
        .decrypt(Nonce::from_slice(&envelope[..12]), &envelope[12..])
        .map_err(|_| anyhow::anyhow!("failed to decrypt local daemon secret payload"))
}

fn legacy_interval(expression: &str) -> Result<Option<TimeDelta>> {
    let normalized = expression.trim().to_ascii_lowercase();
    let rest = normalized
        .strip_prefix("every ")
        .or_else(|| normalized.strip_prefix("alle "));
    let Some(rest) = rest else { return Ok(None) };
    let split = rest
        .find(|character: char| !character.is_ascii_digit())
        .context("invalid interval expression")?;
    let (amount, unit) = rest.split_at(split);
    let amount = amount.parse::<i64>().context("invalid interval value")?;
    if amount < 1 {
        bail!("interval value must be at least one");
    }
    match unit.trim() {
        "m" | "min" | "mins" | "minute" | "minutes" if amount <= 1440 => {
            Ok(Some(TimeDelta::minutes(amount)))
        }
        "h" | "hr" | "hrs" | "hour" | "hours" | "std" | "stunde" | "stunden" if amount <= 168 => {
            Ok(Some(TimeDelta::hours(amount)))
        }
        _ => bail!("unsupported or out-of-range interval expression"),
    }
}

fn normalized_schedule_expression(expression: &str) -> Result<String> {
    let normalized = expression.trim().to_ascii_lowercase();
    if let Some(time) = normalized
        .strip_prefix("daily ")
        .or_else(|| normalized.strip_prefix("taeglich "))
    {
        let (hour, minute) = parse_schedule_time(time)?;
        return Ok(format!("0 {minute} {hour} * * *"));
    }
    let parts = normalized.split_whitespace().collect::<Vec<_>>();
    if parts.len() == 2 {
        let weekday = match parts[0] {
            "monday" | "montag" => Some("MON"),
            "tuesday" | "dienstag" => Some("TUE"),
            "wednesday" | "mittwoch" => Some("WED"),
            "thursday" | "donnerstag" => Some("THU"),
            "friday" | "freitag" => Some("FRI"),
            "saturday" | "samstag" => Some("SAT"),
            "sunday" | "sonntag" => Some("SUN"),
            _ => None,
        };
        if let Some(weekday) = weekday {
            let (hour, minute) = parse_schedule_time(parts[1])?;
            return Ok(format!("0 {minute} {hour} * * {weekday}"));
        }
    }
    let fields = expression.split_whitespace().count();
    let cron = match fields {
        5 => format!("0 {}", expression.trim()),
        6 | 7 => expression.trim().to_owned(),
        _ => bail!("schedule must be a supported interval, legacy expression, or 5-7 field cron"),
    };
    Schedule::from_str(&cron).context("invalid cron expression")?;
    Ok(cron)
}

fn parse_schedule_time(value: &str) -> Result<(u32, u32)> {
    let (hour, minute) = value.split_once(':').context("time must be HH:MM")?;
    let hour = hour.parse::<u32>().context("invalid hour")?;
    let minute = minute.parse::<u32>().context("invalid minute")?;
    if hour > 23 || minute > 59 {
        bail!("time is outside its valid range");
    }
    Ok((hour, minute))
}

fn next_schedule_at(
    expression: &str,
    timezone: &str,
    after: DateTime<Utc>,
) -> Result<DateTime<Utc>> {
    let timezone = timezone
        .parse::<Tz>()
        .with_context(|| format!("{timezone} is not a recognized IANA timezone"))?;
    if let Some(interval) = legacy_interval(expression)? {
        return Ok(after + interval);
    }
    let expression = normalized_schedule_expression(expression)?;
    Schedule::from_str(&expression)?
        .after(&after.with_timezone(&timezone))
        .next()
        .map(|value| value.with_timezone(&Utc))
        .context("schedule has no future occurrence")
}

fn count_schedule_occurrences(
    expression: &str,
    timezone: &str,
    first_due: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<(usize, bool)> {
    if let Some(interval) = legacy_interval(expression)? {
        let elapsed = now.signed_duration_since(first_due).num_seconds().max(0);
        let count = 1 + elapsed / interval.num_seconds().max(1);
        return Ok((count.min(1000) as usize, count > 1000));
    }
    let timezone = timezone
        .parse::<Tz>()
        .with_context(|| format!("{timezone} is not a recognized IANA timezone"))?;
    let expression = normalized_schedule_expression(expression)?;
    let schedule = Schedule::from_str(&expression)?;
    let mut count = 0_usize;
    for occurrence in
        schedule.after(&(first_due - TimeDelta::nanoseconds(1)).with_timezone(&timezone))
    {
        if occurrence.with_timezone(&Utc) > now {
            break;
        }
        count += 1;
        if count == 1000 {
            return Ok((count, true));
        }
    }
    Ok((count.max(1), false))
}

fn persistent_uuid(name: &str, default_path: &Path) -> Result<Uuid> {
    let encoded = if let Ok(value) = env::var(name) {
        value
    } else {
        load_or_create_private(default_path, || Uuid::new_v4().to_string())?
    };
    encoded
        .trim()
        .parse()
        .with_context(|| format!("invalid {name}"))
}

fn load_or_create_private(path: &Path, create: impl FnOnce() -> String) -> Result<String> {
    if let Ok(value) = fs::read_to_string(path) {
        return Ok(value.trim().to_owned());
    }
    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }
    let value = create();
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => {
            set_private_file_permissions(&file)?;
            use std::io::Write;
            file.write_all(value.as_bytes())?;
            file.sync_all()?;
            Ok(value)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Ok(fs::read_to_string(path)?.trim().to_owned())
        }
        Err(error) => Err(error.into()),
    }
}

fn acquire_instance_lock(data_dir: &Path) -> Result<File> {
    let path = data_dir.join("daemon.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("failed to open daemon lock {}", path.display()))?;
    set_private_file_permissions(&file)?;
    file.try_lock_exclusive()
        .with_context(|| "another Open Cowork local daemon is already running")?;
    Ok(file)
}

fn create_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn set_private_file_permissions(file: &File) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = file;
    Ok(())
}

fn default_data_dir() -> PathBuf {
    #[cfg(windows)]
    return PathBuf::from(env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".to_owned()))
        .join("OpenCowork")
        .join("daemon");
    #[cfg(not(windows))]
    return env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env::var("HOME").unwrap_or_else(|_| ".".to_owned()))
                .join(".local")
                .join("state")
        })
        .join("open-cowork")
        .join("daemon");
}

fn default_ipc_endpoint(_data_dir: &Path) -> String {
    #[cfg(windows)]
    {
        let user = env::var("USERNAME")
            .unwrap_or_else(|_| "user".to_owned())
            .replace(|ch: char| !ch.is_ascii_alphanumeric(), "_");
        format!(r"\\.\pipe\open-cowork-{user}")
    }
    #[cfg(not(windows))]
    {
        env::var("XDG_RUNTIME_DIR")
            .map(|dir| PathBuf::from(dir).join("open-cowork").join("daemon.sock"))
            .unwrap_or_else(|_| _data_dir.join("daemon.sock"))
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, TimeZone, Timelike};

    fn schedule_test_request(input: Value) -> CreateRunRequest {
        serde_json::from_value(json!({
            "thread_id": Uuid::new_v4(),
            "project_id": Uuid::new_v4(),
            "project_revision": 1,
            "project_privacy": "private_local",
            "task": {"id": Uuid::new_v4(), "revision": 1},
            "executor_target": {"kind": "personal_device", "device_id": Uuid::new_v4()},
            "required_capabilities": [],
            "input": input,
            "model_profile_id": null,
            "snapshot_id": null,
            "idempotency_key": format!("schedule-test-{}", Uuid::new_v4()),
        }))
        .unwrap()
    }

    fn schedule_test_model_config() -> PersistedModelConfig {
        PersistedModelConfig {
            base_url: "https://models.example.test/v1".to_owned(),
            api_key: Some("device-bound-key".to_owned()),
            model: "frozen-model".to_owned(),
            timeout_ms: 30_000,
            max_steps: 12,
            verify_tls_certificates: true,
            mcp_servers: Vec::new(),
            crew_request: Some(json!({
                "id": "crew-current",
                "name": "Frozen crew",
                "description": "Frozen description",
                "providerConfigs": {"openAICompatible": {
                    "profileId": "profile-current",
                    "baseUrl": "http://127.0.0.1:9/v1",
                    "model": "frozen-crew-model",
                    "apiKey": "frozen-crew-key",
                    "timeoutMs": 30000,
                    "verifyTlsCertificates": true
                }},
                "agents": [],
                "tasks": [],
            })),
            codex_request: None,
        }
    }

    fn daemon_for_test(connection: Connection, device_id: Uuid) -> Daemon {
        let (shutdown, _) = watch::channel(false);
        Daemon {
            config: Arc::new(Config {
                database_path: PathBuf::from(":memory:"),
                ipc_endpoint: "test".to_owned(),
                ipc_token: "x".repeat(32),
                user_id: Uuid::new_v4(),
                device_id,
                model_secret_key: [31; 32],
                fallback_model: schedule_test_model_config(),
                runtime_paths: RuntimePaths::default(),
            }),
            database: Arc::new(Mutex::new(connection)),
            shutdown,
            browser: Arc::new(developer_browser::DeveloperBrowserState::default()),
        }
    }

    #[tokio::test]
    async fn immediate_runs_resolve_encrypted_provider_bindings() {
        let connection = Connection::open_in_memory().unwrap();
        initialize_database(&connection).unwrap();
        let binding = ProviderDeviceBinding {
            base_url: "https://current.example.test/v1".to_owned(),
            api_key: Some("current-native-secret".to_owned()),
        };
        let encrypted =
            encrypt_secret_payload(&serde_json::to_vec(&binding).unwrap(), &[31; 32]).unwrap();
        connection
            .execute(
                "INSERT INTO daemon_provider_bindings (profile_id, encrypted_binding, updated_at) VALUES (?1, ?2, ?3)",
                params!["profile-current", encrypted, Utc::now().to_rfc3339()],
            )
            .unwrap();
        let device_id = Uuid::new_v4();
        let daemon = daemon_for_test(connection, device_id);
        let run: RunRecord = serde_json::from_value(
            create_run(
                &daemon,
                json!({
                    "thread_id": Uuid::new_v4(),
                    "project_id": Uuid::new_v4(),
                    "project_revision": 1,
                    "project_privacy": "private_local",
                    "task": null,
                    "executor_target": {"kind":"personal_device","device_id":device_id},
                    "required_capabilities": ["model.external"],
                    "input": {
                        "prompt": "test",
                        "client_provider_profile_id": "profile-current",
                        "resolve_current_provider_binding": true
                    },
                    "model_profile_id": null,
                    "snapshot_id": null,
                    "idempotency_key": format!("immediate-binding-{}", Uuid::new_v4()),
                    "model_config": {
                        "base_url": "https://stale.example.test/v1",
                        "api_key": null,
                        "model": "test-model",
                        "timeout_ms": 30000,
                        "max_steps": 8,
                        "verify_tls_certificates": true
                    }
                }),
            )
            .await
            .unwrap(),
        )
        .unwrap();

        let resolved = load_run_model_config(&daemon, run.spec.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resolved.base_url, "https://current.example.test/v1");
        assert_eq!(resolved.api_key.as_deref(), Some("current-native-secret"));
    }

    #[test]
    fn legacy_local_creator_ids_are_migrated_to_the_persistent_user_id() {
        let connection = Connection::open_in_memory().unwrap();
        initialize_database(&connection).unwrap();
        let run_id = Uuid::new_v4();
        let thread_id = Uuid::new_v4();
        let project_id = Uuid::new_v4();
        let device_id = Uuid::new_v4();
        let now = Utc::now();
        let record: RunRecord = serde_json::from_value(json!({
            "spec": {
                "schema_version": SCHEMA_VERSION,
                "id": run_id,
                "thread_id": thread_id,
                "project_id": project_id,
                "project": {"id": project_id, "revision": 1},
                "project_privacy": "private_local",
                "task": null,
                "creator_user_id": LEGACY_LOCAL_USER_ID,
                "executor_target": {"kind": "personal_device", "device_id": device_id},
                "required_capabilities": [],
                "input": {},
                "model_profile_id": null,
                "snapshot_id": null,
                "idempotency_key": format!("legacy-user-{run_id}"),
                "created_at": now,
            },
            "state": "completed",
            "revision": 1,
            "etag": format!("W/\"{run_id}:1\""),
            "assigned_executor_id": device_id,
            "lease_expires_at": null,
            "started_at": null,
            "finished_at": now,
            "result": null,
            "error": null,
            "updated_at": now,
        }))
        .unwrap();
        connection
            .execute(
                "INSERT INTO daemon_runs (id, thread_id, state, revision, record_json, created_at, updated_at, idempotency_key) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7)",
                params![
                    run_id.to_string(),
                    thread_id.to_string(),
                    "completed",
                    1,
                    serde_json::to_string(&record).unwrap(),
                    now.to_rfc3339(),
                    record.spec.idempotency_key,
                ],
            )
            .unwrap();

        let current_user_id = Uuid::new_v4();
        assert_eq!(
            migrate_legacy_creator_user_id(&connection, current_user_id).unwrap(),
            1
        );
        assert_eq!(
            migrate_legacy_creator_user_id(&connection, current_user_id).unwrap(),
            0,
            "the migration must be idempotent"
        );
        let encoded: String = connection
            .query_row(
                "SELECT record_json FROM daemon_runs WHERE id = ?1",
                [run_id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        let migrated: RunRecord = serde_json::from_str(&encoded).unwrap();
        assert_eq!(migrated.spec.creator_user_id, current_user_id);
    }

    #[tokio::test]
    async fn imported_server_run_keeps_identity_encrypted_config_and_run_workspace() {
        let connection = Connection::open_in_memory().unwrap();
        initialize_database(&connection).unwrap();
        let device_id = Uuid::new_v4();
        let daemon = daemon_for_test(connection, device_id);
        let workspace = env::temp_dir().join(format!(
            "open-cowork-imported-server-run-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(&workspace).unwrap();
        let run_id = Uuid::new_v4();
        let mut model = schedule_test_model_config();
        model.api_key = Some("imported-server-secret".to_owned());
        model.crew_request = None;
        let spec: RunSpec = serde_json::from_value(json!({
            "schema_version": SCHEMA_VERSION,
            "id": run_id,
            "thread_id": Uuid::new_v4(),
            "project_id": Uuid::new_v4(),
            "project": {"id": Uuid::new_v4(), "revision": 1},
            "project_privacy": "private_local",
            "task": null,
            "creator_user_id": Uuid::new_v4(),
            "executor_target": {"kind": "personal_device", "device_id": device_id},
            "required_capabilities": ["files"],
            "input": {"prompt": "Use the local workspace"},
            "model_profile_id": null,
            "snapshot_id": null,
            "idempotency_key": format!("server-{run_id}"),
            "created_at": Utc::now(),
        }))
        .unwrap();
        let imported: RunRecord = serde_json::from_value(
            import_server_run(
                &daemon,
                json!({
                    "run_spec": spec,
                    "model_config": model,
                    "workspace_path": workspace,
                    "defer_start": true,
                }),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(imported.spec.id, run_id);
        assert_eq!(imported.assigned_executor_id, Some(device_id));
        assert_eq!(imported.state, RunState::WaitingForExecutor);
        assert_eq!(
            run_workspace(&daemon, run_id).await.unwrap(),
            Some(workspace.canonicalize().unwrap())
        );
        let database = daemon.database.lock().await;
        let encrypted: String = database
            .query_row(
                "SELECT encrypted_config FROM daemon_run_model_configs WHERE run_id = ?1",
                [run_id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!encrypted.contains("imported-server-secret"));
        drop(database);
        let started: RunRecord =
            serde_json::from_value(start_imported_server_run(&daemon, run_id).await.unwrap())
                .unwrap();
        assert_eq!(started.state, RunState::Queued);
        cancel_run(&daemon, run_id).await.unwrap();
        assert_eq!(run_workspace(&daemon, run_id).await.unwrap(), None);
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn run_model_configuration_is_authenticated_and_encrypted() {
        let config = PersistedModelConfig {
            base_url: "https://models.example.test/v1".to_owned(),
            api_key: Some("secret-provider-key".to_owned()),
            model: "example-model".to_owned(),
            timeout_ms: 30_000,
            max_steps: 12,
            verify_tls_certificates: true,
            mcp_servers: vec![PersistedMcpServer {
                name: "test".to_owned(),
                command: "test-mcp".to_owned(),
                args: vec!["--stdio".to_owned()],
                env: std::collections::HashMap::from([(
                    "MCP_TOKEN".to_owned(),
                    "secret-mcp-token".to_owned(),
                )]),
            }],
            crew_request: None,
            codex_request: None,
        };
        let key = [7_u8; 32];
        let encrypted = encrypt_model_config(&config, &key).unwrap();
        assert!(!encrypted.contains("secret-provider-key"));
        assert!(!encrypted.contains("secret-mcp-token"));
        let decrypted = decrypt_model_config(&encrypted, &key).unwrap();
        assert_eq!(decrypted.api_key.as_deref(), Some("secret-provider-key"));
        assert_eq!(decrypted.model, "example-model");
        assert_eq!(decrypted.mcp_servers[0].name, "test");
        assert!(decrypt_model_config(&encrypted, &[8_u8; 32]).is_err());
    }

    #[test]
    fn mcp_binding_metadata_never_exposes_command_arguments_or_environment_values() {
        let binding = PersistedMcpServer {
            name: "Private MCP".to_owned(),
            command: "C:\\Private\\mcp-server.exe".to_owned(),
            args: vec!["--token".to_owned(), "argument-secret".to_owned()],
            env: std::collections::HashMap::from([(
                "MCP_TOKEN".to_owned(),
                "environment-secret".to_owned(),
            )]),
        };
        let metadata = mcp_binding_metadata("mcp-private", &binding, "2026-08-09T00:00:00Z");
        let serialized = metadata.to_string();
        assert_eq!(metadata["executable_hint"], "mcp-server.exe");
        assert_eq!(metadata["argument_count"], 2);
        assert_eq!(metadata["environment_keys"], json!(["MCP_TOKEN"]));
        assert!(!serialized.contains("C:\\Private"));
        assert!(!serialized.contains("argument-secret"));
        assert!(!serialized.contains("environment-secret"));
    }

    #[test]
    fn local_schedule_keeps_berlin_wall_clock_across_dst() {
        let before_spring = Utc.with_ymd_and_hms(2026, 3, 28, 12, 0, 0).unwrap();
        let spring = next_schedule_at("0 9 * * *", "Europe/Berlin", before_spring).unwrap();
        let spring_local = spring.with_timezone(&chrono_tz::Europe::Berlin);
        assert_eq!(
            (
                spring_local.year(),
                spring_local.month(),
                spring_local.day()
            ),
            (2026, 3, 29)
        );
        assert_eq!((spring_local.hour(), spring_local.minute()), (9, 0));

        let before_fall = Utc.with_ymd_and_hms(2026, 10, 24, 12, 0, 0).unwrap();
        let fall = next_schedule_at("daily 09:00", "Europe/Berlin", before_fall).unwrap();
        let fall_local = fall.with_timezone(&chrono_tz::Europe::Berlin);
        assert_eq!((fall_local.hour(), fall_local.minute()), (9, 0));
    }

    #[test]
    fn scheduled_template_resolves_current_project_task_crew_and_profile() {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize_database(&connection).unwrap();
        let secret_key = [42_u8; 32];
        write_daemon_entity(
            &mut connection,
            "project",
            "project-current",
            json!({"title":"Current project","instructions":"Use the current project rules."}),
            Some(0),
        )
        .unwrap();
        let encrypted_binding = encrypt_secret_payload(
            &serde_json::to_vec(&ProviderDeviceBinding {
                base_url: "http://127.0.0.1:11434/v1".to_owned(),
                api_key: Some("current-device-key".to_owned()),
            })
            .unwrap(),
            &secret_key,
        )
        .unwrap();
        assert!(!encrypted_binding.contains("current-device-key"));
        connection
            .execute(
                "INSERT INTO daemon_provider_bindings (profile_id, encrypted_binding, updated_at) VALUES (?1, ?2, ?3)",
                params!["profile-current", encrypted_binding, Utc::now().to_rfc3339()],
            )
            .unwrap();
        let encrypted_mcp_binding = encrypt_secret_payload(
            &serde_json::to_vec(&PersistedMcpServer {
                name: "Current MCP".to_owned(),
                command: "current-mcp".to_owned(),
                args: vec!["--stdio".to_owned()],
                env: std::collections::HashMap::from([(
                    "MCP_TOKEN".to_owned(),
                    "current-mcp-secret".to_owned(),
                )]),
            })
            .unwrap(),
            &secret_key,
        )
        .unwrap();
        assert!(!encrypted_mcp_binding.contains("current-mcp-secret"));
        connection
            .execute(
                "INSERT INTO daemon_mcp_bindings (server_id, encrypted_binding, updated_at) VALUES (?1, ?2, ?3)",
                params!["mcp-current", encrypted_mcp_binding, Utc::now().to_rfc3339()],
            )
            .unwrap();
        write_daemon_entity(
            &mut connection,
            "project",
            "project-current",
            json!({"title":"Current project","instructions":"Use the latest project rules."}),
            Some(1),
        )
        .unwrap();
        write_daemon_entity(
            &mut connection,
            "task",
            "task-current",
            json!({"task_kind":"work","description":"Old task","expected_output":"Old output"}),
            Some(0),
        )
        .unwrap();
        write_daemon_entity(
            &mut connection,
            "task",
            "task-current",
            json!({"task_kind":"work","description":"Current task prompt","expected_output":"Current report"}),
            Some(1),
        )
        .unwrap();
        write_daemon_entity(
            &mut connection,
            "provider_profile",
            "profile-current",
            json!({"model":"current-model","timeout_ms":45000,"verify_tls_certificates":false}),
            Some(0),
        )
        .unwrap();
        write_daemon_entity(
            &mut connection,
            "crew",
            "crew-current",
            json!({"definition":{
                "id":"crew-current",
                "name":"Current crew",
                "description":"Current crew definition",
                "agents":[{"id":"agent-current","name":"Current agent"}],
                "tasks":[{"id":"crew-task-current","description":"Current crew task"}]
            }}),
            Some(0),
        )
        .unwrap();

        let mut template = PersistedScheduleTemplate {
            request: schedule_test_request(json!({
                "prompt": "Frozen task prompt",
                "client_project_id": "project-current",
                "client_task_id": "task-current",
                "client_provider_profile_id": "profile-current",
                "crew_id": "crew-current",
                "client_mcp_server_ids": ["mcp-current"],
                "resolve_current_versions": true,
                "resolve_current_provider_binding": true,
                "resolve_current_mcp_bindings": true,
                "resolve_current_crew_provider_bindings": true,
            })),
            model_config: schedule_test_model_config(),
        };
        refresh_schedule_template_from_entities(&connection, &mut template, &secret_key).unwrap();

        assert_eq!(template.request.project_revision, 2);
        assert_eq!(template.request.task.as_ref().unwrap().revision, 2);
        assert_eq!(template.request.input["prompt"], "Current task prompt");
        assert_eq!(
            template.request.input["current_project_instructions"],
            "Use the latest project rules."
        );
        assert_eq!(template.model_config.model, "current-model");
        assert_eq!(template.model_config.timeout_ms, 45_000);
        assert!(!template.model_config.verify_tls_certificates);
        assert_eq!(
            template.model_config.crew_request.as_ref().unwrap()["description"],
            "Current crew definition"
        );
        assert_eq!(
            template.request.input["resolved_entity_revisions"],
            json!({
                "project":2,
                "task":2,
                "provider_profile":1,
                "crew":1,
                "crew_provider_profile:profile-current":1
            })
        );
        assert_eq!(
            template.model_config.api_key.as_deref(),
            Some("current-device-key"),
            "the current encrypted per-device binding is resolved at trigger time"
        );
        assert_eq!(template.model_config.base_url, "http://127.0.0.1:11434/v1");
        assert_eq!(
            template.request.input["resolved_device_provider_binding"],
            json!(true)
        );
        assert_eq!(template.model_config.mcp_servers.len(), 1);
        assert_eq!(template.model_config.mcp_servers[0].command, "current-mcp");
        assert_eq!(
            template.model_config.mcp_servers[0]
                .env
                .get("MCP_TOKEN")
                .map(String::as_str),
            Some("current-mcp-secret")
        );
        assert_eq!(
            template.request.input["resolved_device_mcp_bindings"],
            json!(true)
        );
        assert_eq!(
            template.request.input["resolved_mcp_server_ids"],
            json!(["mcp-current"])
        );
        let crew_provider = &template.model_config.crew_request.as_ref().unwrap()
            ["providerConfigs"]["openAICompatible"];
        assert_eq!(crew_provider["baseUrl"], "http://127.0.0.1:11434/v1");
        assert_eq!(crew_provider["apiKey"], "current-device-key");
        assert_eq!(crew_provider["model"], "current-model");
        assert_eq!(crew_provider["timeoutMs"], 45_000);
        assert_eq!(crew_provider["verifyTlsCertificates"], false);
        assert_eq!(
            template.request.input["resolved_device_crew_provider_bindings"],
            json!(true)
        );
        assert_eq!(
            template.request.input["resolved_crew_provider_profile_ids"],
            json!(["profile-current"])
        );
    }

    #[test]
    fn current_version_schedule_waits_when_required_metadata_is_missing() {
        let connection = Connection::open_in_memory().unwrap();
        initialize_database(&connection).unwrap();
        let mut template = PersistedScheduleTemplate {
            request: schedule_test_request(json!({
                "prompt": "Scheduled task",
                "client_project_id": "missing-project",
                "resolve_current_versions": true,
            })),
            model_config: schedule_test_model_config(),
        };
        let error = refresh_schedule_template_from_entities(&connection, &mut template, &[9; 32])
            .unwrap_err()
            .to_string();
        assert!(error.contains("waiting for current project metadata"));
    }

    #[test]
    fn current_schedule_waits_for_a_missing_device_provider_binding() {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize_database(&connection).unwrap();
        write_daemon_entity(
            &mut connection,
            "provider_profile",
            "profile-without-binding",
            json!({"model":"local-model"}),
            Some(0),
        )
        .unwrap();
        let mut template = PersistedScheduleTemplate {
            request: schedule_test_request(json!({
                "prompt": "Scheduled task",
                "client_provider_profile_id": "profile-without-binding",
                "resolve_current_versions": true,
                "resolve_current_provider_binding": true,
            })),
            model_config: schedule_test_model_config(),
        };
        let error = refresh_schedule_template_from_entities(&connection, &mut template, &[7; 32])
            .unwrap_err()
            .to_string();
        assert!(error.contains("waiting for the per-device provider binding"));
    }

    #[test]
    fn current_schedule_waits_for_a_missing_device_mcp_binding() {
        let connection = Connection::open_in_memory().unwrap();
        initialize_database(&connection).unwrap();
        let mut template = PersistedScheduleTemplate {
            request: schedule_test_request(json!({
                "prompt": "Scheduled MCP task",
                "client_mcp_server_ids": ["mcp-without-binding"],
                "resolve_current_mcp_bindings": true,
            })),
            model_config: schedule_test_model_config(),
        };
        let error = refresh_schedule_template_from_entities(&connection, &mut template, &[6; 32])
            .unwrap_err()
            .to_string();
        assert!(error.contains("waiting for the per-device MCP binding"));
    }

    #[test]
    fn current_crew_schedule_waits_for_a_missing_device_provider_binding() {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize_database(&connection).unwrap();
        write_daemon_entity(
            &mut connection,
            "crew",
            "crew-current",
            json!({"definition":{"id":"crew-current","name":"Current crew"}}),
            Some(0),
        )
        .unwrap();
        write_daemon_entity(
            &mut connection,
            "provider_profile",
            "crew-profile-without-binding",
            json!({"model":"current-crew-model"}),
            Some(0),
        )
        .unwrap();
        let mut model_config = schedule_test_model_config();
        model_config.crew_request.as_mut().unwrap()["providerConfigs"] = json!({
            "openAICompatible": {
                "profileId": "crew-profile-without-binding",
                "baseUrl": "http://127.0.0.1:9/v1",
                "model": "frozen-model",
                "apiKey": "frozen-key"
            }
        });
        let mut template = PersistedScheduleTemplate {
            request: schedule_test_request(json!({
                "prompt": "Scheduled Crew task",
                "crew_id": "crew-current",
                "resolve_current_versions": true,
                "resolve_current_crew_provider_bindings": true,
            })),
            model_config,
        };
        let error = refresh_schedule_template_from_entities(&connection, &mut template, &[5; 32])
            .unwrap_err()
            .to_string();
        assert!(error.contains("waiting for the per-device Crew provider binding"));
    }

    #[test]
    fn missed_intervals_collapse_into_one_catch_up_count() {
        let first = Utc.with_ymd_and_hms(2026, 1, 1, 9, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 13, 30, 0).unwrap();
        let (count, truncated) =
            count_schedule_occurrences("every 1h", "Europe/Berlin", first, now).unwrap();
        assert_eq!(count, 5);
        assert!(!truncated);
    }

    #[test]
    fn file_tools_copy_directory_without_overwrite_or_self_recursion() {
        let root = env::temp_dir().join(format!("open-cowork-file-tools-{}", Uuid::new_v4()));
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("nested").join("note.txt"), "durable").unwrap();

        copy_workspace_path(&source, &destination).unwrap();
        assert_eq!(
            fs::read_to_string(destination.join("nested").join("note.txt")).unwrap(),
            "durable"
        );
        assert!(copy_workspace_path(&source, &destination).is_err());
        assert!(copy_workspace_path(&source, &source.join("inside")).is_err());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn file_tools_reject_workspace_traversal() {
        let root = env::temp_dir().join(format!("open-cowork-file-path-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        assert!(safe_workspace_path_no_symlinks(&root, "../outside", false).is_err());
        assert_eq!(
            safe_workspace_path_no_symlinks(&root, ".", true).unwrap(),
            root.canonicalize().unwrap()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn daemon_entities_are_revisioned_and_leave_tombstones_and_changes() {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize_database(&connection).unwrap();
        let created = write_daemon_entity(
            &mut connection,
            "task",
            "task-1",
            json!({"title": "Persistent", "status": "pending"}),
            None,
        )
        .unwrap();
        assert_eq!(created["revision"], 1);
        let idempotent = write_daemon_entity(
            &mut connection,
            "task",
            "task-1",
            json!({"title": "Persistent", "status": "pending"}),
            Some(1),
        )
        .unwrap();
        assert_eq!(idempotent["revision"], 1);
        let updated = write_daemon_entity(
            &mut connection,
            "task",
            "task-1",
            json!({"title": "Persistent", "status": "completed"}),
            Some(1),
        )
        .unwrap();
        assert_eq!(updated["revision"], 2);
        assert!(write_daemon_entity(
            &mut connection,
            "task",
            "task-1",
            json!({"title": "Conflict"}),
            Some(1),
        )
        .is_err());
        let deleted = tombstone_daemon_entity(&mut connection, "task", "task-1", Some(2)).unwrap();
        assert_eq!(deleted["revision"], 3);
        assert_eq!(deleted["tombstone"], true);
        assert!(list_daemon_entities(&connection, "task", false)
            .unwrap()
            .is_empty());
        assert_eq!(
            list_daemon_entities(&connection, "task", true)
                .unwrap()
                .len(),
            1
        );
        let changes: i64 = connection
            .query_row("SELECT COUNT(*) FROM daemon_sync_changes", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(changes, 3);
    }

    #[test]
    fn web_search_parser_decodes_public_result_urls_and_rejects_unsafe_schemes() {
        let body = r#"
          <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fguide">Example &amp; Guide</a>
          <a class="result__snippet">A <b>bounded</b> result.</a>
          <a class="result__a" href="javascript:alert(1)">Unsafe</a>
        "#;
        let results = parse_duckduckgo_results(body, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://example.com/guide");
        assert_eq!(results[0].title, "Example & Guide");
        assert!(results[0].snippet.contains("bounded result"));
    }
}
