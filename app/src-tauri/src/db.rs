use rusqlite::{
    params, params_from_iter, Connection, DatabaseName, OptionalExtension, Result as SqlResult,
    TransactionBehavior,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use crate::sensitive_data::{
    diagnostic_label, redact_and_bound_json_text, redact_and_bound_optional_json,
    redact_and_bound_optional_text, redact_and_bound_text, MAX_LOG_JSON_BYTES,
    MAX_LOG_SUMMARY_BYTES, MAX_LOG_TEXT_BYTES,
};

const LATEST_SCHEMA_VERSION: i64 = 23;
const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PRE_MIGRATION_BACKUPS: usize = 3;
const MAX_ENGINE_EVENTS_PER_RUN: i64 = 2_000;
const MAX_CREW_EVENTS_PER_RUN: i64 = 2_000;
const MAX_CREW_LOGS_PER_RUN: i64 = 5_000;
const MAX_SCHEDULED_RUNS_PER_TASK: i64 = 500;
const MAX_AUDIT_EVENTS: i64 = 10_000;

pub struct Database {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StartupRecoveryReport {
    pub recovered_at: String,
    pub engine_runs: usize,
    pub legacy_tasks: usize,
    pub task_steps: usize,
    pub work_tasks: usize,
    pub scheduled_runs: usize,
    pub crew_runs: usize,
    pub worker_sandboxes: usize,
    pub managed_processes: usize,
    pub terminal_backends: usize,
}

impl StartupRecoveryReport {
    pub fn total(&self) -> usize {
        self.engine_runs
            + self.legacy_tasks
            + self.task_steps
            + self.work_tasks
            + self.scheduled_runs
            + self.crew_runs
            + self.worker_sandboxes
            + self.managed_processes
            + self.terminal_backends
    }
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> SqlResult<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = stmt.query_map([], |row| row.get::<_, String>(1))?;

    for name in columns {
        if name? == column {
            return Ok(true);
        }
    }

    Ok(false)
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> SqlResult<()> {
    if !table_has_column(conn, table, column)? {
        conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {definition}"), [])?;
    }

    Ok(())
}

fn database_error(code: i32, message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), Some(message.into()))
}

fn configure_connection(conn: &Connection, persistent: bool) -> SqlResult<()> {
    conn.busy_timeout(SQLITE_BUSY_TIMEOUT)?;
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;
         PRAGMA synchronous=FULL;
         PRAGMA temp_store=MEMORY;",
    )?;
    if persistent {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA wal_autocheckpoint=1000;
             PRAGMA journal_size_limit=67108864;",
        )?;
    }
    Ok(())
}

fn ensure_database_integrity(conn: &Connection) -> SqlResult<()> {
    let result: String = conn.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
    if !result.eq_ignore_ascii_case("ok") {
        return Err(database_error(
            rusqlite::ffi::SQLITE_CORRUPT,
            format!("database integrity check failed: {result}"),
        ));
    }

    let foreign_key_violation = conn
        .query_row("PRAGMA foreign_key_check", [], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .optional()?;
    if let Some((table, row_id, parent)) = foreign_key_violation {
        return Err(database_error(
            rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY,
            format!(
                "foreign key integrity check failed for table {table}, row {row_id:?}, parent {parent}"
            ),
        ));
    }

    Ok(())
}

fn current_schema_version(conn: &Connection) -> SqlResult<Option<i64>> {
    let has_schema_version = conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM sqlite_master
             WHERE type = 'table' AND name = 'schema_version'
         )",
        [],
        |row| row.get::<_, i64>(0),
    )? != 0;
    if !has_schema_version {
        return Ok(None);
    }

    let (row_count, version) = conn.query_row(
        "SELECT COUNT(*), MAX(version) FROM schema_version",
        [],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?)),
    )?;
    match (row_count, version) {
        (0, _) => Ok(None),
        (1, Some(version)) => Ok(Some(version)),
        _ => Err(database_error(
            rusqlite::ffi::SQLITE_CORRUPT,
            "schema_version must contain exactly one integer row",
        )),
    }
}

fn ensure_supported_schema_version(version: i64) -> SqlResult<()> {
    if (0..=LATEST_SCHEMA_VERSION).contains(&version) {
        Ok(())
    } else {
        Err(database_error(
            rusqlite::ffi::SQLITE_ERROR,
            format!("unsupported database schema version {version}; this build supports versions 0 through {LATEST_SCHEMA_VERSION}"),
        ))
    }
}

fn create_pre_migration_backup(
    conn: &Connection,
    app_data_dir: &Path,
    source_version: i64,
) -> SqlResult<PathBuf> {
    let backup_dir = app_data_dir.join("database-backups");
    std::fs::create_dir_all(&backup_dir)
        .map_err(|_| rusqlite::Error::InvalidPath(backup_dir.clone()))?;
    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let backup_path = backup_dir.join(format!(
        "pre-migration-v{source_version}-to-v{LATEST_SCHEMA_VERSION}-{timestamp}-{}.db",
        uuid::Uuid::new_v4()
    ));

    conn.backup(DatabaseName::Main, &backup_path, None)?;
    let backup = Connection::open(&backup_path)?;
    ensure_database_integrity(&backup)?;
    prune_pre_migration_backups(&backup_dir, MAX_PRE_MIGRATION_BACKUPS);
    Ok(backup_path)
}

fn prune_pre_migration_backups(backup_dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(backup_dir) else {
        return;
    };
    let mut backups = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false)
                && entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("pre-migration-")
                && entry.path().extension().and_then(|value| value.to_str()) == Some("db")
        })
        .collect::<Vec<_>>();
    backups.sort_by_key(|entry| std::cmp::Reverse(entry.file_name()));
    for entry in backups.into_iter().skip(keep) {
        let _ = std::fs::remove_file(entry.path());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRow {
    pub id: String,
    pub title: String,
    pub instructions: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectResourceRow {
    pub id: String,
    pub project_id: String,
    pub kind: String,
    pub path: String,
    pub label: Option<String>,
    pub enabled: bool,
    pub added_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkTaskRow {
    pub id: String,
    pub title: String,
    pub prompt: String,
    pub expected_output: String,
    pub work_dir: String,
    pub thread_id: Option<String>,
    pub runner: String,
    pub crew_id: Option<String>,
    pub model: String,
    pub schedule_expr: String,
    pub schedule_enabled: bool,
    pub status: String,
    pub output: Option<String>,
    pub error: Option<String>,
    pub last_run_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntryRow {
    pub id: String,
    pub scope: String,
    pub category: String,
    pub key: String,
    pub content: String,
    pub source_session_id: Option<String>,
    pub confidence: f64,
    pub access_count: i32,
    pub last_accessed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfileRow {
    pub id: String,
    pub key: String,
    pub value: String,
    pub source: String,
    pub confidence: f64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRow {
    pub id: String,
    pub name: String,
    pub description: String,
    pub prompt_template: String,
    pub trigger_pattern: Option<String>,
    pub run_mode: String,
    pub version: i32,
    pub usage_count: i32,
    pub success_count: i32,
    pub fail_count: i32,
    pub avg_quality: f64,
    pub auto_generated: bool,
    pub parent_skill_id: Option<String>,
    pub source_task_ids: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRow {
    pub id: String,
    pub thread_id: Option<String>,
    pub title: String,
    pub summary: Option<String>,
    pub model_used: Option<String>,
    pub provider: Option<String>,
    pub personality: Option<String>,
    pub total_messages: i32,
    pub total_tokens_est: i64,
    pub outcome: Option<String>,
    pub started_at: String,
    pub ended_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningOutcomeRow {
    pub id: String,
    pub session_id: Option<String>,
    pub task_id: Option<String>,
    pub outcome_type: String,
    pub description: String,
    pub learned_pattern: Option<String>,
    pub confidence: f64,
    pub applied_count: i32,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalBackendRow {
    pub id: String,
    pub name: String,
    pub backend_type: String,
    pub config_json: String,
    pub status: String,
    pub last_connected_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedProcessRow {
    pub id: String,
    pub label: String,
    pub command: String,
    pub backend_id: Option<String>,
    pub pid: Option<i64>,
    pub status: String,
    pub exit_code: Option<i32>,
    pub requires_admin: bool,
    pub admin_approved: bool,
    pub log_path: Option<String>,
    pub started_at: Option<String>,
    pub stopped_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonalityRow {
    pub id: String,
    pub name: String,
    pub description: String,
    pub role: String,
    pub goal: String,
    pub system_prompt: String,
    pub skills_markdown: String,
    pub temperature: Option<f64>,
    pub model_override: Option<String>,
    pub icon: Option<String>,
    pub is_default: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsightsEventRow {
    pub id: String,
    pub event_type: String,
    pub category: String,
    pub value_num: Option<f64>,
    pub value_text: Option<String>,
    pub session_id: Option<String>,
    pub metadata_json: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcPipelineRow {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub steps_json: String,
    pub zero_context: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryProviderRow {
    pub id: String,
    pub name: String,
    pub provider_type: String,
    pub config_json: String,
    pub enabled: bool,
    pub last_sync_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolGatewayRow {
    pub id: String,
    pub tool_type: String,
    pub name: String,
    pub config_json: String,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSearchResultRow {
    pub session_id: String,
    pub session_title: String,
    pub session_summary: Option<String>,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub matched_message_id: Option<String>,
    pub matched_content: Option<String>,
    pub matched_role: Option<String>,
    pub matched_timestamp: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineRunRow {
    pub id: String,
    pub parent_run_id: Option<String>,
    pub thread_id: Option<String>,
    pub session_id: Option<String>,
    pub title: String,
    pub input_summary: Option<String>,
    pub source: String,
    pub status: String,
    pub phase: String,
    pub cwd: Option<String>,
    pub workspace_path: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub provider_profile_id: Option<String>,
    pub runtime_mode: String,
    pub toolset_policy_id: Option<String>,
    pub channel_kind: Option<String>,
    pub channel_ref: Option<String>,
    pub retry_count: i32,
    pub resumed_from_run_id: Option<String>,
    pub checkpoint_json: Option<String>,
    pub result_summary: Option<String>,
    pub error: Option<String>,
    pub metadata_json: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub canceled_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineRunEventRow {
    pub id: String,
    pub run_id: String,
    pub sequence: i64,
    pub event_type: String,
    pub summary: String,
    pub payload_json: Option<String>,
    pub redaction_level: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineRunArtifactRow {
    pub id: String,
    pub run_id: String,
    pub kind: String,
    pub path: String,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineRunCheckpointRow {
    pub id: String,
    pub run_id: String,
    pub label: String,
    pub snapshot_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeInstructionRow {
    pub id: String,
    pub scope_type: String,
    pub scope_ref: Option<String>,
    pub title: String,
    pub content: String,
    pub enabled: bool,
    pub priority: i32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerSandboxRow {
    pub id: String,
    pub run_id: String,
    pub parent_run_id: Option<String>,
    pub backend_id: Option<String>,
    pub status: String,
    pub mode: String,
    pub source_cwd: String,
    pub workspace_root: String,
    pub allowed_roots_json: String,
    pub read_only_roots_json: Option<String>,
    pub allow_file_read: bool,
    pub allow_file_write: bool,
    pub allow_shell_execution: bool,
    pub allow_web_fetch: bool,
    pub allow_web_search: bool,
    pub allow_mcp: bool,
    pub env_json: Option<String>,
    pub metadata_json: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub ended_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrewDefinitionRow {
    pub id: String,
    pub name: String,
    pub description: String,
    pub definition_json: String,
    pub flow_json: Option<String>,
    pub version_count: i32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrewDefinitionVersionRow {
    pub id: String,
    pub crew_id: String,
    pub version_number: i32,
    pub change_summary: Option<String>,
    pub definition_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrewRoleBindingRow {
    pub id: String,
    pub scope_type: String,
    pub scope_ref: Option<String>,
    pub role: String,
    pub subject: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrewApprovalRow {
    pub id: String,
    pub crew_id: Option<String>,
    pub run_id: Option<String>,
    pub approval_type: String,
    pub scope_ref: Option<String>,
    pub status: String,
    pub requested_by: Option<String>,
    pub resolved_by: Option<String>,
    pub payload_json: Option<String>,
    pub resolution_note: Option<String>,
    pub requested_at: String,
    pub resolved_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrewRunEventRow {
    pub id: String,
    pub run_id: String,
    pub crew_id: String,
    pub event_type: String,
    pub payload_json: Option<String>,
    pub created_at: String,
}

fn map_engine_run_row(row: &rusqlite::Row) -> SqlResult<EngineRunRow> {
    Ok(EngineRunRow {
        id: row.get(0)?,
        parent_run_id: row.get(1)?,
        thread_id: row.get(2)?,
        session_id: row.get(3)?,
        title: row.get(4)?,
        input_summary: row.get(5)?,
        status: row.get(6)?,
        phase: row.get(7)?,
        cwd: row.get(8)?,
        model: row.get(9)?,
        provider: row.get(10)?,
        retry_count: row.get(11)?,
        resumed_from_run_id: row.get(12)?,
        checkpoint_json: row.get(13)?,
        result_summary: row.get(14)?,
        error: row.get(15)?,
        metadata_json: row.get(16)?,
        created_at: row.get(17)?,
        updated_at: row.get(18)?,
        started_at: row.get(19)?,
        ended_at: row.get(20)?,
        canceled_at: row.get(21)?,
        source: row.get(22)?,
        workspace_path: row.get(23)?,
        provider_profile_id: row.get(24)?,
        runtime_mode: row.get(25)?,
        toolset_policy_id: row.get(26)?,
        channel_kind: row.get(27)?,
        channel_ref: row.get(28)?,
    })
}

fn map_engine_run_event_row(row: &rusqlite::Row) -> SqlResult<EngineRunEventRow> {
    Ok(EngineRunEventRow {
        id: row.get(0)?,
        run_id: row.get(1)?,
        sequence: row.get(2)?,
        event_type: row.get(3)?,
        summary: row.get(4)?,
        payload_json: row.get(5)?,
        redaction_level: row.get(6)?,
        created_at: row.get(7)?,
    })
}

fn map_engine_run_artifact_row(row: &rusqlite::Row) -> SqlResult<EngineRunArtifactRow> {
    Ok(EngineRunArtifactRow {
        id: row.get(0)?,
        run_id: row.get(1)?,
        kind: row.get(2)?,
        path: row.get(3)?,
        title: row.get(4)?,
        summary: row.get(5)?,
        created_at: row.get(6)?,
    })
}

fn map_insights_row(row: &rusqlite::Row) -> SqlResult<InsightsEventRow> {
    Ok(InsightsEventRow {
        id: row.get(0)?,
        event_type: row.get(1)?,
        category: row.get(2)?,
        value_num: row.get(3)?,
        value_text: row.get(4)?,
        session_id: row.get(5)?,
        metadata_json: row.get(6)?,
        created_at: row.get(7)?,
    })
}
fn map_worker_sandbox_row(row: &rusqlite::Row) -> SqlResult<WorkerSandboxRow> {
    Ok(WorkerSandboxRow {
        id: row.get(0)?,
        run_id: row.get(1)?,
        parent_run_id: row.get(2)?,
        backend_id: row.get(3)?,
        status: row.get(4)?,
        mode: row.get(5)?,
        source_cwd: row.get(6)?,
        workspace_root: row.get(7)?,
        allowed_roots_json: row.get(8)?,
        read_only_roots_json: row.get(9)?,
        allow_file_read: row.get::<_, i32>(10)? != 0,
        allow_file_write: row.get::<_, i32>(11)? != 0,
        allow_shell_execution: row.get::<_, i32>(12)? != 0,
        allow_web_fetch: row.get::<_, i32>(13)? != 0,
        allow_web_search: row.get::<_, i32>(14)? != 0,
        allow_mcp: row.get::<_, i32>(15)? != 0,
        env_json: row.get(16)?,
        metadata_json: row.get(17)?,
        created_at: row.get(18)?,
        updated_at: row.get(19)?,
        ended_at: row.get(20)?,
    })
}

impl Database {
    pub fn open(app_data_dir: PathBuf) -> SqlResult<Self> {
        std::fs::create_dir_all(&app_data_dir)
            .map_err(|_| rusqlite::Error::InvalidPath(app_data_dir.clone()))?;
        let db_path = app_data_dir.join("open_cowork.db");
        let should_backup = db_path
            .metadata()
            .map(|metadata| metadata.is_file() && metadata.len() > 0)
            .unwrap_or(false);
        let conn = Connection::open(&db_path)?;
        configure_connection(&conn, true)?;
        ensure_database_integrity(&conn)?;
        let source_version = current_schema_version(&conn)?.unwrap_or(0);
        ensure_supported_schema_version(source_version)?;
        if should_backup && source_version < LATEST_SCHEMA_VERSION {
            create_pre_migration_backup(&conn, &app_data_dir, source_version)?;
        }
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.migrate()?;
        db.ensure_integrity()?;
        Ok(db)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> SqlResult<Self> {
        let conn = Connection::open_in_memory()?;
        configure_connection(&conn, false)?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.migrate()?;
        db.ensure_integrity()?;
        Ok(db)
    }

    fn ensure_integrity(&self) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        ensure_database_integrity(&conn)
    }

    fn migrate(&self) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let transaction =
            rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Immediate)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER NOT NULL
            );

            INSERT INTO schema_version (version)
            SELECT 0 WHERE NOT EXISTS (SELECT 1 FROM schema_version);",
        )?;

        let version: i64 =
            conn.query_row("SELECT version FROM schema_version LIMIT 1", [], |row| {
                row.get(0)
            })?;
        ensure_supported_schema_version(version)?;

        if version < 1 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS chat_threads (
                    id TEXT PRIMARY KEY,
                    title TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS chat_messages (
                    id TEXT PRIMARY KEY,
                    thread_id TEXT NOT NULL,
                    role TEXT NOT NULL,
                    content TEXT NOT NULL,
                    timestamp INTEGER NOT NULL,
                    FOREIGN KEY(thread_id) REFERENCES chat_threads(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_messages_thread ON chat_messages(thread_id, timestamp);

                CREATE TABLE IF NOT EXISTS tasks (
                    id TEXT PRIMARY KEY,
                    title TEXT NOT NULL,
                    prompt TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'created',
                    thread_id TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    error TEXT,
                    FOREIGN KEY(thread_id) REFERENCES chat_threads(id) ON DELETE SET NULL
                );

                CREATE TABLE IF NOT EXISTS task_steps (
                    id TEXT PRIMARY KEY,
                    task_id TEXT NOT NULL,
                    idx INTEGER NOT NULL,
                    title TEXT NOT NULL,
                    state TEXT NOT NULL DEFAULT 'pending',
                    requires_approval INTEGER NOT NULL DEFAULT 0,
                    risk_level TEXT NOT NULL DEFAULT 'low',
                    output TEXT,
                    FOREIGN KEY(task_id) REFERENCES tasks(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_steps_task ON task_steps(task_id, idx);

                CREATE TABLE IF NOT EXISTS audit_events (
                    id TEXT PRIMARY KEY,
                    ts TEXT NOT NULL,
                    event_type TEXT NOT NULL,
                    resource_type TEXT,
                    resource_id TEXT,
                    details_json TEXT
                );

                CREATE INDEX IF NOT EXISTS idx_audit_ts ON audit_events(ts, event_type);

                UPDATE schema_version SET version = 1;"
            )?;
        }

        if version < 2 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS allowed_folders (
                    path TEXT PRIMARY KEY,
                    created_at TEXT NOT NULL DEFAULT (datetime('now'))
                );

                UPDATE schema_version SET version = 2;",
            )?;
        }

        if version < 3 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS artifact_versions (
                    id TEXT PRIMARY KEY,
                    run_id TEXT,
                    label TEXT,
                    source_path TEXT NOT NULL,
                    format TEXT NOT NULL,
                    size_bytes INTEGER NOT NULL,
                    summary TEXT NOT NULL,
                    preview TEXT NOT NULL,
                    metadata_json TEXT NOT NULL,
                    created_at TEXT NOT NULL
                );

                CREATE INDEX IF NOT EXISTS idx_artifact_versions_created_at
                    ON artifact_versions(created_at DESC);
                CREATE INDEX IF NOT EXISTS idx_artifact_versions_run_id
                    ON artifact_versions(run_id, created_at DESC);

                UPDATE schema_version SET version = 3;",
            )?;
        }

        if version < 4 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS scheduled_tasks (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    prompt TEXT NOT NULL,
                    schedule_expr TEXT NOT NULL,
                    active INTEGER NOT NULL DEFAULT 1,
                    last_run_at TEXT,
                    next_run_at TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_next_run
                    ON scheduled_tasks(active, next_run_at);

                CREATE TABLE IF NOT EXISTS scheduled_runs (
                    id TEXT PRIMARY KEY,
                    task_id TEXT NOT NULL,
                    status TEXT NOT NULL,
                    started_at TEXT NOT NULL,
                    finished_at TEXT,
                    result TEXT,
                    error TEXT,
                    FOREIGN KEY(task_id) REFERENCES scheduled_tasks(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_scheduled_runs_task
                    ON scheduled_runs(task_id, started_at DESC);

                UPDATE schema_version SET version = 4;",
            )?;
        }

        if version < 5 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS artifact_exports (
                    id TEXT PRIMARY KEY,
                    artifact_version_id TEXT NOT NULL,
                    export_format TEXT NOT NULL,
                    target_path TEXT NOT NULL,
                    size_bytes INTEGER NOT NULL,
                    created_at TEXT NOT NULL,
                    FOREIGN KEY(artifact_version_id) REFERENCES artifact_versions(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_artifact_exports_created_at
                    ON artifact_exports(created_at DESC);
                CREATE INDEX IF NOT EXISTS idx_artifact_exports_version_id
                    ON artifact_exports(artifact_version_id, created_at DESC);

                CREATE TABLE IF NOT EXISTS policy_flags (
                    key TEXT PRIMARY KEY,
                    value INTEGER NOT NULL,
                    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
                );

                CREATE TABLE IF NOT EXISTS policy_deny_rules (
                    rule TEXT PRIMARY KEY,
                    created_at TEXT NOT NULL DEFAULT (datetime('now'))
                );

                UPDATE schema_version SET version = 5;"
            )?;
        }

        if version < 6 {
            conn.execute_batch(
                "-- Agent memory entries (separate from user profile)
                CREATE TABLE IF NOT EXISTS memory_entries (
                    id TEXT PRIMARY KEY,
                    scope TEXT NOT NULL DEFAULT 'agent',
                    category TEXT NOT NULL DEFAULT 'general',
                    key TEXT NOT NULL,
                    content TEXT NOT NULL,
                    source_session_id TEXT,
                    confidence REAL NOT NULL DEFAULT 1.0,
                    access_count INTEGER NOT NULL DEFAULT 0,
                    last_accessed_at TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    UNIQUE(scope, category, key)
                );

                CREATE INDEX IF NOT EXISTS idx_memory_scope_cat
                    ON memory_entries(scope, category);
                CREATE INDEX IF NOT EXISTS idx_memory_key
                    ON memory_entries(key);

                -- User profile entries (separate from agent memory)
                CREATE TABLE IF NOT EXISTS user_profile (
                    id TEXT PRIMARY KEY,
                    key TEXT NOT NULL UNIQUE,
                    value TEXT NOT NULL,
                    source TEXT NOT NULL DEFAULT 'inferred',
                    confidence REAL NOT NULL DEFAULT 0.8,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                -- Skills (auto-generated + improved)
                CREATE TABLE IF NOT EXISTS skills (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL UNIQUE,
                    description TEXT NOT NULL,
                    prompt_template TEXT NOT NULL,
                    trigger_pattern TEXT,
                    run_mode TEXT NOT NULL DEFAULT 'execute',
                    version INTEGER NOT NULL DEFAULT 1,
                    usage_count INTEGER NOT NULL DEFAULT 0,
                    success_count INTEGER NOT NULL DEFAULT 0,
                    fail_count INTEGER NOT NULL DEFAULT 0,
                    avg_quality REAL NOT NULL DEFAULT 0.0,
                    auto_generated INTEGER NOT NULL DEFAULT 0,
                    parent_skill_id TEXT,
                    source_task_ids TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    FOREIGN KEY(parent_skill_id) REFERENCES skills(id) ON DELETE SET NULL
                );

                CREATE INDEX IF NOT EXISTS idx_skills_name ON skills(name);
                CREATE INDEX IF NOT EXISTS idx_skills_trigger ON skills(trigger_pattern);

                -- Skill improvement log
                CREATE TABLE IF NOT EXISTS skill_improvements (
                    id TEXT PRIMARY KEY,
                    skill_id TEXT NOT NULL,
                    version_before INTEGER NOT NULL,
                    version_after INTEGER NOT NULL,
                    reason TEXT NOT NULL,
                    diff_summary TEXT,
                    prompt_before TEXT,
                    prompt_after TEXT,
                    quality_delta REAL NOT NULL DEFAULT 0.0,
                    created_at TEXT NOT NULL,
                    FOREIGN KEY(skill_id) REFERENCES skills(id) ON DELETE CASCADE
                );

                -- Session snapshots for frozen memory + recall
                CREATE TABLE IF NOT EXISTS sessions (
                    id TEXT PRIMARY KEY,
                    thread_id TEXT,
                    title TEXT NOT NULL,
                    summary TEXT,
                    memory_snapshot_json TEXT,
                    model_used TEXT,
                    provider TEXT,
                    personality TEXT,
                    total_messages INTEGER NOT NULL DEFAULT 0,
                    total_tokens_est INTEGER NOT NULL DEFAULT 0,
                    task_ids TEXT,
                    skill_ids_used TEXT,
                    outcome TEXT,
                    started_at TEXT NOT NULL,
                    ended_at TEXT,
                    FOREIGN KEY(thread_id) REFERENCES chat_threads(id) ON DELETE SET NULL
                );

                CREATE INDEX IF NOT EXISTS idx_sessions_started
                    ON sessions(started_at DESC);

                -- Learning loop outcomes
                CREATE TABLE IF NOT EXISTS learning_outcomes (
                    id TEXT PRIMARY KEY,
                    session_id TEXT,
                    task_id TEXT,
                    outcome_type TEXT NOT NULL,
                    description TEXT NOT NULL,
                    learned_pattern TEXT,
                    confidence REAL NOT NULL DEFAULT 0.5,
                    applied_count INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT NOT NULL,
                    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE SET NULL,
                    FOREIGN KEY(task_id) REFERENCES tasks(id) ON DELETE SET NULL
                );

                CREATE INDEX IF NOT EXISTS idx_learning_outcomes_session
                    ON learning_outcomes(session_id);

                -- Terminal backends
                CREATE TABLE IF NOT EXISTS terminal_backends (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL UNIQUE,
                    backend_type TEXT NOT NULL DEFAULT 'local',
                    config_json TEXT NOT NULL DEFAULT '{}',
                    status TEXT NOT NULL DEFAULT 'disconnected',
                    last_connected_at TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                -- Background processes
                CREATE TABLE IF NOT EXISTS managed_processes (
                    id TEXT PRIMARY KEY,
                    label TEXT NOT NULL,
                    command TEXT NOT NULL,
                    backend_id TEXT,
                    pid INTEGER,
                    status TEXT NOT NULL DEFAULT 'stopped',
                    exit_code INTEGER,
                    requires_admin INTEGER NOT NULL DEFAULT 0,
                    admin_approved INTEGER NOT NULL DEFAULT 0,
                    log_path TEXT,
                    started_at TEXT,
                    stopped_at TEXT,
                    created_at TEXT NOT NULL,
                    FOREIGN KEY(backend_id) REFERENCES terminal_backends(id) ON DELETE SET NULL
                );

                CREATE INDEX IF NOT EXISTS idx_managed_processes_status
                    ON managed_processes(status);

                -- Agent personalities / work modes
                CREATE TABLE IF NOT EXISTS agent_personalities (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL UNIQUE,
                    description TEXT NOT NULL,
                    role TEXT NOT NULL DEFAULT 'custom',
                    goal TEXT NOT NULL DEFAULT '',
                    system_prompt TEXT NOT NULL,
                    skills_markdown TEXT NOT NULL DEFAULT '',
                    temperature REAL,
                    model_override TEXT,
                    icon TEXT,
                    is_default INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                -- Insights / usage analytics
                CREATE TABLE IF NOT EXISTS insights_events (
                    id TEXT PRIMARY KEY,
                    event_type TEXT NOT NULL,
                    category TEXT NOT NULL,
                    value_num REAL,
                    value_text TEXT,
                    session_id TEXT,
                    metadata_json TEXT,
                    created_at TEXT NOT NULL
                );

                CREATE INDEX IF NOT EXISTS idx_insights_events_type
                    ON insights_events(event_type, created_at DESC);
                CREATE INDEX IF NOT EXISTS idx_insights_events_category
                    ON insights_events(category, created_at DESC);

                -- RPC pipeline definitions
                CREATE TABLE IF NOT EXISTS rpc_pipelines (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL UNIQUE,
                    description TEXT,
                    steps_json TEXT NOT NULL,
                    zero_context INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                -- External memory provider configs
                CREATE TABLE IF NOT EXISTS memory_providers (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL UNIQUE,
                    provider_type TEXT NOT NULL,
                    config_json TEXT NOT NULL DEFAULT '{}',
                    enabled INTEGER NOT NULL DEFAULT 0,
                    last_sync_at TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                -- Tool gateway configs
                CREATE TABLE IF NOT EXISTS tool_gateway_entries (
                    id TEXT PRIMARY KEY,
                    tool_type TEXT NOT NULL,
                    name TEXT NOT NULL UNIQUE,
                    config_json TEXT NOT NULL DEFAULT '{}',
                    enabled INTEGER NOT NULL DEFAULT 1,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                UPDATE schema_version SET version = 6;",
            )?;
        }

        if version < 7 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS engine_runs (
                    id TEXT PRIMARY KEY,
                    parent_run_id TEXT,
                    thread_id TEXT,
                    session_id TEXT,
                    title TEXT NOT NULL,
                    input_summary TEXT,
                    status TEXT NOT NULL DEFAULT 'pending',
                    phase TEXT NOT NULL DEFAULT 'queued',
                    cwd TEXT,
                    model TEXT,
                    provider TEXT,
                    retry_count INTEGER NOT NULL DEFAULT 0,
                    resumed_from_run_id TEXT,
                    checkpoint_json TEXT,
                    result_summary TEXT,
                    error TEXT,
                    metadata_json TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    started_at TEXT,
                    ended_at TEXT,
                    canceled_at TEXT,
                    FOREIGN KEY(parent_run_id) REFERENCES engine_runs(id) ON DELETE SET NULL,
                    FOREIGN KEY(thread_id) REFERENCES chat_threads(id) ON DELETE SET NULL,
                    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE SET NULL,
                    FOREIGN KEY(resumed_from_run_id) REFERENCES engine_runs(id) ON DELETE SET NULL
                );

                CREATE INDEX IF NOT EXISTS idx_engine_runs_status
                    ON engine_runs(status, updated_at DESC);
                CREATE INDEX IF NOT EXISTS idx_engine_runs_parent
                    ON engine_runs(parent_run_id, created_at DESC);
                CREATE INDEX IF NOT EXISTS idx_engine_runs_session
                    ON engine_runs(session_id, created_at DESC);

                CREATE TABLE IF NOT EXISTS engine_run_events (
                    id TEXT PRIMARY KEY,
                    run_id TEXT NOT NULL,
                    event_type TEXT NOT NULL,
                    payload_json TEXT,
                    created_at TEXT NOT NULL,
                    FOREIGN KEY(run_id) REFERENCES engine_runs(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_engine_run_events_run
                    ON engine_run_events(run_id, created_at DESC);

                CREATE TABLE IF NOT EXISTS engine_run_checkpoints (
                    id TEXT PRIMARY KEY,
                    run_id TEXT NOT NULL,
                    label TEXT NOT NULL,
                    snapshot_json TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    FOREIGN KEY(run_id) REFERENCES engine_runs(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_engine_run_checkpoints_run
                    ON engine_run_checkpoints(run_id, created_at DESC);

                CREATE TABLE IF NOT EXISTS runtime_instructions (
                    id TEXT PRIMARY KEY,
                    scope_type TEXT NOT NULL,
                    scope_ref TEXT,
                    title TEXT NOT NULL,
                    content TEXT NOT NULL,
                    enabled INTEGER NOT NULL DEFAULT 1,
                    priority INTEGER NOT NULL DEFAULT 100,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE INDEX IF NOT EXISTS idx_runtime_instructions_scope
                    ON runtime_instructions(scope_type, scope_ref, enabled, priority DESC);

                UPDATE schema_version SET version = 7;",
            )?;
        }

        if version < 8 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS worker_sandboxes (
                    id TEXT PRIMARY KEY,
                    run_id TEXT NOT NULL UNIQUE,
                    parent_run_id TEXT,
                    backend_id TEXT,
                    status TEXT NOT NULL DEFAULT 'active',
                    mode TEXT NOT NULL DEFAULT 'workspace_copy',
                    source_cwd TEXT NOT NULL,
                    workspace_root TEXT NOT NULL,
                    allowed_roots_json TEXT NOT NULL,
                    read_only_roots_json TEXT,
                    allow_file_read INTEGER NOT NULL DEFAULT 1,
                    allow_file_write INTEGER NOT NULL DEFAULT 1,
                    allow_shell_execution INTEGER NOT NULL DEFAULT 1,
                    allow_web_fetch INTEGER NOT NULL DEFAULT 0,
                    allow_web_search INTEGER NOT NULL DEFAULT 0,
                    allow_mcp INTEGER NOT NULL DEFAULT 0,
                    env_json TEXT,
                    metadata_json TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    ended_at TEXT,
                    FOREIGN KEY(run_id) REFERENCES engine_runs(id) ON DELETE CASCADE,
                    FOREIGN KEY(parent_run_id) REFERENCES engine_runs(id) ON DELETE SET NULL,
                    FOREIGN KEY(backend_id) REFERENCES terminal_backends(id) ON DELETE SET NULL
                );

                CREATE INDEX IF NOT EXISTS idx_worker_sandboxes_status
                    ON worker_sandboxes(status, updated_at DESC);
                CREATE INDEX IF NOT EXISTS idx_worker_sandboxes_parent
                    ON worker_sandboxes(parent_run_id, created_at DESC);

                UPDATE schema_version SET version = 8;",
            )?;
        }

        if version < 9 {
            conn.execute_batch(
                "ALTER TABLE scheduled_tasks ADD COLUMN task_kind TEXT NOT NULL DEFAULT 'prompt';
                ALTER TABLE scheduled_tasks ADD COLUMN crew_id TEXT;
                ALTER TABLE scheduled_tasks ADD COLUMN crew_snapshot_json TEXT;

                UPDATE schema_version SET version = 9;",
            )?;
        }

        if version < 10 {
            conn.execute_batch(
                "ALTER TABLE scheduled_tasks ADD COLUMN priority INTEGER NOT NULL DEFAULT 100;
                ALTER TABLE scheduled_tasks ADD COLUMN depends_on_task_ids_json TEXT NOT NULL DEFAULT '[]';

                UPDATE schema_version SET version = 10;"
            )?;
        }

        if version < 11 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS crew_runs (
                    id TEXT PRIMARY KEY,
                    crew_id TEXT NOT NULL,
                    crew_name TEXT NOT NULL,
                    process TEXT NOT NULL,
                    status TEXT NOT NULL,
                    manager_agent_id TEXT,
                    error TEXT,
                    started_at TEXT NOT NULL,
                    finished_at TEXT,
                    created_at TEXT NOT NULL
                );

                CREATE INDEX IF NOT EXISTS idx_crew_runs_crew
                    ON crew_runs(crew_id, started_at DESC);

                CREATE TABLE IF NOT EXISTS crew_run_logs (
                    id TEXT PRIMARY KEY,
                    run_id TEXT NOT NULL,
                    crew_id TEXT NOT NULL,
                    agent_id TEXT NOT NULL,
                    task_id TEXT NOT NULL,
                    action TEXT NOT NULL,
                    result TEXT NOT NULL,
                    timestamp INTEGER NOT NULL,
                    created_at TEXT NOT NULL,
                    FOREIGN KEY(run_id) REFERENCES crew_runs(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_crew_run_logs_run
                    ON crew_run_logs(run_id, timestamp ASC);

                UPDATE schema_version SET version = 11;",
            )?;
        }

        if version < 12 {
            conn.execute_batch(
                "ALTER TABLE crew_runs ADD COLUMN crew_snapshot_json TEXT NOT NULL DEFAULT '{}';

                UPDATE schema_version SET version = 12;",
            )?;
        }

        if version < 13 {
            conn.execute_batch(
                "ALTER TABLE scheduled_tasks ADD COLUMN model_config_json TEXT;

                UPDATE schema_version SET version = 13;",
            )?;
        }

        if version < 14 {
            conn.execute_batch(
                "ALTER TABLE chat_threads ADD COLUMN provider_settings_json TEXT;

                UPDATE schema_version SET version = 14;",
            )?;
        }

        if version < 15 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS policy_tool_states (
                    tool_id TEXT PRIMARY KEY,
                    enabled INTEGER NOT NULL,
                    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
                );

                UPDATE schema_version SET version = 15;",
            )?;
        }

        if version < 16 {
            conn.execute_batch(
                "ALTER TABLE chat_threads ADD COLUMN permission_config_json TEXT;\n
                UPDATE schema_version SET version = 16;",
            )?;
        }

        if version < 17 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS crew_definitions (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    description TEXT NOT NULL DEFAULT '',
                    definition_json TEXT NOT NULL,
                    flow_json TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS crew_definition_versions (
                    id TEXT PRIMARY KEY,
                    crew_id TEXT NOT NULL,
                    version_number INTEGER NOT NULL,
                    change_summary TEXT,
                    definition_json TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    FOREIGN KEY(crew_id) REFERENCES crew_definitions(id) ON DELETE CASCADE
                );

                CREATE UNIQUE INDEX IF NOT EXISTS idx_crew_definition_versions_unique
                    ON crew_definition_versions(crew_id, version_number);

                CREATE TABLE IF NOT EXISTS crew_role_bindings (
                    id TEXT PRIMARY KEY,
                    scope_type TEXT NOT NULL,
                    scope_ref TEXT,
                    role TEXT NOT NULL,
                    subject TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE INDEX IF NOT EXISTS idx_crew_role_bindings_scope
                    ON crew_role_bindings(scope_type, scope_ref, role);

                CREATE TABLE IF NOT EXISTS crew_approvals (
                    id TEXT PRIMARY KEY,
                    crew_id TEXT,
                    run_id TEXT,
                    approval_type TEXT NOT NULL,
                    scope_ref TEXT,
                    status TEXT NOT NULL,
                    requested_by TEXT,
                    resolved_by TEXT,
                    payload_json TEXT,
                    resolution_note TEXT,
                    requested_at TEXT NOT NULL,
                    resolved_at TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    FOREIGN KEY(crew_id) REFERENCES crew_definitions(id) ON DELETE SET NULL,
                    FOREIGN KEY(run_id) REFERENCES crew_runs(id) ON DELETE SET NULL
                );

                CREATE INDEX IF NOT EXISTS idx_crew_approvals_status
                    ON crew_approvals(status, requested_at DESC);
                CREATE INDEX IF NOT EXISTS idx_crew_approvals_run
                    ON crew_approvals(run_id, requested_at DESC);

                CREATE TABLE IF NOT EXISTS crew_run_events (
                    id TEXT PRIMARY KEY,
                    run_id TEXT NOT NULL,
                    crew_id TEXT NOT NULL,
                    event_type TEXT NOT NULL,
                    payload_json TEXT,
                    created_at TEXT NOT NULL,
                    FOREIGN KEY(run_id) REFERENCES crew_runs(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_crew_run_events_run
                    ON crew_run_events(run_id, created_at DESC);
                CREATE INDEX IF NOT EXISTS idx_crew_run_events_crew
                    ON crew_run_events(crew_id, created_at DESC);

                UPDATE schema_version SET version = 17;",
            )?;
        }

        if version < 18 {
            add_column_if_missing(
                &conn,
                "agent_personalities",
                "role",
                "role TEXT NOT NULL DEFAULT 'custom'",
            )?;
            add_column_if_missing(
                &conn,
                "agent_personalities",
                "goal",
                "goal TEXT NOT NULL DEFAULT ''",
            )?;
            add_column_if_missing(
                &conn,
                "agent_personalities",
                "skills_markdown",
                "skills_markdown TEXT NOT NULL DEFAULT ''",
            )?;
            conn.execute(
                "UPDATE agent_personalities
                 SET goal = description
                 WHERE TRIM(COALESCE(goal, '')) = ''",
                [],
            )?;
            conn.execute("UPDATE schema_version SET version = 18", [])?;
        }

        if version < 19 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS projects (
                    id TEXT PRIMARY KEY,
                    title TEXT NOT NULL,
                    instructions TEXT NOT NULL DEFAULT '',
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS project_resources (
                    id TEXT PRIMARY KEY,
                    project_id TEXT NOT NULL,
                    kind TEXT NOT NULL CHECK(kind IN ('file', 'folder', 'link')),
                    path TEXT NOT NULL,
                    label TEXT,
                    enabled INTEGER NOT NULL DEFAULT 1,
                    added_at TEXT NOT NULL,
                    FOREIGN KEY(project_id) REFERENCES projects(id) ON DELETE CASCADE,
                    UNIQUE(project_id, kind, path)
                );

                CREATE INDEX IF NOT EXISTS idx_project_resources_project
                    ON project_resources(project_id, added_at DESC);

                CREATE TABLE IF NOT EXISTS project_threads (
                    project_id TEXT NOT NULL,
                    thread_id TEXT NOT NULL UNIQUE,
                    added_at TEXT NOT NULL,
                    PRIMARY KEY(project_id, thread_id),
                    FOREIGN KEY(project_id) REFERENCES projects(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_project_threads_project
                    ON project_threads(project_id, added_at DESC);

                UPDATE schema_version SET version = 19;",
            )?;
        }

        if version < 20 {
            add_column_if_missing(
                &conn,
                "crew_run_logs",
                "metadata_json",
                "metadata_json TEXT",
            )?;
            conn.execute("UPDATE schema_version SET version = 20", [])?;
        }

        if version < 21 {
            add_column_if_missing(
                &conn,
                "engine_runs",
                "source",
                "source TEXT NOT NULL DEFAULT 'desktop'",
            )?;
            add_column_if_missing(
                &conn,
                "engine_runs",
                "workspace_path",
                "workspace_path TEXT",
            )?;
            add_column_if_missing(
                &conn,
                "engine_runs",
                "provider_profile_id",
                "provider_profile_id TEXT",
            )?;
            add_column_if_missing(
                &conn,
                "engine_runs",
                "runtime_mode",
                "runtime_mode TEXT NOT NULL DEFAULT 'host'",
            )?;
            add_column_if_missing(
                &conn,
                "engine_runs",
                "toolset_policy_id",
                "toolset_policy_id TEXT",
            )?;
            add_column_if_missing(&conn, "engine_runs", "channel_kind", "channel_kind TEXT")?;
            add_column_if_missing(&conn, "engine_runs", "channel_ref", "channel_ref TEXT")?;
            add_column_if_missing(&conn, "engine_run_events", "sequence", "sequence INTEGER")?;
            add_column_if_missing(
                &conn,
                "engine_run_events",
                "summary",
                "summary TEXT NOT NULL DEFAULT ''",
            )?;
            add_column_if_missing(
                &conn,
                "engine_run_events",
                "redaction_level",
                "redaction_level TEXT NOT NULL DEFAULT 'none'",
            )?;
            conn.execute(
                "UPDATE engine_run_events SET sequence = rowid WHERE sequence IS NULL",
                [],
            )?;
            conn.execute_batch(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_engine_run_events_run_sequence
                    ON engine_run_events(run_id, sequence);

                CREATE TABLE IF NOT EXISTS engine_run_artifacts (
                    id TEXT PRIMARY KEY,
                    run_id TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    path TEXT NOT NULL,
                    title TEXT,
                    summary TEXT,
                    created_at TEXT NOT NULL,
                    FOREIGN KEY(run_id) REFERENCES engine_runs(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_engine_run_artifacts_run
                    ON engine_run_artifacts(run_id, created_at DESC);

                UPDATE schema_version SET version = 21;",
            )?;
        }

        if version < 22 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS policy_settings (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL,
                    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
                );

                UPDATE schema_version SET version = 22;",
            )?;
        }

        if version < 23 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS work_tasks (
                    id TEXT PRIMARY KEY,
                    title TEXT NOT NULL DEFAULT '',
                    prompt TEXT NOT NULL,
                    expected_output TEXT NOT NULL DEFAULT '',
                    work_dir TEXT NOT NULL DEFAULT '',
                    thread_id TEXT,
                    runner TEXT NOT NULL CHECK(runner IN ('crew', 'model')),
                    crew_id TEXT,
                    model TEXT NOT NULL DEFAULT '',
                    schedule_expr TEXT NOT NULL DEFAULT '',
                    schedule_enabled INTEGER NOT NULL DEFAULT 0,
                    status TEXT NOT NULL DEFAULT 'idle',
                    output TEXT,
                    error TEXT,
                    last_run_at TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    FOREIGN KEY(thread_id) REFERENCES chat_threads(id) ON DELETE SET NULL
                );

                CREATE INDEX IF NOT EXISTS idx_work_tasks_created
                    ON work_tasks(created_at DESC);
                CREATE INDEX IF NOT EXISTS idx_work_tasks_status
                    ON work_tasks(status, updated_at DESC);
                CREATE INDEX IF NOT EXISTS idx_work_tasks_thread
                    ON work_tasks(thread_id);

                UPDATE schema_version SET version = 23;",
            )?;
        }

        transaction.commit()
    }

    // -- Projects --

    pub fn upsert_project(
        &self,
        id: &str,
        title: &str,
        instructions: &str,
        created_at: &str,
        updated_at: &str,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO projects (id, title, instructions, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
               title = excluded.title,
               instructions = excluded.instructions,
               updated_at = excluded.updated_at",
            params![id, title, instructions, created_at, updated_at],
        )?;
        Ok(())
    }

    pub fn list_projects(&self) -> SqlResult<Vec<ProjectRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, title, instructions, created_at, updated_at
             FROM projects ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ProjectRow {
                id: row.get(0)?,
                title: row.get(1)?,
                instructions: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    pub fn delete_project(&self, project_id: &str, delete_threads: bool) -> SqlResult<Vec<String>> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let tx = conn.transaction()?;
        let thread_ids = {
            let mut stmt = tx.prepare(
                "SELECT thread_id FROM project_threads WHERE project_id = ?1 ORDER BY added_at DESC",
            )?;
            let rows = stmt.query_map(params![project_id], |row| row.get::<_, String>(0))?;
            rows.collect::<SqlResult<Vec<_>>>()?
        };

        tx.execute("DELETE FROM projects WHERE id = ?1", params![project_id])?;

        if delete_threads {
            for thread_id in &thread_ids {
                tx.execute("DELETE FROM chat_threads WHERE id = ?1", params![thread_id])?;
            }
        }

        tx.commit()?;
        Ok(if delete_threads {
            thread_ids
        } else {
            Vec::new()
        })
    }

    pub fn upsert_project_resource(
        &self,
        id: &str,
        project_id: &str,
        kind: &str,
        path: &str,
        label: Option<&str>,
        enabled: bool,
        added_at: &str,
    ) -> SqlResult<()> {
        if !matches!(kind, "file" | "folder" | "link") {
            return Err(rusqlite::Error::InvalidQuery);
        }

        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO project_resources (id, project_id, kind, path, label, enabled, added_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(project_id, kind, path) DO UPDATE SET
               label = excluded.label,
               enabled = excluded.enabled",
            params![
                id,
                project_id,
                kind,
                path,
                label,
                if enabled { 1 } else { 0 },
                added_at
            ],
        )?;
        conn.execute(
            "UPDATE projects SET updated_at = datetime('now') WHERE id = ?1",
            params![project_id],
        )?;
        Ok(())
    }

    pub fn list_project_resources(&self) -> SqlResult<Vec<ProjectResourceRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, project_id, kind, path, label, enabled, added_at
             FROM project_resources ORDER BY added_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ProjectResourceRow {
                id: row.get(0)?,
                project_id: row.get(1)?,
                kind: row.get(2)?,
                path: row.get(3)?,
                label: row.get(4)?,
                enabled: row.get::<_, i64>(5)? != 0,
                added_at: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    pub fn delete_project_resource(&self, resource_id: &str) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "DELETE FROM project_resources WHERE id = ?1",
            params![resource_id],
        )?;
        Ok(())
    }

    pub fn set_project_resource_enabled(&self, resource_id: &str, enabled: bool) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE project_resources SET enabled = ?2 WHERE id = ?1",
            params![resource_id, if enabled { 1 } else { 0 }],
        )?;
        Ok(())
    }

    pub fn attach_project_thread(&self, project_id: &str, thread_id: &str) -> SqlResult<()> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM project_threads WHERE thread_id = ?1",
            params![thread_id],
        )?;
        tx.execute(
            "INSERT INTO project_threads (project_id, thread_id, added_at)
             VALUES (?1, ?2, datetime('now'))",
            params![project_id, thread_id],
        )?;
        tx.execute(
            "UPDATE projects SET updated_at = datetime('now') WHERE id = ?1",
            params![project_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn detach_project_thread(&self, project_id: &str, thread_id: &str) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "DELETE FROM project_threads WHERE project_id = ?1 AND thread_id = ?2",
            params![project_id, thread_id],
        )?;
        conn.execute(
            "UPDATE projects SET updated_at = datetime('now') WHERE id = ?1",
            params![project_id],
        )?;
        Ok(())
    }

    pub fn list_project_threads(&self) -> SqlResult<Vec<(String, String)>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn
            .prepare("SELECT project_id, thread_id FROM project_threads ORDER BY added_at DESC")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    }

    // -- Chat Threads --

    pub fn insert_thread(
        &self,
        id: &str,
        title: &str,
        created_at: &str,
        provider_settings_json: Option<&str>,
        permission_config_json: Option<&str>,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO chat_threads (id, title, created_at, updated_at, provider_settings_json, permission_config_json) VALUES (?1, ?2, ?3, ?3, ?4, ?5)",
            params![id, title, created_at, provider_settings_json, permission_config_json],
        )?;
        Ok(())
    }

    pub fn list_threads(
        &self,
    ) -> SqlResult<
        Vec<(
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
        )>,
    > {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, title, created_at, updated_at, provider_settings_json, permission_config_json FROM chat_threads ORDER BY updated_at DESC"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })?;
        rows.collect()
    }

    pub fn update_thread_provider_settings(
        &self,
        id: &str,
        provider_settings_json: Option<&str>,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE chat_threads SET provider_settings_json = ?2, updated_at = datetime('now') WHERE id = ?1",
            params![id, provider_settings_json],
        )?;
        Ok(())
    }

    pub fn update_thread_permission_config(
        &self,
        id: &str,
        permission_config_json: Option<&str>,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE chat_threads SET permission_config_json = ?2, updated_at = datetime('now') WHERE id = ?1",
            params![id, permission_config_json],
        )?;
        Ok(())
    }

    pub fn delete_thread(&self, id: &str) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "DELETE FROM project_threads WHERE thread_id = ?1",
            params![id],
        )?;
        conn.execute("DELETE FROM chat_threads WHERE id = ?1", params![id])?;
        Ok(())
    }

    // -- Chat Messages --

    pub fn insert_message(
        &self,
        id: &str,
        thread_id: &str,
        role: &str,
        content: &str,
        timestamp: i64,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO chat_messages (id, thread_id, role, content, timestamp) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, thread_id, role, content, timestamp],
        )?;
        conn.execute(
            "UPDATE chat_threads SET updated_at = datetime('now') WHERE id = ?1",
            params![thread_id],
        )?;
        Ok(())
    }

    pub fn list_messages(&self, thread_id: &str) -> SqlResult<Vec<(String, String, String, i64)>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, role, content, timestamp FROM chat_messages WHERE thread_id = ?1 ORDER BY timestamp"
        )?;
        let rows = stmt.query_map(params![thread_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
        rows.collect()
    }

    pub fn insert_crew_run(
        &self,
        id: &str,
        crew_id: &str,
        crew_name: &str,
        process: &str,
        status: &str,
        manager_agent_id: Option<&str>,
        error: Option<&str>,
        crew_snapshot_json: &str,
        started_at: &str,
        finished_at: Option<&str>,
    ) -> SqlResult<()> {
        let error = redact_and_bound_optional_text(error, MAX_LOG_TEXT_BYTES);
        let crew_snapshot_json = redact_and_bound_json_text(crew_snapshot_json, MAX_LOG_JSON_BYTES);
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO crew_runs (id, crew_id, crew_name, process, status, manager_agent_id, error, crew_snapshot_json, started_at, finished_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, datetime('now'))
             ON CONFLICT(id) DO UPDATE SET
               crew_id = excluded.crew_id,
               crew_name = excluded.crew_name,
               process = excluded.process,
               status = excluded.status,
               manager_agent_id = excluded.manager_agent_id,
               error = excluded.error,
               crew_snapshot_json = excluded.crew_snapshot_json,
               started_at = excluded.started_at,
               finished_at = excluded.finished_at",
            params![id, crew_id, crew_name, process, status, manager_agent_id, error, crew_snapshot_json, started_at, finished_at],
        )?;
        Ok(())
    }

    pub fn insert_crew_run_logs(
        &self,
        run_id: &str,
        logs: &[crate::CrewExecutionLogRow],
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "INSERT INTO crew_run_logs (id, run_id, crew_id, agent_id, task_id, action, result, timestamp, metadata_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, datetime('now'))"
        )?;

        for log in logs {
            let action = redact_and_bound_text(&log.action, MAX_LOG_SUMMARY_BYTES);
            let result = redact_and_bound_text(&log.result, MAX_LOG_TEXT_BYTES);
            let metadata_json = serde_json::to_string(log)
                .ok()
                .map(|value| redact_and_bound_json_text(&value, MAX_LOG_JSON_BYTES));
            stmt.execute(params![
                log.id,
                run_id,
                log.crew_id,
                log.agent_id,
                log.task_id,
                action,
                result,
                log.timestamp,
                metadata_json,
            ])?;
        }

        conn.execute(
            "DELETE FROM crew_run_logs
             WHERE run_id = ?1
               AND id NOT IN (
                 SELECT id FROM crew_run_logs
                 WHERE run_id = ?1
                 ORDER BY timestamp DESC, rowid DESC
                 LIMIT ?2
               )",
            params![run_id, MAX_CREW_LOGS_PER_RUN],
        )?;

        Ok(())
    }

    pub fn list_crew_runs(
        &self,
        crew_id: Option<&str>,
        limit: i64,
    ) -> SqlResult<
        Vec<(
            String,
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
            Option<String>,
        )>,
    > {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = if crew_id.is_some() {
            conn.prepare(
                "SELECT id, crew_id, crew_name, process, status, manager_agent_id, error, started_at, finished_at
                 FROM crew_runs
                 WHERE crew_id = ?1
                 ORDER BY started_at DESC
                 LIMIT ?2"
            )?
        } else {
            conn.prepare(
                "SELECT id, crew_id, crew_name, process, status, manager_agent_id, error, started_at, finished_at
                 FROM crew_runs
                 ORDER BY started_at DESC
                 LIMIT ?1"
            )?
        };

        if let Some(id) = crew_id {
            let rows = stmt.query_map(params![id, limit], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            })?;
            rows.collect()
        } else {
            let rows = stmt.query_map(params![limit], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            })?;
            rows.collect()
        }
    }

    pub fn list_crew_run_logs(&self, run_id: &str) -> SqlResult<Vec<crate::CrewExecutionLogRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, crew_id, agent_id, task_id, action, result, timestamp, metadata_json
             FROM crew_run_logs
             WHERE run_id = ?1
             ORDER BY timestamp ASC",
        )?;

        let rows = stmt.query_map(params![run_id], |row| {
            let metadata_json: Option<String> = row.get(7)?;
            if let Some(metadata) = metadata_json
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                if let Ok(log) = serde_json::from_str::<crate::CrewExecutionLogRow>(metadata) {
                    return Ok(log);
                }
            }

            Ok(crate::CrewExecutionLogRow {
                id: row.get(0)?,
                crew_id: row.get(1)?,
                agent_id: row.get(2)?,
                task_id: row.get(3)?,
                action: row.get(4)?,
                result: row.get(5)?,
                timestamp: row.get(6)?,
                agent_name: None,
                source_agent: None,
                target_agent: None,
                provider: None,
                model: None,
                task_title: None,
                phase: None,
                summary: None,
                detail: None,
                severity: None,
                provider_reasoning: None,
            })
        })?;

        rows.collect()
    }

    pub fn get_crew_run_snapshot(&self, run_id: &str) -> SqlResult<Option<String>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.query_row(
            "SELECT crew_snapshot_json FROM crew_runs WHERE id = ?1 LIMIT 1",
            params![run_id],
            |row| row.get(0),
        )
        .optional()
    }

    pub fn upsert_crew_definition(
        &self,
        id: &str,
        name: &str,
        description: &str,
        definition_json: &str,
        flow_json: Option<&str>,
        change_summary: Option<&str>,
    ) -> SqlResult<CrewDefinitionRow> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO crew_definitions (id, name, description, definition_json, flow_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'), datetime('now'))
             ON CONFLICT(id) DO UPDATE SET
               name = excluded.name,
               description = excluded.description,
               definition_json = excluded.definition_json,
               flow_json = excluded.flow_json,
               updated_at = datetime('now')",
            params![id, name, description, definition_json, flow_json],
        )?;

        let current_version = tx
            .query_row(
                "SELECT MAX(version_number) FROM crew_definition_versions WHERE crew_id = ?1",
                params![id],
                |row| row.get::<_, Option<i32>>(0),
            )?
            .unwrap_or(0);
        let next_version = current_version + 1;

        tx.execute(
            "INSERT INTO crew_definition_versions (id, crew_id, version_number, change_summary, definition_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
            params![uuid::Uuid::new_v4().to_string(), id, next_version, change_summary, definition_json],
        )?;

        let row = tx.query_row(
            "SELECT d.id, d.name, d.description, d.definition_json, d.flow_json,
                    COALESCE((SELECT MAX(v.version_number) FROM crew_definition_versions v WHERE v.crew_id = d.id), 0),
                    d.created_at, d.updated_at
             FROM crew_definitions d
             WHERE d.id = ?1",
            params![id],
            |row| {
                Ok(CrewDefinitionRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                    definition_json: row.get(3)?,
                    flow_json: row.get(4)?,
                    version_count: row.get(5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            },
        )?;

        tx.commit()?;
        Ok(row)
    }

    pub fn list_crew_definitions(&self) -> SqlResult<Vec<CrewDefinitionRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT d.id, d.name, d.description, d.definition_json, d.flow_json,
                    COALESCE((SELECT MAX(v.version_number) FROM crew_definition_versions v WHERE v.crew_id = d.id), 0),
                    d.created_at, d.updated_at
             FROM crew_definitions d
             ORDER BY d.updated_at DESC"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(CrewDefinitionRow {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                definition_json: row.get(3)?,
                flow_json: row.get(4)?,
                version_count: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })?;
        rows.collect()
    }

    pub fn list_crew_definition_versions(
        &self,
        crew_id: &str,
    ) -> SqlResult<Vec<CrewDefinitionVersionRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, crew_id, version_number, change_summary, definition_json, created_at
             FROM crew_definition_versions
             WHERE crew_id = ?1
             ORDER BY version_number DESC",
        )?;
        let rows = stmt.query_map(params![crew_id], |row| {
            Ok(CrewDefinitionVersionRow {
                id: row.get(0)?,
                crew_id: row.get(1)?,
                version_number: row.get(2)?,
                change_summary: row.get(3)?,
                definition_json: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        rows.collect()
    }

    pub fn upsert_crew_role_binding(
        &self,
        id: &str,
        scope_type: &str,
        scope_ref: Option<&str>,
        role: &str,
        subject: &str,
    ) -> SqlResult<CrewRoleBindingRow> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO crew_role_bindings (id, scope_type, scope_ref, role, subject, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'), datetime('now'))
             ON CONFLICT(id) DO UPDATE SET
               scope_type = excluded.scope_type,
               scope_ref = excluded.scope_ref,
               role = excluded.role,
               subject = excluded.subject,
               updated_at = datetime('now')",
            params![id, scope_type, scope_ref, role, subject],
        )?;

        conn.query_row(
            "SELECT id, scope_type, scope_ref, role, subject, created_at, updated_at
             FROM crew_role_bindings WHERE id = ?1",
            params![id],
            |row| {
                Ok(CrewRoleBindingRow {
                    id: row.get(0)?,
                    scope_type: row.get(1)?,
                    scope_ref: row.get(2)?,
                    role: row.get(3)?,
                    subject: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            },
        )
    }

    pub fn list_crew_role_bindings(
        &self,
        scope_type: Option<&str>,
        scope_ref: Option<&str>,
    ) -> SqlResult<Vec<CrewRoleBindingRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, scope_type, scope_ref, role, subject, created_at, updated_at
             FROM crew_role_bindings
             WHERE (?1 IS NULL OR scope_type = ?1)
               AND (?2 IS NULL OR scope_ref = ?2)
             ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map(params![scope_type, scope_ref], |row| {
            Ok(CrewRoleBindingRow {
                id: row.get(0)?,
                scope_type: row.get(1)?,
                scope_ref: row.get(2)?,
                role: row.get(3)?,
                subject: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    pub fn insert_crew_approval(
        &self,
        id: &str,
        crew_id: Option<&str>,
        run_id: Option<&str>,
        approval_type: &str,
        scope_ref: Option<&str>,
        status: &str,
        requested_by: Option<&str>,
        payload_json: Option<&str>,
    ) -> SqlResult<CrewApprovalRow> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO crew_approvals (
                id, crew_id, run_id, approval_type, scope_ref, status,
                requested_by, resolved_by, payload_json, resolution_note,
                requested_at, resolved_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8, NULL, datetime('now'), NULL, datetime('now'), datetime('now'))",
            params![id, crew_id, run_id, approval_type, scope_ref, status, requested_by, payload_json],
        )?;

        conn.query_row(
            "SELECT id, crew_id, run_id, approval_type, scope_ref, status, requested_by, resolved_by, payload_json, resolution_note, requested_at, resolved_at, created_at, updated_at
             FROM crew_approvals WHERE id = ?1",
            params![id],
            |row| {
                Ok(CrewApprovalRow {
                    id: row.get(0)?,
                    crew_id: row.get(1)?,
                    run_id: row.get(2)?,
                    approval_type: row.get(3)?,
                    scope_ref: row.get(4)?,
                    status: row.get(5)?,
                    requested_by: row.get(6)?,
                    resolved_by: row.get(7)?,
                    payload_json: row.get(8)?,
                    resolution_note: row.get(9)?,
                    requested_at: row.get(10)?,
                    resolved_at: row.get(11)?,
                    created_at: row.get(12)?,
                    updated_at: row.get(13)?,
                })
            },
        )
    }

    pub fn get_crew_approval(&self, id: &str) -> SqlResult<Option<CrewApprovalRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.query_row(
            "SELECT id, crew_id, run_id, approval_type, scope_ref, status, requested_by, resolved_by, payload_json, resolution_note, requested_at, resolved_at, created_at, updated_at
             FROM crew_approvals WHERE id = ?1",
            params![id],
            |row| {
                Ok(CrewApprovalRow {
                    id: row.get(0)?,
                    crew_id: row.get(1)?,
                    run_id: row.get(2)?,
                    approval_type: row.get(3)?,
                    scope_ref: row.get(4)?,
                    status: row.get(5)?,
                    requested_by: row.get(6)?,
                    resolved_by: row.get(7)?,
                    payload_json: row.get(8)?,
                    resolution_note: row.get(9)?,
                    requested_at: row.get(10)?,
                    resolved_at: row.get(11)?,
                    created_at: row.get(12)?,
                    updated_at: row.get(13)?,
                })
            },
        ).optional()
    }

    pub fn resolve_crew_approval(
        &self,
        id: &str,
        status: &str,
        resolved_by: Option<&str>,
        resolution_note: Option<&str>,
    ) -> SqlResult<CrewApprovalRow> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE crew_approvals
             SET status = ?2,
                 resolved_by = ?3,
                 resolution_note = ?4,
                 resolved_at = datetime('now'),
                 updated_at = datetime('now')
             WHERE id = ?1",
            params![id, status, resolved_by, resolution_note],
        )?;

        conn.query_row(
            "SELECT id, crew_id, run_id, approval_type, scope_ref, status, requested_by, resolved_by, payload_json, resolution_note, requested_at, resolved_at, created_at, updated_at
             FROM crew_approvals WHERE id = ?1",
            params![id],
            |row| {
                Ok(CrewApprovalRow {
                    id: row.get(0)?,
                    crew_id: row.get(1)?,
                    run_id: row.get(2)?,
                    approval_type: row.get(3)?,
                    scope_ref: row.get(4)?,
                    status: row.get(5)?,
                    requested_by: row.get(6)?,
                    resolved_by: row.get(7)?,
                    payload_json: row.get(8)?,
                    resolution_note: row.get(9)?,
                    requested_at: row.get(10)?,
                    resolved_at: row.get(11)?,
                    created_at: row.get(12)?,
                    updated_at: row.get(13)?,
                })
            },
        )
    }

    pub fn list_crew_approvals(
        &self,
        status: Option<&str>,
        crew_id: Option<&str>,
    ) -> SqlResult<Vec<CrewApprovalRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, crew_id, run_id, approval_type, scope_ref, status, requested_by, resolved_by, payload_json, resolution_note, requested_at, resolved_at, created_at, updated_at
             FROM crew_approvals
             WHERE (?1 IS NULL OR status = ?1)
               AND (?2 IS NULL OR crew_id = ?2)
             ORDER BY requested_at DESC"
        )?;
        let rows = stmt.query_map(params![status, crew_id], |row| {
            Ok(CrewApprovalRow {
                id: row.get(0)?,
                crew_id: row.get(1)?,
                run_id: row.get(2)?,
                approval_type: row.get(3)?,
                scope_ref: row.get(4)?,
                status: row.get(5)?,
                requested_by: row.get(6)?,
                resolved_by: row.get(7)?,
                payload_json: row.get(8)?,
                resolution_note: row.get(9)?,
                requested_at: row.get(10)?,
                resolved_at: row.get(11)?,
                created_at: row.get(12)?,
                updated_at: row.get(13)?,
            })
        })?;
        rows.collect()
    }

    pub fn insert_crew_run_event(
        &self,
        id: &str,
        run_id: &str,
        crew_id: &str,
        event_type: &str,
        payload_json: Option<&str>,
    ) -> SqlResult<()> {
        let payload_json = redact_and_bound_optional_json(payload_json, MAX_LOG_JSON_BYTES);
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO crew_run_events (id, run_id, crew_id, event_type, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
            params![id, run_id, crew_id, event_type, payload_json],
        )?;
        conn.execute(
            "DELETE FROM crew_run_events
             WHERE run_id = ?1
               AND id NOT IN (
                 SELECT id FROM crew_run_events
                 WHERE run_id = ?1
                 ORDER BY created_at DESC, rowid DESC
                 LIMIT ?2
               )",
            params![run_id, MAX_CREW_EVENTS_PER_RUN],
        )?;
        Ok(())
    }

    pub fn list_crew_run_events(
        &self,
        run_id: Option<&str>,
        crew_id: Option<&str>,
        limit: i64,
    ) -> SqlResult<Vec<CrewRunEventRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, run_id, crew_id, event_type, payload_json, created_at
             FROM crew_run_events
             WHERE (?1 IS NULL OR run_id = ?1)
               AND (?2 IS NULL OR crew_id = ?2)
             ORDER BY created_at DESC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![run_id, crew_id, limit], |row| {
            Ok(CrewRunEventRow {
                id: row.get(0)?,
                run_id: row.get(1)?,
                crew_id: row.get(2)?,
                event_type: row.get(3)?,
                payload_json: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        rows.collect()
    }

    pub fn update_message_content(&self, id: &str, content: &str) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE chat_messages SET content = ?2 WHERE id = ?1",
            params![id, content],
        )?;
        Ok(())
    }

    pub fn delete_messages(&self, ids: &[String]) -> SqlResult<usize> {
        if ids.is_empty() {
            return Ok(0);
        }

        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(", ");

        let thread_query = format!(
            "SELECT DISTINCT thread_id FROM chat_messages WHERE id IN ({})",
            placeholders
        );
        let mut stmt = conn.prepare(&thread_query)?;
        let thread_rows =
            stmt.query_map(params_from_iter(ids.iter()), |row| row.get::<_, String>(0))?;
        let thread_ids: Vec<String> = thread_rows.collect::<SqlResult<Vec<_>>>()?;
        drop(stmt);

        let delete_query = format!("DELETE FROM chat_messages WHERE id IN ({})", placeholders);
        let deleted = conn.execute(&delete_query, params_from_iter(ids.iter()))?;

        for thread_id in thread_ids {
            conn.execute(
                "UPDATE chat_threads SET updated_at = datetime('now') WHERE id = ?1",
                params![thread_id],
            )?;
        }

        Ok(deleted)
    }

    // -- Tasks --

    pub fn insert_task(
        &self,
        id: &str,
        title: &str,
        prompt: &str,
        status: &str,
        thread_id: Option<&str>,
        created_at: &str,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO tasks (id, title, prompt, status, thread_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![id, title, prompt, status, thread_id, created_at],
        )?;
        Ok(())
    }

    pub fn update_task_status(&self, id: &str, status: &str) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE tasks SET status = ?2, updated_at = datetime('now') WHERE id = ?1",
            params![id, status],
        )?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn set_task_error(&self, id: &str, error: &str) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE tasks SET error = ?2, status = 'failed', updated_at = datetime('now') WHERE id = ?1",
            params![id, error],
        )?;
        Ok(())
    }

    pub fn list_tasks(
        &self,
    ) -> SqlResult<
        Vec<(
            String,
            String,
            String,
            String,
            Option<String>,
            String,
            String,
            Option<String>,
        )>,
    > {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, title, prompt, status, thread_id, created_at, updated_at, error FROM tasks ORDER BY created_at DESC"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
            ))
        })?;
        rows.collect()
    }

    // -- Task Steps --

    pub fn insert_step(
        &self,
        id: &str,
        task_id: &str,
        idx: i32,
        title: &str,
        state: &str,
        requires_approval: bool,
        risk_level: &str,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO task_steps (id, task_id, idx, title, state, requires_approval, risk_level) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, task_id, idx, title, state, requires_approval as i32, risk_level],
        )?;
        Ok(())
    }

    pub fn update_step_state(&self, id: &str, state: &str, output: Option<&str>) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE task_steps SET state = ?2, output = ?3 WHERE id = ?1",
            params![id, state, output],
        )?;
        Ok(())
    }

    pub fn list_steps(
        &self,
        task_id: &str,
    ) -> SqlResult<Vec<(String, i32, String, String, bool, String, Option<String>)>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, idx, title, state, requires_approval, risk_level, output FROM task_steps WHERE task_id = ?1 ORDER BY idx"
        )?;
        let rows = stmt.query_map(params![task_id], |row| {
            let approval: i32 = row.get(4)?;
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                approval != 0,
                row.get(5)?,
                row.get(6)?,
            ))
        })?;
        rows.collect()
    }

    // -- Work Tasks --

    pub fn upsert_work_task(&self, task: &WorkTaskRow) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO work_tasks (
                id, title, prompt, expected_output, work_dir, thread_id, runner, crew_id, model,
                schedule_expr, schedule_enabled, status, output, error, last_run_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
             ON CONFLICT(id) DO UPDATE SET
                title = excluded.title,
                prompt = excluded.prompt,
                expected_output = excluded.expected_output,
                work_dir = excluded.work_dir,
                thread_id = excluded.thread_id,
                runner = excluded.runner,
                crew_id = excluded.crew_id,
                model = excluded.model,
                schedule_expr = excluded.schedule_expr,
                schedule_enabled = excluded.schedule_enabled,
                status = excluded.status,
                output = excluded.output,
                error = excluded.error,
                last_run_at = excluded.last_run_at,
                updated_at = excluded.updated_at",
            params![
                &task.id,
                &task.title,
                &task.prompt,
                &task.expected_output,
                &task.work_dir,
                &task.thread_id,
                &task.runner,
                &task.crew_id,
                &task.model,
                &task.schedule_expr,
                task.schedule_enabled as i32,
                &task.status,
                &task.output,
                &task.error,
                &task.last_run_at,
                &task.created_at,
                &task.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_work_task(&self, id: &str) -> SqlResult<Option<WorkTaskRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.query_row(
            "SELECT id, title, prompt, expected_output, work_dir, thread_id, runner, crew_id, model,
                    schedule_expr, schedule_enabled, status, output, error, last_run_at, created_at, updated_at
             FROM work_tasks
             WHERE id = ?1
             LIMIT 1",
            params![id],
            |row| {
                let schedule_enabled: i32 = row.get(10)?;
                Ok(WorkTaskRow {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    prompt: row.get(2)?,
                    expected_output: row.get(3)?,
                    work_dir: row.get(4)?,
                    thread_id: row.get(5)?,
                    runner: row.get(6)?,
                    crew_id: row.get(7)?,
                    model: row.get(8)?,
                    schedule_expr: row.get(9)?,
                    schedule_enabled: schedule_enabled != 0,
                    status: row.get(11)?,
                    output: row.get(12)?,
                    error: row.get(13)?,
                    last_run_at: row.get(14)?,
                    created_at: row.get(15)?,
                    updated_at: row.get(16)?,
                })
            },
        )
        .optional()
    }

    pub fn list_work_tasks(&self) -> SqlResult<Vec<WorkTaskRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, title, prompt, expected_output, work_dir, thread_id, runner, crew_id, model,
                    schedule_expr, schedule_enabled, status, output, error, last_run_at, created_at, updated_at
             FROM work_tasks
             ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let schedule_enabled: i32 = row.get(10)?;
            Ok(WorkTaskRow {
                id: row.get(0)?,
                title: row.get(1)?,
                prompt: row.get(2)?,
                expected_output: row.get(3)?,
                work_dir: row.get(4)?,
                thread_id: row.get(5)?,
                runner: row.get(6)?,
                crew_id: row.get(7)?,
                model: row.get(8)?,
                schedule_expr: row.get(9)?,
                schedule_enabled: schedule_enabled != 0,
                status: row.get(11)?,
                output: row.get(12)?,
                error: row.get(13)?,
                last_run_at: row.get(14)?,
                created_at: row.get(15)?,
                updated_at: row.get(16)?,
            })
        })?;
        rows.collect()
    }

    pub fn update_work_task_status(
        &self,
        id: &str,
        status: &str,
        output: Option<&str>,
        error: Option<&str>,
        last_run_at: Option<&str>,
        updated_at: &str,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE work_tasks
             SET status = ?2, output = ?3, error = ?4, last_run_at = ?5, updated_at = ?6
             WHERE id = ?1",
            params![id, status, output, error, last_run_at, updated_at],
        )?;
        Ok(())
    }

    pub fn delete_work_task(&self, id: &str) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute("DELETE FROM scheduled_tasks WHERE id = ?1", params![id])?;
        conn.execute("DELETE FROM work_tasks WHERE id = ?1", params![id])?;
        Ok(())
    }

    // -- Audit --

    #[allow(dead_code)]
    pub fn insert_audit_event(
        &self,
        id: &str,
        ts: &str,
        event_type: &str,
        resource_type: Option<&str>,
        resource_id: Option<&str>,
        details_json: Option<&str>,
    ) -> SqlResult<()> {
        let details_json = redact_and_bound_optional_json(details_json, MAX_LOG_JSON_BYTES);
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO audit_events (id, ts, event_type, resource_type, resource_id, details_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, ts, event_type, resource_type, resource_id, details_json],
        )?;
        conn.execute(
            "DELETE FROM audit_events
             WHERE id NOT IN (
               SELECT id FROM audit_events ORDER BY ts DESC, rowid DESC LIMIT ?1
             )",
            params![MAX_AUDIT_EVENTS],
        )?;
        Ok(())
    }

    // -- File Safety --

    pub fn add_allowed_folder(&self, path: &str) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT OR REPLACE INTO allowed_folders (path, created_at) VALUES (?1, datetime('now'))",
            params![path],
        )?;
        Ok(())
    }

    pub fn remove_allowed_folder(&self, path: &str) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute("DELETE FROM allowed_folders WHERE path = ?1", params![path])?;
        Ok(())
    }

    pub fn list_allowed_folders(&self) -> SqlResult<Vec<String>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare("SELECT path FROM allowed_folders ORDER BY created_at DESC")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect()
    }

    // -- Runtime Policy --

    pub fn set_policy_flag(&self, key: &str, value: bool) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO policy_flags (key, value, updated_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET
                value = excluded.value,
                updated_at = excluded.updated_at",
            params![key, value as i32],
        )?;
        Ok(())
    }

    pub fn list_policy_flags(&self) -> SqlResult<Vec<(String, bool)>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare("SELECT key, value FROM policy_flags")?;
        let rows = stmt.query_map([], |row| {
            let value: i32 = row.get(1)?;
            Ok((row.get(0)?, value != 0))
        })?;
        rows.collect()
    }

    pub fn replace_policy_deny_rules(&self, rules: &[String]) -> SqlResult<()> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM policy_deny_rules", [])?;

        let mut seen = HashSet::new();
        for raw_rule in rules {
            let rule = raw_rule.trim();
            if rule.is_empty() {
                continue;
            }
            if !seen.insert(rule.to_string()) {
                continue;
            }
            tx.execute(
                "INSERT INTO policy_deny_rules (rule, created_at) VALUES (?1, datetime('now'))",
                params![rule],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    pub fn list_policy_deny_rules(&self) -> SqlResult<Vec<String>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt =
            conn.prepare("SELECT rule FROM policy_deny_rules ORDER BY created_at DESC")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect()
    }

    pub fn replace_policy_tool_states(&self, states: &[(String, bool)]) -> SqlResult<()> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM policy_tool_states", [])?;

        for (tool_id, enabled) in states {
            let normalized_tool_id = tool_id.trim();
            if normalized_tool_id.is_empty() {
                continue;
            }

            tx.execute(
                "INSERT INTO policy_tool_states (tool_id, enabled, updated_at)
                 VALUES (?1, ?2, datetime('now'))",
                params![normalized_tool_id, *enabled as i32],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    pub fn list_policy_tool_states(&self) -> SqlResult<Vec<(String, bool)>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt =
            conn.prepare("SELECT tool_id, enabled FROM policy_tool_states ORDER BY tool_id ASC")?;
        let rows = stmt.query_map([], |row| {
            let enabled: i32 = row.get(1)?;
            Ok((row.get(0)?, enabled != 0))
        })?;
        rows.collect()
    }

    pub fn set_policy_setting(&self, key: &str, value: &str) -> SqlResult<()> {
        let normalized_key = key.trim();
        if normalized_key.is_empty() {
            return Err(rusqlite::Error::InvalidQuery);
        }

        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO policy_settings (key, value, updated_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET
                value = excluded.value,
                updated_at = excluded.updated_at",
            params![normalized_key, value.trim()],
        )?;
        Ok(())
    }

    pub fn get_policy_setting(&self, key: &str) -> SqlResult<Option<String>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare("SELECT value FROM policy_settings WHERE key = ?1 LIMIT 1")?;
        let mut rows = stmt.query(params![key.trim()])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    // -- Artifact Versions --

    pub fn insert_artifact_version(
        &self,
        id: &str,
        run_id: Option<&str>,
        label: Option<&str>,
        source_path: &str,
        format: &str,
        size_bytes: i64,
        summary: &str,
        preview: &str,
        metadata_json: &str,
        created_at: &str,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO artifact_versions (
                id, run_id, label, source_path, format, size_bytes, summary, preview, metadata_json, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id,
                run_id,
                label,
                source_path,
                format,
                size_bytes,
                summary,
                preview,
                metadata_json,
                created_at
            ],
        )?;
        Ok(())
    }

    pub fn list_artifact_versions(
        &self,
        limit: i64,
    ) -> SqlResult<
        Vec<(
            String,
            Option<String>,
            Option<String>,
            String,
            String,
            i64,
            String,
            String,
            String,
            String,
        )>,
    > {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT
                id,
                run_id,
                label,
                source_path,
                format,
                size_bytes,
                summary,
                preview,
                metadata_json,
                created_at
             FROM artifact_versions
             ORDER BY created_at DESC
             LIMIT ?1",
        )?;

        let rows = stmt.query_map(params![limit], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
            ))
        })?;

        rows.collect()
    }

    pub fn get_artifact_version_by_id(
        &self,
        artifact_version_id: &str,
    ) -> SqlResult<
        Option<(
            String,
            Option<String>,
            Option<String>,
            String,
            String,
            i64,
            String,
            String,
            String,
            String,
        )>,
    > {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT
                id,
                run_id,
                label,
                source_path,
                format,
                size_bytes,
                summary,
                preview,
                metadata_json,
                created_at
             FROM artifact_versions
             WHERE id = ?1
             LIMIT 1",
        )?;

        let mut rows = stmt.query(params![artifact_version_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
            )))
        } else {
            Ok(None)
        }
    }

    pub fn insert_artifact_export(
        &self,
        id: &str,
        artifact_version_id: &str,
        export_format: &str,
        target_path: &str,
        size_bytes: i64,
        created_at: &str,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO artifact_exports (
                id,
                artifact_version_id,
                export_format,
                target_path,
                size_bytes,
                created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                artifact_version_id,
                export_format,
                target_path,
                size_bytes,
                created_at
            ],
        )?;
        Ok(())
    }

    pub fn list_artifact_exports(
        &self,
        limit: i64,
    ) -> SqlResult<
        Vec<(
            String,
            String,
            String,
            String,
            i64,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
        )>,
    > {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT
                e.id,
                e.artifact_version_id,
                e.export_format,
                e.target_path,
                e.size_bytes,
                e.created_at,
                v.source_path,
                v.run_id,
                v.label,
                v.format
             FROM artifact_exports e
             JOIN artifact_versions v ON v.id = e.artifact_version_id
             ORDER BY e.created_at DESC
             LIMIT ?1",
        )?;

        let rows = stmt.query_map(params![limit], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
            ))
        })?;

        rows.collect()
    }

    // -- Scheduler --

    pub fn upsert_scheduled_task(
        &self,
        id: &str,
        name: &str,
        prompt: &str,
        schedule_expr: &str,
        task_kind: &str,
        crew_id: Option<&str>,
        crew_snapshot_json: Option<&str>,
        model_config_json: Option<&str>,
        priority: i64,
        depends_on_task_ids_json: &str,
        active: bool,
        last_run_at: Option<&str>,
        next_run_at: Option<&str>,
        now: &str,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO scheduled_tasks (
                     id, name, prompt, schedule_expr, task_kind, crew_id, crew_snapshot_json, model_config_json, priority, depends_on_task_ids_json, active, last_run_at, next_run_at, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                prompt = excluded.prompt,
                schedule_expr = excluded.schedule_expr,
                task_kind = excluded.task_kind,
                crew_id = excluded.crew_id,
                crew_snapshot_json = excluded.crew_snapshot_json,
                     model_config_json = excluded.model_config_json,
                priority = excluded.priority,
                depends_on_task_ids_json = excluded.depends_on_task_ids_json,
                active = excluded.active,
                last_run_at = excluded.last_run_at,
                next_run_at = excluded.next_run_at,
                updated_at = excluded.updated_at",
            params![
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
                active as i32,
                last_run_at,
                next_run_at,
                now
            ],
        )?;
        Ok(())
    }

    pub fn list_scheduled_tasks(
        &self,
    ) -> SqlResult<
        Vec<(
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
        )>,
    > {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, name, prompt, schedule_expr, task_kind, crew_id, crew_snapshot_json, model_config_json, priority, depends_on_task_ids_json, active, last_run_at, next_run_at, created_at, updated_at
             FROM scheduled_tasks
             ORDER BY priority DESC, created_at DESC"
        )?;

        let rows = stmt.query_map([], |row| {
            let active_value: i32 = row.get(10)?;
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
                active_value != 0,
                row.get(11)?,
                row.get(12)?,
                row.get(13)?,
                row.get(14)?,
            ))
        })?;

        rows.collect()
    }

    pub fn delete_scheduled_task(&self, id: &str) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute("DELETE FROM scheduled_tasks WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn set_scheduled_task_active(
        &self,
        id: &str,
        active: bool,
        next_run_at: Option<&str>,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE scheduled_tasks
             SET active = ?2, next_run_at = ?3, updated_at = datetime('now')
             WHERE id = ?1",
            params![id, active as i32, next_run_at],
        )?;
        Ok(())
    }

    pub fn list_due_scheduled_tasks(
        &self,
        now: &str,
    ) -> SqlResult<
        Vec<(
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
        )>,
    > {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, name, prompt, schedule_expr, next_run_at, task_kind, crew_id, crew_snapshot_json, model_config_json, priority, depends_on_task_ids_json, last_run_at
             FROM scheduled_tasks
             WHERE active = 1 AND next_run_at IS NOT NULL AND next_run_at <= ?1
             ORDER BY priority DESC, next_run_at ASC"
        )?;

        let rows = stmt.query_map(params![now], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
                row.get(10)?,
                row.get(11)?,
            ))
        })?;

        rows.collect()
    }

    pub fn latest_scheduled_run_status(
        &self,
        task_id: &str,
    ) -> SqlResult<Option<(String, Option<String>)>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.query_row(
            "SELECT status, finished_at
             FROM scheduled_runs
             WHERE task_id = ?1
             ORDER BY started_at DESC
             LIMIT 1",
            params![task_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
    }

    pub fn update_scheduled_task_runtime(
        &self,
        id: &str,
        last_run_at: Option<&str>,
        next_run_at: Option<&str>,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE scheduled_tasks
             SET last_run_at = ?2, next_run_at = ?3, updated_at = datetime('now')
             WHERE id = ?1",
            params![id, last_run_at, next_run_at],
        )?;
        Ok(())
    }

    pub fn begin_scheduled_run(
        &self,
        id: &str,
        task_id: &str,
        started_at: &str,
        next_run_at: Option<&str>,
    ) -> SqlResult<bool> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let already_running = transaction.query_row(
            "SELECT EXISTS(
               SELECT 1 FROM scheduled_runs WHERE task_id = ?1 AND status = 'running'
             )",
            params![task_id],
            |row| row.get::<_, bool>(0),
        )?;
        if already_running {
            transaction.rollback()?;
            return Ok(false);
        }

        transaction.execute(
            "INSERT INTO scheduled_runs (id, task_id, status, started_at)
             VALUES (?1, ?2, 'running', ?3)",
            params![id, task_id, started_at],
        )?;
        transaction.execute(
            "UPDATE scheduled_tasks
             SET last_run_at = ?2, next_run_at = ?3, updated_at = datetime('now')
             WHERE id = ?1",
            params![task_id, started_at, next_run_at],
        )?;
        transaction.commit()?;
        Ok(true)
    }

    pub fn insert_scheduled_run(
        &self,
        id: &str,
        task_id: &str,
        status: &str,
        started_at: &str,
        finished_at: Option<&str>,
        result: Option<&str>,
        error: Option<&str>,
    ) -> SqlResult<()> {
        let result = redact_and_bound_optional_text(result, MAX_LOG_TEXT_BYTES);
        let error = redact_and_bound_optional_text(error, MAX_LOG_TEXT_BYTES);
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO scheduled_runs (id, task_id, status, started_at, finished_at, result, error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
               status = excluded.status,
               started_at = excluded.started_at,
               finished_at = excluded.finished_at,
               result = excluded.result,
               error = excluded.error",
            params![id, task_id, status, started_at, finished_at, result, error],
        )?;
        conn.execute(
            "DELETE FROM scheduled_runs
             WHERE task_id = ?1
               AND id NOT IN (
                 SELECT id FROM scheduled_runs
                 WHERE task_id = ?1
                 ORDER BY started_at DESC, rowid DESC
                 LIMIT ?2
               )",
            params![task_id, MAX_SCHEDULED_RUNS_PER_TASK],
        )?;
        Ok(())
    }

    pub fn list_scheduled_runs(
        &self,
        limit: i64,
    ) -> SqlResult<
        Vec<(
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
        )>,
    > {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, task_id, status, started_at, finished_at, result, error
             FROM scheduled_runs
             ORDER BY started_at DESC
             LIMIT ?1",
        )?;

        let rows = stmt.query_map(params![limit], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ))
        })?;

        rows.collect()
    }

    pub fn recover_after_unclean_shutdown(
        &self,
        recovered_at: &str,
    ) -> SqlResult<StartupRecoveryReport> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let engine_run_ids = {
            let mut stmt = transaction.prepare(
                "SELECT id FROM engine_runs WHERE status = 'running' ORDER BY created_at",
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<SqlResult<Vec<_>>>()?
        };
        let crew_runs = {
            let mut stmt = transaction.prepare(
                "SELECT id, crew_id FROM crew_runs WHERE status = 'running' ORDER BY started_at",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<SqlResult<Vec<_>>>()?
        };

        let engine_runs = transaction.execute(
            "UPDATE engine_runs
             SET status = 'interrupted', phase = 'interrupted',
                 error = COALESCE(error, 'Application exited before the run completed.'),
                 ended_at = ?1, updated_at = ?1
             WHERE status = 'running'",
            params![recovered_at],
        )?;
        let legacy_tasks = transaction.execute(
            "UPDATE tasks
             SET status = 'failed',
                 error = COALESCE(error, 'Application exited before the task completed.'),
                 updated_at = ?1
             WHERE status = 'running'",
            params![recovered_at],
        )?;
        let task_steps = transaction.execute(
            "UPDATE task_steps
             SET state = 'failed',
                 output = COALESCE(output, 'Application exited before the step completed.')
             WHERE state = 'running'",
            [],
        )?;
        let work_tasks = transaction.execute(
            "UPDATE work_tasks
             SET status = 'failed',
                 error = COALESCE(error, 'Application exited before the task completed.'),
                 updated_at = ?1
             WHERE status = 'running'",
            params![recovered_at],
        )?;
        let scheduled_runs = transaction.execute(
            "UPDATE scheduled_runs
             SET status = 'interrupted', finished_at = ?1,
                 error = COALESCE(error, 'Application exited before the scheduled run completed.')
             WHERE status = 'running'",
            params![recovered_at],
        )?;
        let recovered_crew_runs = transaction.execute(
            "UPDATE crew_runs
             SET status = 'interrupted', finished_at = ?1,
                 error = COALESCE(error, 'Application exited before the crew run completed.')
             WHERE status = 'running'",
            params![recovered_at],
        )?;
        let worker_sandboxes = transaction.execute(
            "UPDATE worker_sandboxes
             SET status = 'interrupted', ended_at = ?1, updated_at = ?1
             WHERE status = 'active'",
            params![recovered_at],
        )?;
        let managed_processes = transaction.execute(
            "UPDATE managed_processes
             SET status = 'interrupted', pid = NULL, stopped_at = ?1
             WHERE status IN ('starting', 'running')",
            params![recovered_at],
        )?;
        let terminal_backends = transaction.execute(
            "UPDATE terminal_backends
             SET status = 'disconnected', updated_at = ?1
             WHERE status IN ('connecting', 'connected')",
            params![recovered_at],
        )?;

        let engine_payload = serde_json::json!({
            "reason": "unclean_shutdown",
            "recoveredAt": recovered_at,
        })
        .to_string();
        for run_id in &engine_run_ids {
            transaction.execute(
                "INSERT INTO engine_run_events (
                    id, run_id, sequence, event_type, summary, payload_json, redaction_level, created_at
                 ) VALUES (
                    ?1, ?2,
                    (SELECT COALESCE(MAX(sequence), 0) + 1 FROM engine_run_events WHERE run_id = ?2),
                    'run_interrupted', 'Run interrupted during startup recovery', ?3, 'metadata', ?4
                 )",
                params![
                    uuid::Uuid::new_v4().to_string(),
                    run_id,
                    engine_payload,
                    recovered_at
                ],
            )?;
            transaction.execute(
                "DELETE FROM engine_run_events
                 WHERE run_id = ?1
                   AND id NOT IN (
                     SELECT id FROM engine_run_events
                     WHERE run_id = ?1 ORDER BY sequence DESC LIMIT ?2
                   )",
                params![run_id, MAX_ENGINE_EVENTS_PER_RUN],
            )?;
        }

        let crew_payload = serde_json::json!({
            "reason": "unclean_shutdown",
            "recoveredAt": recovered_at,
        })
        .to_string();
        for (run_id, crew_id) in &crew_runs {
            transaction.execute(
                "INSERT INTO crew_run_events (
                    id, run_id, crew_id, event_type, payload_json, created_at
                 ) VALUES (?1, ?2, ?3, 'run_interrupted', ?4, ?5)",
                params![
                    uuid::Uuid::new_v4().to_string(),
                    run_id,
                    crew_id,
                    crew_payload,
                    recovered_at
                ],
            )?;
            transaction.execute(
                "DELETE FROM crew_run_events
                 WHERE run_id = ?1
                   AND id NOT IN (
                     SELECT id FROM crew_run_events
                     WHERE run_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT ?2
                   )",
                params![run_id, MAX_CREW_EVENTS_PER_RUN],
            )?;
        }

        let report = StartupRecoveryReport {
            recovered_at: recovered_at.to_string(),
            engine_runs,
            legacy_tasks,
            task_steps,
            work_tasks,
            scheduled_runs,
            crew_runs: recovered_crew_runs,
            worker_sandboxes,
            managed_processes,
            terminal_backends,
        };
        transaction.commit()?;
        Ok(report)
    }

    pub fn support_diagnostics_snapshot(&self) -> SqlResult<serde_json::Value> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let schema_version = current_schema_version(&conn)?.unwrap_or(0);
        let counts = conn.query_row(
            "SELECT
               (SELECT COUNT(*) FROM chat_threads),
               (SELECT COUNT(*) FROM engine_runs),
               (SELECT COUNT(*) FROM engine_run_events),
               (SELECT COUNT(*) FROM scheduled_tasks),
               (SELECT COUNT(*) FROM scheduled_runs),
               (SELECT COUNT(*) FROM crew_runs),
               (SELECT COUNT(*) FROM crew_run_logs),
               (SELECT COUNT(*) FROM crew_run_events),
               (SELECT COUNT(*) FROM audit_events)",
            [],
            |row| {
                Ok(serde_json::json!({
                    "chatThreads": row.get::<_, i64>(0)?,
                    "engineRuns": row.get::<_, i64>(1)?,
                    "engineRunEvents": row.get::<_, i64>(2)?,
                    "scheduledTasks": row.get::<_, i64>(3)?,
                    "scheduledRuns": row.get::<_, i64>(4)?,
                    "crewRuns": row.get::<_, i64>(5)?,
                    "crewRunLogs": row.get::<_, i64>(6)?,
                    "crewRunEvents": row.get::<_, i64>(7)?,
                    "auditEvents": row.get::<_, i64>(8)?,
                }))
            },
        )?;

        let recent_engine_runs = {
            let mut stmt = conn.prepare(
                "SELECT status, phase, created_at, updated_at, ended_at, error IS NOT NULL
                 FROM engine_runs ORDER BY updated_at DESC LIMIT 50",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(serde_json::json!({
                    "status": diagnostic_label(&row.get::<_, String>(0)?),
                    "phase": diagnostic_label(&row.get::<_, String>(1)?),
                    "createdAt": row.get::<_, String>(2)?,
                    "updatedAt": row.get::<_, String>(3)?,
                    "endedAt": row.get::<_, Option<String>>(4)?,
                    "hasError": row.get::<_, i64>(5)? != 0,
                }))
            })?;
            rows.collect::<SqlResult<Vec<_>>>()?
        };

        let recent_engine_events = {
            let mut stmt = conn.prepare(
                "SELECT event_type, redaction_level, created_at
                 FROM engine_run_events ORDER BY created_at DESC, rowid DESC LIMIT 100",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(serde_json::json!({
                    "eventType": diagnostic_label(&row.get::<_, String>(0)?),
                    "redactionLevel": diagnostic_label(&row.get::<_, String>(1)?),
                    "createdAt": row.get::<_, String>(2)?,
                }))
            })?;
            rows.collect::<SqlResult<Vec<_>>>()?
        };

        let recent_scheduled_runs = {
            let mut stmt = conn.prepare(
                "SELECT status, started_at, finished_at, error IS NOT NULL
                 FROM scheduled_runs ORDER BY started_at DESC, rowid DESC LIMIT 50",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(serde_json::json!({
                    "status": diagnostic_label(&row.get::<_, String>(0)?),
                    "startedAt": row.get::<_, String>(1)?,
                    "finishedAt": row.get::<_, Option<String>>(2)?,
                    "hasError": row.get::<_, i64>(3)? != 0,
                }))
            })?;
            rows.collect::<SqlResult<Vec<_>>>()?
        };

        let recent_crew_runs = {
            let mut stmt = conn.prepare(
                "SELECT process, status, started_at, finished_at, error IS NOT NULL
                 FROM crew_runs ORDER BY started_at DESC, rowid DESC LIMIT 50",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(serde_json::json!({
                    "process": diagnostic_label(&row.get::<_, String>(0)?),
                    "status": diagnostic_label(&row.get::<_, String>(1)?),
                    "startedAt": row.get::<_, String>(2)?,
                    "finishedAt": row.get::<_, Option<String>>(3)?,
                    "hasError": row.get::<_, i64>(4)? != 0,
                }))
            })?;
            rows.collect::<SqlResult<Vec<_>>>()?
        };

        Ok(serde_json::json!({
            "schemaVersion": schema_version,
            "counts": counts,
            "recentEngineRuns": recent_engine_runs,
            "recentEngineEvents": recent_engine_events,
            "recentScheduledRuns": recent_scheduled_runs,
            "recentCrewRuns": recent_crew_runs,
        }))
    }

    // -- Memory Entries --

    pub fn upsert_memory_entry(
        &self,
        id: &str,
        scope: &str,
        category: &str,
        key: &str,
        content: &str,
        source_session_id: Option<&str>,
        confidence: f64,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO memory_entries (id, scope, category, key, content, source_session_id, confidence, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'), datetime('now'))
             ON CONFLICT(scope, category, key) DO UPDATE SET
                content = excluded.content,
                confidence = excluded.confidence,
                source_session_id = COALESCE(excluded.source_session_id, memory_entries.source_session_id),
                updated_at = datetime('now')",
            params![id, scope, category, key, content, source_session_id, confidence],
        )?;
        Ok(())
    }

    pub fn get_memory_entry(
        &self,
        scope: &str,
        category: &str,
        key: &str,
    ) -> SqlResult<Option<MemoryEntryRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, scope, category, key, content, source_session_id, confidence, access_count, last_accessed_at, created_at, updated_at
             FROM memory_entries WHERE scope = ?1 AND category = ?2 AND key = ?3 LIMIT 1"
        )?;
        let mut rows = stmt.query(params![scope, category, key])?;
        if let Some(row) = rows.next()? {
            Ok(Some(MemoryEntryRow {
                id: row.get(0)?,
                scope: row.get(1)?,
                category: row.get(2)?,
                key: row.get(3)?,
                content: row.get(4)?,
                source_session_id: row.get(5)?,
                confidence: row.get(6)?,
                access_count: row.get(7)?,
                last_accessed_at: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn list_memory_entries(
        &self,
        scope: &str,
        category: Option<&str>,
        limit: i64,
    ) -> SqlResult<Vec<MemoryEntryRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let (sql, params_vec): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(cat) =
            category
        {
            (
                "SELECT id, scope, category, key, content, source_session_id, confidence, access_count, last_accessed_at, created_at, updated_at
                 FROM memory_entries WHERE scope = ?1 AND category = ?2 ORDER BY updated_at DESC LIMIT ?3",
                vec![Box::new(scope.to_string()), Box::new(cat.to_string()), Box::new(limit)],
            )
        } else {
            (
                "SELECT id, scope, category, key, content, source_session_id, confidence, access_count, last_accessed_at, created_at, updated_at
                 FROM memory_entries WHERE scope = ?1 ORDER BY updated_at DESC LIMIT ?2",
                vec![Box::new(scope.to_string()), Box::new(limit)],
            )
        };
        let mut stmt = conn.prepare(sql)?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(params_refs.as_slice(), |row| {
            Ok(MemoryEntryRow {
                id: row.get(0)?,
                scope: row.get(1)?,
                category: row.get(2)?,
                key: row.get(3)?,
                content: row.get(4)?,
                source_session_id: row.get(5)?,
                confidence: row.get(6)?,
                access_count: row.get(7)?,
                last_accessed_at: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            })
        })?;
        rows.collect()
    }

    pub fn list_all_memory_entries(
        &self,
        category: Option<&str>,
        limit: i64,
    ) -> SqlResult<Vec<MemoryEntryRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let (sql, params_vec): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(
            category,
        ) = category
        {
            (
                    "SELECT id, scope, category, key, content, source_session_id, confidence, access_count, last_accessed_at, created_at, updated_at
                     FROM memory_entries WHERE category = ?1 ORDER BY updated_at DESC LIMIT ?2",
                    vec![Box::new(category.to_string()), Box::new(limit)],
                )
        } else {
            (
                    "SELECT id, scope, category, key, content, source_session_id, confidence, access_count, last_accessed_at, created_at, updated_at
                     FROM memory_entries ORDER BY updated_at DESC LIMIT ?1",
                    vec![Box::new(limit)],
                )
        };
        let mut stmt = conn.prepare(sql)?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|value| value.as_ref()).collect();
        let rows = stmt.query_map(params_refs.as_slice(), |row| {
            Ok(MemoryEntryRow {
                id: row.get(0)?,
                scope: row.get(1)?,
                category: row.get(2)?,
                key: row.get(3)?,
                content: row.get(4)?,
                source_session_id: row.get(5)?,
                confidence: row.get(6)?,
                access_count: row.get(7)?,
                last_accessed_at: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            })
        })?;
        rows.collect()
    }

    pub fn search_memory_entries(
        &self,
        query: &str,
        scope: Option<&str>,
        category: Option<&str>,
        limit: i64,
    ) -> SqlResult<Vec<MemoryEntryRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let normalized_query = query.trim().to_lowercase();
        if normalized_query.is_empty() {
            return Ok(Vec::new());
        }

        let mut terms = normalized_query
            .split(|character: char| {
                !character.is_alphanumeric() && character != '_' && character != '-'
            })
            .filter(|term| term.chars().count() >= 3)
            .filter(|term| {
                !matches!(
                    *term,
                    "and"
                        | "the"
                        | "for"
                        | "this"
                        | "that"
                        | "with"
                        | "from"
                        | "into"
                        | "und"
                        | "der"
                        | "die"
                        | "das"
                        | "den"
                        | "dem"
                        | "ein"
                        | "eine"
                        | "mit"
                        | "von"
                        | "crew"
                        | "task"
                )
            })
            .map(str::to_string)
            .collect::<Vec<_>>();
        terms.sort();
        terms.dedup();
        terms.truncate(12);
        if terms.is_empty() {
            terms.push(normalized_query.clone());
        }

        // Keep retrieval bounded, then rank in Rust. This supports natural-language
        // queries without requiring every query term to occur as one exact phrase.
        let mut stmt = conn.prepare(
            "SELECT id, scope, category, key, content, source_session_id, confidence, access_count, last_accessed_at, created_at, updated_at
             FROM memory_entries ORDER BY updated_at DESC LIMIT 5000"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(MemoryEntryRow {
                id: row.get(0)?,
                scope: row.get(1)?,
                category: row.get(2)?,
                key: row.get(3)?,
                content: row.get(4)?,
                source_session_id: row.get(5)?,
                confidence: row.get(6)?,
                access_count: row.get(7)?,
                last_accessed_at: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            })
        })?;
        let entries = rows.collect::<SqlResult<Vec<_>>>()?;
        let mut ranked = entries
            .into_iter()
            .filter(|entry| scope.is_none_or(|value| entry.scope == value))
            .filter(|entry| category.is_none_or(|value| entry.category == value))
            .filter_map(|entry| {
                let key = entry.key.to_lowercase();
                let category = entry.category.to_lowercase();
                let content = entry.content.to_lowercase();
                let mut score = 0_i64;

                if key == normalized_query {
                    score += 140;
                } else if key.contains(&normalized_query) {
                    score += 90;
                }
                if content.contains(&normalized_query) {
                    score += 60;
                }
                for term in &terms {
                    if key.contains(term) {
                        score += 24;
                    }
                    if category.contains(term) {
                        score += 12;
                    }
                    if content.contains(term) {
                        score += 6;
                    }
                }
                if score == 0 {
                    return None;
                }

                score += (entry.confidence.clamp(0.0, 1.0) * 8.0).round() as i64;
                score += i64::from(entry.access_count.clamp(0, 10));
                Some((score, entry))
            })
            .collect::<Vec<_>>();
        ranked.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .cmp(left_score)
                .then_with(|| right.updated_at.cmp(&left.updated_at))
        });
        ranked.truncate(limit.clamp(1, 200) as usize);
        Ok(ranked.into_iter().map(|(_, entry)| entry).collect())
    }

    pub fn delete_memory_entry(&self, id: &str) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute("DELETE FROM memory_entries WHERE id = ?1", params![id])?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn touch_memory_entry(&self, id: &str) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE memory_entries SET access_count = access_count + 1, last_accessed_at = datetime('now') WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn compact_memory(
        &self,
        scope: &str,
        min_confidence: f64,
        max_age_days: i64,
    ) -> SqlResult<usize> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let deleted = conn.execute(
            "DELETE FROM memory_entries WHERE scope = ?1 AND confidence < ?2 AND updated_at < datetime('now', ?3 || ' days')",
            params![scope, min_confidence, -max_age_days],
        )?;
        Ok(deleted)
    }

    // -- User Profile --

    pub fn upsert_user_profile(
        &self,
        id: &str,
        key: &str,
        value: &str,
        source: &str,
        confidence: f64,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO user_profile (id, key, value, source, confidence, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'), datetime('now'))
             ON CONFLICT(key) DO UPDATE SET
                value = excluded.value,
                source = excluded.source,
                confidence = excluded.confidence,
                updated_at = datetime('now')",
            params![id, key, value, source, confidence],
        )?;
        Ok(())
    }

    pub fn list_user_profile(&self) -> SqlResult<Vec<UserProfileRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, key, value, source, confidence, created_at, updated_at FROM user_profile ORDER BY key"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(UserProfileRow {
                id: row.get(0)?,
                key: row.get(1)?,
                value: row.get(2)?,
                source: row.get(3)?,
                confidence: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    pub fn delete_user_profile_entry(&self, key: &str) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute("DELETE FROM user_profile WHERE key = ?1", params![key])?;
        Ok(())
    }

    // -- Skills --

    pub fn upsert_skill(
        &self,
        id: &str,
        name: &str,
        description: &str,
        prompt_template: &str,
        trigger_pattern: Option<&str>,
        run_mode: &str,
        _auto_generated: bool,
        parent_skill_id: Option<&str>,
        source_task_ids: Option<&str>,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO skills (id, name, description, prompt_template, trigger_pattern, run_mode, auto_generated, parent_skill_id, source_task_ids, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?8, datetime('now'), datetime('now'))
             ON CONFLICT(name) DO UPDATE SET
                description = excluded.description,
                prompt_template = excluded.prompt_template,
                trigger_pattern = excluded.trigger_pattern,
                run_mode = excluded.run_mode,
                updated_at = datetime('now')",
            params![id, name, description, prompt_template, trigger_pattern, run_mode,
                    parent_skill_id, source_task_ids],
        )?;
        Ok(())
    }

    pub fn list_skills(&self, limit: i64) -> SqlResult<Vec<SkillRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, name, description, prompt_template, trigger_pattern, run_mode, version,
                    usage_count, success_count, fail_count, avg_quality, auto_generated,
                    parent_skill_id, source_task_ids, created_at, updated_at
             FROM skills ORDER BY usage_count DESC, updated_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |row| {
            let auto_gen: i32 = row.get(11)?;
            Ok(SkillRow {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                prompt_template: row.get(3)?,
                trigger_pattern: row.get(4)?,
                run_mode: row.get(5)?,
                version: row.get(6)?,
                usage_count: row.get(7)?,
                success_count: row.get(8)?,
                fail_count: row.get(9)?,
                avg_quality: row.get(10)?,
                auto_generated: auto_gen != 0,
                parent_skill_id: row.get(12)?,
                source_task_ids: row.get(13)?,
                created_at: row.get(14)?,
                updated_at: row.get(15)?,
            })
        })?;
        rows.collect()
    }

    pub fn get_skill_by_name(&self, name: &str) -> SqlResult<Option<SkillRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, name, description, prompt_template, trigger_pattern, run_mode, version,
                    usage_count, success_count, fail_count, avg_quality, auto_generated,
                    parent_skill_id, source_task_ids, created_at, updated_at
             FROM skills WHERE name = ?1 LIMIT 1",
        )?;
        let mut rows = stmt.query(params![name])?;
        if let Some(row) = rows.next()? {
            let auto_gen: i32 = row.get(11)?;
            Ok(Some(SkillRow {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                prompt_template: row.get(3)?,
                trigger_pattern: row.get(4)?,
                run_mode: row.get(5)?,
                version: row.get(6)?,
                usage_count: row.get(7)?,
                success_count: row.get(8)?,
                fail_count: row.get(9)?,
                avg_quality: row.get(10)?,
                auto_generated: auto_gen != 0,
                parent_skill_id: row.get(12)?,
                source_task_ids: row.get(13)?,
                created_at: row.get(14)?,
                updated_at: row.get(15)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn record_skill_usage(&self, id: &str, success: bool, quality: f64) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        if success {
            conn.execute(
                "UPDATE skills SET usage_count = usage_count + 1, success_count = success_count + 1,
                        avg_quality = (avg_quality * usage_count + ?2) / (usage_count + 1),
                        updated_at = datetime('now') WHERE id = ?1",
                params![id, quality],
            )?;
        } else {
            conn.execute(
                "UPDATE skills SET usage_count = usage_count + 1, fail_count = fail_count + 1,
                        updated_at = datetime('now') WHERE id = ?1",
                params![id],
            )?;
        }
        Ok(())
    }

    pub fn improve_skill(&self, skill_id: &str, new_prompt: &str, reason: &str) -> SqlResult<i32> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let old_version: i32 = conn.query_row(
            "SELECT version FROM skills WHERE id = ?1",
            params![skill_id],
            |row| row.get(0),
        )?;
        let new_version = old_version + 1;
        let old_prompt: String = conn.query_row(
            "SELECT prompt_template FROM skills WHERE id = ?1",
            params![skill_id],
            |row| row.get(0),
        )?;
        conn.execute(
            "UPDATE skills SET prompt_template = ?2, version = ?3, updated_at = datetime('now') WHERE id = ?1",
            params![skill_id, new_prompt, new_version],
        )?;
        let imp_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO skill_improvements (id, skill_id, version_before, version_after, reason, prompt_before, prompt_after, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))",
            params![imp_id, skill_id, old_version, new_version, reason, old_prompt, new_prompt],
        )?;
        Ok(new_version)
    }

    pub fn delete_skill(&self, id: &str) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute("DELETE FROM skills WHERE id = ?1", params![id])?;
        Ok(())
    }

    // -- Sessions --

    pub fn insert_session(
        &self,
        id: &str,
        thread_id: Option<&str>,
        title: &str,
        memory_snapshot_json: Option<&str>,
        model_used: Option<&str>,
        provider: Option<&str>,
        personality: Option<&str>,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO sessions (id, thread_id, title, memory_snapshot_json, model_used, provider, personality, started_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))
             ON CONFLICT(id) DO NOTHING",
            params![id, thread_id, title, memory_snapshot_json, model_used, provider, personality],
        )?;
        Ok(())
    }

    pub fn end_session(
        &self,
        id: &str,
        summary: Option<&str>,
        total_messages: i32,
        total_tokens_est: i64,
        outcome: Option<&str>,
        task_ids: Option<&str>,
        skill_ids_used: Option<&str>,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE sessions SET summary = ?2, total_messages = ?3, total_tokens_est = ?4,
                    outcome = ?5, task_ids = ?6, skill_ids_used = ?7, ended_at = datetime('now')
             WHERE id = ?1",
            params![
                id,
                summary,
                total_messages,
                total_tokens_est,
                outcome,
                task_ids,
                skill_ids_used
            ],
        )?;
        Ok(())
    }

    pub fn list_sessions(&self, limit: i64) -> SqlResult<Vec<SessionRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, thread_id, title, summary, model_used, provider, personality,
                    total_messages, total_tokens_est, outcome, started_at, ended_at
             FROM sessions ORDER BY started_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |row| {
            Ok(SessionRow {
                id: row.get(0)?,
                thread_id: row.get(1)?,
                title: row.get(2)?,
                summary: row.get(3)?,
                model_used: row.get(4)?,
                provider: row.get(5)?,
                personality: row.get(6)?,
                total_messages: row.get(7)?,
                total_tokens_est: row.get(8)?,
                outcome: row.get(9)?,
                started_at: row.get(10)?,
                ended_at: row.get(11)?,
            })
        })?;
        rows.collect()
    }

    pub fn delete_session(&self, id: &str) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute("DELETE FROM sessions WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn get_session(&self, id: &str) -> SqlResult<Option<SessionRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, thread_id, title, summary, model_used, provider, personality,
                    total_messages, total_tokens_est, outcome, started_at, ended_at
             FROM sessions WHERE id = ?1 LIMIT 1",
        )?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(SessionRow {
                id: row.get(0)?,
                thread_id: row.get(1)?,
                title: row.get(2)?,
                summary: row.get(3)?,
                model_used: row.get(4)?,
                provider: row.get(5)?,
                personality: row.get(6)?,
                total_messages: row.get(7)?,
                total_tokens_est: row.get(8)?,
                outcome: row.get(9)?,
                started_at: row.get(10)?,
                ended_at: row.get(11)?,
            }))
        } else {
            Ok(None)
        }
    }

    #[allow(dead_code)]
    pub fn search_sessions(&self, query: &str, limit: i64) -> SqlResult<Vec<SessionRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let pattern = format!("%{}%", query);
        let mut stmt = conn.prepare(
            "SELECT id, thread_id, title, summary, model_used, provider, personality,
                    total_messages, total_tokens_est, outcome, started_at, ended_at
             FROM sessions WHERE title LIKE ?1 OR summary LIKE ?1 ORDER BY started_at DESC LIMIT ?2"
        )?;
        let rows = stmt.query_map(params![pattern, limit], |row| {
            Ok(SessionRow {
                id: row.get(0)?,
                thread_id: row.get(1)?,
                title: row.get(2)?,
                summary: row.get(3)?,
                model_used: row.get(4)?,
                provider: row.get(5)?,
                personality: row.get(6)?,
                total_messages: row.get(7)?,
                total_tokens_est: row.get(8)?,
                outcome: row.get(9)?,
                started_at: row.get(10)?,
                ended_at: row.get(11)?,
            })
        })?;
        rows.collect()
    }

    #[allow(dead_code)]
    pub fn get_session_memory_snapshot(&self, session_id: &str) -> SqlResult<Option<String>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt =
            conn.prepare("SELECT memory_snapshot_json FROM sessions WHERE id = ?1 LIMIT 1")?;
        let mut rows = stmt.query(params![session_id])?;
        if let Some(row) = rows.next()? {
            Ok(row.get(0)?)
        } else {
            Ok(None)
        }
    }

    pub fn save_session_snapshot(&self, session_id: &str, snapshot_json: &str) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE sessions SET memory_snapshot_json = ?2 WHERE id = ?1",
            params![session_id, snapshot_json],
        )?;
        Ok(())
    }

    // -- Engine Runs --

    #[allow(dead_code)]
    pub fn insert_engine_run(
        &self,
        id: &str,
        parent_run_id: Option<&str>,
        thread_id: Option<&str>,
        session_id: Option<&str>,
        title: &str,
        input_summary: Option<&str>,
        status: &str,
        phase: &str,
        cwd: Option<&str>,
        model: Option<&str>,
        provider: Option<&str>,
        retry_count: i32,
        resumed_from_run_id: Option<&str>,
        checkpoint_json: Option<&str>,
        metadata_json: Option<&str>,
    ) -> SqlResult<()> {
        self.insert_engine_run_with_gateway_metadata(
            id,
            parent_run_id,
            thread_id,
            session_id,
            title,
            input_summary,
            status,
            phase,
            cwd,
            model,
            provider,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            retry_count,
            resumed_from_run_id,
            checkpoint_json,
            metadata_json,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_engine_run_with_gateway_metadata(
        &self,
        id: &str,
        parent_run_id: Option<&str>,
        thread_id: Option<&str>,
        session_id: Option<&str>,
        title: &str,
        input_summary: Option<&str>,
        status: &str,
        phase: &str,
        cwd: Option<&str>,
        model: Option<&str>,
        provider: Option<&str>,
        source: Option<&str>,
        workspace_path: Option<&str>,
        provider_profile_id: Option<&str>,
        runtime_mode: Option<&str>,
        toolset_policy_id: Option<&str>,
        channel_kind: Option<&str>,
        channel_ref: Option<&str>,
        retry_count: i32,
        resumed_from_run_id: Option<&str>,
        checkpoint_json: Option<&str>,
        metadata_json: Option<&str>,
    ) -> SqlResult<()> {
        let title = redact_and_bound_text(title, MAX_LOG_SUMMARY_BYTES);
        let input_summary = redact_and_bound_optional_text(input_summary, MAX_LOG_SUMMARY_BYTES);
        let checkpoint_json = redact_and_bound_optional_json(checkpoint_json, MAX_LOG_JSON_BYTES);
        let metadata_json = redact_and_bound_optional_json(metadata_json, MAX_LOG_JSON_BYTES);
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO engine_runs (
                id, parent_run_id, thread_id, session_id, title, input_summary, status, phase,
                cwd, model, provider, source, workspace_path, provider_profile_id, runtime_mode,
                toolset_policy_id, channel_kind, channel_ref, retry_count, resumed_from_run_id, checkpoint_json,
                metadata_json, created_at, updated_at, started_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                ?16, ?17, ?18, ?19, ?20, ?21,
                ?22, datetime('now'), datetime('now'),
                CASE WHEN ?7 = 'running' THEN datetime('now') ELSE NULL END
             )",
            params![
                id,
                parent_run_id,
                thread_id,
                session_id,
                title,
                input_summary,
                status,
                phase,
                cwd,
                model,
                provider,
                source.unwrap_or("desktop"),
                workspace_path,
                provider_profile_id,
                runtime_mode.unwrap_or("host"),
                toolset_policy_id,
                channel_kind,
                channel_ref,
                retry_count,
                resumed_from_run_id,
                checkpoint_json,
                metadata_json
            ],
        )?;
        Ok(())
    }

    pub fn get_engine_run(&self, id: &str) -> SqlResult<Option<EngineRunRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, parent_run_id, thread_id, session_id, title, input_summary, status, phase,
                    cwd, model, provider, retry_count, resumed_from_run_id, checkpoint_json,
                    result_summary, error, metadata_json, created_at, updated_at, started_at, ended_at, canceled_at,
                    source, workspace_path, provider_profile_id, runtime_mode, toolset_policy_id,
                    channel_kind, channel_ref
             FROM engine_runs WHERE id = ?1 LIMIT 1"
        )?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(map_engine_run_row(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn list_engine_runs(
        &self,
        limit: i64,
        status: Option<&str>,
    ) -> SqlResult<Vec<EngineRunRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let rows = if let Some(status_filter) = status {
            let mut stmt = conn.prepare(
                "SELECT id, parent_run_id, thread_id, session_id, title, input_summary, status, phase,
                        cwd, model, provider, retry_count, resumed_from_run_id, checkpoint_json,
                        result_summary, error, metadata_json, created_at, updated_at, started_at, ended_at, canceled_at,
                        source, workspace_path, provider_profile_id, runtime_mode, toolset_policy_id,
                        channel_kind, channel_ref
                 FROM engine_runs
                 WHERE status = ?1
                 ORDER BY updated_at DESC
                 LIMIT ?2"
            )?;
            let mapped = stmt.query_map(params![status_filter, limit], map_engine_run_row)?;
            mapped.collect::<SqlResult<Vec<_>>>()?
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, parent_run_id, thread_id, session_id, title, input_summary, status, phase,
                        cwd, model, provider, retry_count, resumed_from_run_id, checkpoint_json,
                        result_summary, error, metadata_json, created_at, updated_at, started_at, ended_at, canceled_at,
                        source, workspace_path, provider_profile_id, runtime_mode, toolset_policy_id,
                        channel_kind, channel_ref
                 FROM engine_runs
                 ORDER BY updated_at DESC
                 LIMIT ?1"
            )?;
            let mapped = stmt.query_map(params![limit], map_engine_run_row)?;
            mapped.collect::<SqlResult<Vec<_>>>()?
        };
        Ok(rows)
    }

    pub fn update_engine_run(
        &self,
        id: &str,
        status: Option<&str>,
        phase: Option<&str>,
        checkpoint_json: Option<&str>,
        result_summary: Option<&str>,
        error: Option<&str>,
        metadata_json: Option<&str>,
    ) -> SqlResult<()> {
        let current = self.get_engine_run(id)?;
        let existing = current.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;

        let next_status = status.unwrap_or(existing.status.as_str()).to_string();
        let next_phase = phase.unwrap_or(existing.phase.as_str()).to_string();
        let next_checkpoint = redact_and_bound_optional_json(
            checkpoint_json.or(existing.checkpoint_json.as_deref()),
            MAX_LOG_JSON_BYTES,
        );
        let next_result = redact_and_bound_optional_text(
            result_summary.or(existing.result_summary.as_deref()),
            MAX_LOG_TEXT_BYTES,
        );
        let next_error =
            redact_and_bound_optional_text(error.or(existing.error.as_deref()), MAX_LOG_TEXT_BYTES);
        let next_metadata = redact_and_bound_optional_json(
            metadata_json.or(existing.metadata_json.as_deref()),
            MAX_LOG_JSON_BYTES,
        );
        let started_at = if existing.started_at.is_none() && next_status == "running" {
            Some(chrono::Utc::now().to_rfc3339())
        } else {
            existing.started_at
        };
        let ended_at = if ["completed", "failed", "canceled"].contains(&next_status.as_str()) {
            Some(chrono::Utc::now().to_rfc3339())
        } else if ["running", "pending"].contains(&next_status.as_str()) {
            None
        } else {
            existing.ended_at
        };
        let canceled_at = if next_status == "canceled" {
            Some(chrono::Utc::now().to_rfc3339())
        } else if ["running", "pending", "completed", "failed"].contains(&next_status.as_str()) {
            None
        } else {
            existing.canceled_at
        };

        conn.execute(
            "UPDATE engine_runs
             SET status = ?2,
                 phase = ?3,
                 checkpoint_json = ?4,
                 result_summary = ?5,
                 error = ?6,
                 metadata_json = ?7,
                 started_at = ?8,
                 ended_at = ?9,
                 canceled_at = ?10,
                 updated_at = datetime('now')
             WHERE id = ?1",
            params![
                id,
                next_status,
                next_phase,
                next_checkpoint,
                next_result,
                next_error,
                next_metadata,
                started_at,
                ended_at,
                canceled_at
            ],
        )?;
        Ok(())
    }

    pub fn insert_engine_run_event(
        &self,
        id: &str,
        run_id: &str,
        event_type: &str,
        payload_json: Option<&str>,
    ) -> SqlResult<()> {
        self.insert_engine_run_event_with_details(id, run_id, event_type, None, payload_json, None)
    }

    pub fn insert_engine_run_event_with_details(
        &self,
        id: &str,
        run_id: &str,
        event_type: &str,
        summary: Option<&str>,
        payload_json: Option<&str>,
        redaction_level: Option<&str>,
    ) -> SqlResult<()> {
        let original_summary = summary.unwrap_or(event_type);
        let summary = redact_and_bound_text(original_summary, MAX_LOG_SUMMARY_BYTES);
        let payload = redact_and_bound_optional_json(payload_json, MAX_LOG_JSON_BYTES);
        let content_changed = summary != original_summary || payload.as_deref() != payload_json;
        let redaction_level = if content_changed {
            "automatic"
        } else {
            redaction_level.unwrap_or("none")
        };
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO engine_run_events (
                id, run_id, sequence, event_type, summary, payload_json, redaction_level, created_at
             ) VALUES (
                ?1,
                ?2,
                (SELECT COALESCE(MAX(sequence), 0) + 1 FROM engine_run_events WHERE run_id = ?2),
                ?3,
                ?4,
                ?5,
                ?6,
                datetime('now')
             )",
            params![id, run_id, event_type, summary, payload, redaction_level],
        )?;
        conn.execute(
            "DELETE FROM engine_run_events
             WHERE run_id = ?1
               AND id NOT IN (
                 SELECT id FROM engine_run_events
                 WHERE run_id = ?1
                 ORDER BY sequence DESC
                 LIMIT ?2
               )",
            params![run_id, MAX_ENGINE_EVENTS_PER_RUN],
        )?;
        conn.execute(
            "UPDATE engine_runs SET updated_at = datetime('now') WHERE id = ?1",
            params![run_id],
        )?;
        Ok(())
    }

    pub fn list_engine_run_events(
        &self,
        run_id: &str,
        limit: i64,
    ) -> SqlResult<Vec<EngineRunEventRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, run_id, sequence, event_type, summary, payload_json, redaction_level, created_at
             FROM engine_run_events
             WHERE run_id = ?1
             ORDER BY sequence ASC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![run_id, limit], map_engine_run_event_row)?;
        rows.collect()
    }

    pub fn insert_engine_run_artifact(
        &self,
        id: &str,
        run_id: &str,
        kind: &str,
        path: &str,
        title: Option<&str>,
        summary: Option<&str>,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO engine_run_artifacts (id, run_id, kind, path, title, summary, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))",
            params![id, run_id, kind, path, title, summary],
        )?;
        conn.execute(
            "UPDATE engine_runs SET updated_at = datetime('now') WHERE id = ?1",
            params![run_id],
        )?;
        Ok(())
    }

    pub fn list_engine_run_artifacts(
        &self,
        run_id: &str,
        limit: i64,
    ) -> SqlResult<Vec<EngineRunArtifactRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, run_id, kind, path, title, summary, created_at
             FROM engine_run_artifacts
             WHERE run_id = ?1
             ORDER BY created_at DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![run_id, limit], map_engine_run_artifact_row)?;
        rows.collect()
    }

    pub fn insert_engine_run_checkpoint(
        &self,
        id: &str,
        run_id: &str,
        label: &str,
        snapshot_json: &str,
    ) -> SqlResult<()> {
        let label = redact_and_bound_text(label, MAX_LOG_SUMMARY_BYTES);
        let snapshot_json = redact_and_bound_json_text(snapshot_json, MAX_LOG_JSON_BYTES);
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO engine_run_checkpoints (id, run_id, label, snapshot_json, created_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))",
            params![id, run_id, label, snapshot_json],
        )?;
        conn.execute(
            "UPDATE engine_runs
             SET checkpoint_json = ?2, updated_at = datetime('now')
             WHERE id = ?1",
            params![run_id, snapshot_json],
        )?;
        Ok(())
    }

    pub fn list_engine_run_checkpoints(
        &self,
        run_id: &str,
        limit: i64,
    ) -> SqlResult<Vec<EngineRunCheckpointRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, run_id, label, snapshot_json, created_at
             FROM engine_run_checkpoints
             WHERE run_id = ?1
             ORDER BY created_at DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![run_id, limit], |row| {
            Ok(EngineRunCheckpointRow {
                id: row.get(0)?,
                run_id: row.get(1)?,
                label: row.get(2)?,
                snapshot_json: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    // -- Runtime Instructions --

    pub fn upsert_runtime_instruction(
        &self,
        id: &str,
        scope_type: &str,
        scope_ref: Option<&str>,
        title: &str,
        content: &str,
        enabled: bool,
        priority: i32,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO runtime_instructions (
                id, scope_type, scope_ref, title, content, enabled, priority, created_at, updated_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'), datetime('now')
             )
             ON CONFLICT(id) DO UPDATE SET
                scope_type = excluded.scope_type,
                scope_ref = excluded.scope_ref,
                title = excluded.title,
                content = excluded.content,
                enabled = excluded.enabled,
                priority = excluded.priority,
                updated_at = datetime('now')",
            params![
                id,
                scope_type,
                scope_ref,
                title,
                content,
                enabled as i32,
                priority
            ],
        )?;
        Ok(())
    }

    pub fn delete_runtime_instruction(&self, id: &str) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "DELETE FROM runtime_instructions WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    pub fn list_runtime_instructions(
        &self,
        scope_type: Option<&str>,
        enabled_only: bool,
    ) -> SqlResult<Vec<RuntimeInstructionRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let rows = match scope_type {
            Some(scope) => {
                let mut stmt = conn.prepare(
                    "SELECT id, scope_type, scope_ref, title, content, enabled, priority, created_at, updated_at
                     FROM runtime_instructions
                     WHERE scope_type = ?1 AND (?2 = 0 OR enabled = 1)
                     ORDER BY priority DESC, updated_at DESC"
                )?;
                let mapped = stmt.query_map(params![scope, enabled_only as i32], |row| {
                    Ok(RuntimeInstructionRow {
                        id: row.get(0)?,
                        scope_type: row.get(1)?,
                        scope_ref: row.get(2)?,
                        title: row.get(3)?,
                        content: row.get(4)?,
                        enabled: row.get::<_, i32>(5)? != 0,
                        priority: row.get(6)?,
                        created_at: row.get(7)?,
                        updated_at: row.get(8)?,
                    })
                })?;
                mapped.collect::<SqlResult<Vec<_>>>()?
            }
            None => {
                let mut stmt = conn.prepare(
                    "SELECT id, scope_type, scope_ref, title, content, enabled, priority, created_at, updated_at
                     FROM runtime_instructions
                     WHERE (?1 = 0 OR enabled = 1)
                     ORDER BY priority DESC, updated_at DESC"
                )?;
                let mapped = stmt.query_map(params![enabled_only as i32], |row| {
                    Ok(RuntimeInstructionRow {
                        id: row.get(0)?,
                        scope_type: row.get(1)?,
                        scope_ref: row.get(2)?,
                        title: row.get(3)?,
                        content: row.get(4)?,
                        enabled: row.get::<_, i32>(5)? != 0,
                        priority: row.get(6)?,
                        created_at: row.get(7)?,
                        updated_at: row.get(8)?,
                    })
                })?;
                mapped.collect::<SqlResult<Vec<_>>>()?
            }
        };
        Ok(rows)
    }

    // -- Worker Sandboxes --

    pub fn insert_worker_sandbox(
        &self,
        id: &str,
        run_id: &str,
        parent_run_id: Option<&str>,
        backend_id: Option<&str>,
        status: &str,
        mode: &str,
        source_cwd: &str,
        workspace_root: &str,
        allowed_roots_json: &str,
        read_only_roots_json: Option<&str>,
        allow_file_read: bool,
        allow_file_write: bool,
        allow_shell_execution: bool,
        allow_web_fetch: bool,
        allow_web_search: bool,
        allow_mcp: bool,
        env_json: Option<&str>,
        metadata_json: Option<&str>,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO worker_sandboxes (
                id, run_id, parent_run_id, backend_id, status, mode, source_cwd, workspace_root,
                allowed_roots_json, read_only_roots_json, allow_file_read, allow_file_write,
                allow_shell_execution, allow_web_fetch, allow_web_search, allow_mcp, env_json,
                metadata_json, created_at, updated_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
                datetime('now'), datetime('now')
             )",
            params![
                id,
                run_id,
                parent_run_id,
                backend_id,
                status,
                mode,
                source_cwd,
                workspace_root,
                allowed_roots_json,
                read_only_roots_json,
                allow_file_read as i32,
                allow_file_write as i32,
                allow_shell_execution as i32,
                allow_web_fetch as i32,
                allow_web_search as i32,
                allow_mcp as i32,
                env_json,
                metadata_json,
            ],
        )?;
        Ok(())
    }

    pub fn get_worker_sandbox(&self, id: &str) -> SqlResult<Option<WorkerSandboxRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, run_id, parent_run_id, backend_id, status, mode, source_cwd, workspace_root,
                    allowed_roots_json, read_only_roots_json, allow_file_read, allow_file_write,
                    allow_shell_execution, allow_web_fetch, allow_web_search, allow_mcp, env_json,
                    metadata_json, created_at, updated_at, ended_at
             FROM worker_sandboxes
             WHERE id = ?1
             LIMIT 1"
        )?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            return Ok(Some(map_worker_sandbox_row(row)?));
        }
        Ok(None)
    }

    pub fn get_worker_sandbox_by_run(&self, run_id: &str) -> SqlResult<Option<WorkerSandboxRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, run_id, parent_run_id, backend_id, status, mode, source_cwd, workspace_root,
                    allowed_roots_json, read_only_roots_json, allow_file_read, allow_file_write,
                    allow_shell_execution, allow_web_fetch, allow_web_search, allow_mcp, env_json,
                    metadata_json, created_at, updated_at, ended_at
             FROM worker_sandboxes
             WHERE run_id = ?1
             LIMIT 1"
        )?;
        let mut rows = stmt.query(params![run_id])?;
        if let Some(row) = rows.next()? {
            return Ok(Some(map_worker_sandbox_row(row)?));
        }
        Ok(None)
    }

    pub fn list_worker_sandboxes(
        &self,
        limit: i64,
        status: Option<&str>,
    ) -> SqlResult<Vec<WorkerSandboxRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        match status {
            Some(filter_status) => {
                let mut stmt = conn.prepare(
                    "SELECT id, run_id, parent_run_id, backend_id, status, mode, source_cwd, workspace_root,
                            allowed_roots_json, read_only_roots_json, allow_file_read, allow_file_write,
                            allow_shell_execution, allow_web_fetch, allow_web_search, allow_mcp, env_json,
                            metadata_json, created_at, updated_at, ended_at
                     FROM worker_sandboxes
                     WHERE status = ?1
                     ORDER BY updated_at DESC
                     LIMIT ?2"
                )?;
                let mapped =
                    stmt.query_map(params![filter_status, limit], map_worker_sandbox_row)?;
                mapped.collect()
            }
            None => {
                let mut stmt = conn.prepare(
                    "SELECT id, run_id, parent_run_id, backend_id, status, mode, source_cwd, workspace_root,
                            allowed_roots_json, read_only_roots_json, allow_file_read, allow_file_write,
                            allow_shell_execution, allow_web_fetch, allow_web_search, allow_mcp, env_json,
                            metadata_json, created_at, updated_at, ended_at
                     FROM worker_sandboxes
                     ORDER BY updated_at DESC
                     LIMIT ?1"
                )?;
                let mapped = stmt.query_map(params![limit], map_worker_sandbox_row)?;
                mapped.collect()
            }
        }
    }

    pub fn update_worker_sandbox(
        &self,
        id: &str,
        status: Option<&str>,
        metadata_json: Option<&str>,
    ) -> SqlResult<()> {
        let Some(existing) = self.get_worker_sandbox(id)? else {
            return Ok(());
        };
        let next_status = status.unwrap_or(existing.status.as_str());
        let next_metadata = metadata_json.or(existing.metadata_json.as_deref());
        let ended_at = if [
            "completed",
            "failed",
            "canceled",
            "destroyed",
            "interrupted",
        ]
        .contains(&next_status)
        {
            Some(chrono::Utc::now().to_rfc3339())
        } else {
            None
        };

        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE worker_sandboxes
             SET status = ?2,
                 metadata_json = ?3,
                 ended_at = ?4,
                 updated_at = datetime('now')
             WHERE id = ?1",
            params![id, next_status, next_metadata, ended_at],
        )?;
        Ok(())
    }

    pub fn update_worker_sandbox_env(&self, id: &str, env_json: Option<&str>) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE worker_sandboxes
             SET env_json = ?2, updated_at = datetime('now')
             WHERE id = ?1",
            params![id, env_json],
        )?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn delete_worker_sandbox(&self, id: &str) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute("DELETE FROM worker_sandboxes WHERE id = ?1", params![id])?;
        Ok(())
    }

    // -- Learning Outcomes --

    pub fn insert_learning_outcome(
        &self,
        id: &str,
        session_id: Option<&str>,
        task_id: Option<&str>,
        outcome_type: &str,
        description: &str,
        learned_pattern: Option<&str>,
        confidence: f64,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO learning_outcomes (id, session_id, task_id, outcome_type, description, learned_pattern, confidence, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))",
            params![id, session_id, task_id, outcome_type, description, learned_pattern, confidence],
        )?;
        Ok(())
    }

    pub fn list_learning_outcomes(&self, limit: i64) -> SqlResult<Vec<LearningOutcomeRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, session_id, task_id, outcome_type, description, learned_pattern, confidence, applied_count, created_at
             FROM learning_outcomes ORDER BY created_at DESC LIMIT ?1"
        )?;
        let rows = stmt.query_map(params![limit], |row| {
            Ok(LearningOutcomeRow {
                id: row.get(0)?,
                session_id: row.get(1)?,
                task_id: row.get(2)?,
                outcome_type: row.get(3)?,
                description: row.get(4)?,
                learned_pattern: row.get(5)?,
                confidence: row.get(6)?,
                applied_count: row.get(7)?,
                created_at: row.get(8)?,
            })
        })?;
        rows.collect()
    }

    // -- Terminal Backends --

    pub fn upsert_terminal_backend(
        &self,
        id: &str,
        name: &str,
        backend_type: &str,
        config_json: &str,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO terminal_backends (id, name, backend_type, config_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'), datetime('now'))
             ON CONFLICT(name) DO UPDATE SET
                backend_type = excluded.backend_type,
                config_json = excluded.config_json,
                updated_at = datetime('now')",
            params![id, name, backend_type, config_json],
        )?;
        Ok(())
    }

    pub fn list_terminal_backends(&self) -> SqlResult<Vec<TerminalBackendRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, name, backend_type, config_json, status, last_connected_at, created_at, updated_at
             FROM terminal_backends ORDER BY name"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(TerminalBackendRow {
                id: row.get(0)?,
                name: row.get(1)?,
                backend_type: row.get(2)?,
                config_json: row.get(3)?,
                status: row.get(4)?,
                last_connected_at: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })?;
        rows.collect()
    }

    pub fn update_terminal_backend_status(&self, id: &str, status: &str) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let extra = if status == "connected" {
            ", last_connected_at = datetime('now')"
        } else {
            ""
        };
        conn.execute(
            &format!("UPDATE terminal_backends SET status = ?2{} , updated_at = datetime('now') WHERE id = ?1", extra),
            params![id, status],
        )?;
        Ok(())
    }

    pub fn delete_terminal_backend(&self, id: &str) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute("DELETE FROM terminal_backends WHERE id = ?1", params![id])?;
        Ok(())
    }

    // -- Managed Processes --

    pub fn insert_managed_process(
        &self,
        id: &str,
        label: &str,
        command: &str,
        backend_id: Option<&str>,
        requires_admin: bool,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO managed_processes (id, label, command, backend_id, requires_admin, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
            params![id, label, command, backend_id, requires_admin as i32],
        )?;
        Ok(())
    }

    pub fn update_process_status(
        &self,
        id: &str,
        status: &str,
        pid: Option<i64>,
        exit_code: Option<i32>,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let extra_field = match status {
            "running" => ", started_at = datetime('now')",
            "stopped" | "failed" | "killed" => ", stopped_at = datetime('now')",
            _ => "",
        };
        conn.execute(
            &format!("UPDATE managed_processes SET status = ?2, pid = ?3, exit_code = ?4{} WHERE id = ?1", extra_field),
            params![id, status, pid, exit_code],
        )?;
        Ok(())
    }

    pub fn update_process_exit_if_running(
        &self,
        id: &str,
        status: &str,
        exit_code: Option<i32>,
    ) -> SqlResult<bool> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let changed = conn.execute(
            "UPDATE managed_processes
             SET status = ?2, pid = NULL, exit_code = ?3, stopped_at = datetime('now')
             WHERE id = ?1 AND status = 'running'",
            params![id, status, exit_code],
        )?;
        Ok(changed > 0)
    }

    pub fn approve_process_admin(&self, id: &str) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE managed_processes SET admin_approved = 1 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    pub fn list_managed_processes(&self) -> SqlResult<Vec<ManagedProcessRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, label, command, backend_id, pid, status, exit_code, requires_admin, admin_approved, log_path, started_at, stopped_at, created_at
             FROM managed_processes ORDER BY created_at DESC"
        )?;
        let rows = stmt.query_map([], |row| {
            let req_admin: i32 = row.get(7)?;
            let admin_approved: i32 = row.get(8)?;
            Ok(ManagedProcessRow {
                id: row.get(0)?,
                label: row.get(1)?,
                command: row.get(2)?,
                backend_id: row.get(3)?,
                pid: row.get(4)?,
                status: row.get(5)?,
                exit_code: row.get(6)?,
                requires_admin: req_admin != 0,
                admin_approved: admin_approved != 0,
                log_path: row.get(9)?,
                started_at: row.get(10)?,
                stopped_at: row.get(11)?,
                created_at: row.get(12)?,
            })
        })?;
        rows.collect()
    }

    #[allow(dead_code)]
    pub fn delete_managed_process(&self, id: &str) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute("DELETE FROM managed_processes WHERE id = ?1", params![id])?;
        Ok(())
    }

    // -- Agent Personalities --

    pub fn upsert_personality(
        &self,
        id: &str,
        name: &str,
        description: &str,
        role: &str,
        goal: &str,
        system_prompt: &str,
        skills_markdown: &str,
        temperature: Option<f64>,
        model_override: Option<&str>,
        icon: Option<&str>,
        is_default: bool,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        if is_default {
            conn.execute("UPDATE agent_personalities SET is_default = 0", [])?;
        }
        conn.execute(
            "INSERT INTO agent_personalities (id, name, description, role, goal, system_prompt, skills_markdown, temperature, model_override, icon, is_default, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, datetime('now'), datetime('now'))
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                description = excluded.description,
                role = excluded.role,
                goal = excluded.goal,
                system_prompt = excluded.system_prompt,
                skills_markdown = excluded.skills_markdown,
                temperature = excluded.temperature,
                model_override = excluded.model_override,
                icon = excluded.icon,
                is_default = excluded.is_default,
                updated_at = datetime('now')",
            params![
                id,
                name,
                description,
                role,
                goal,
                system_prompt,
                skills_markdown,
                temperature,
                model_override,
                icon,
                is_default as i32
            ],
        )?;
        Ok(())
    }

    pub fn list_personalities(&self) -> SqlResult<Vec<PersonalityRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, name, description, role, goal, system_prompt, skills_markdown, temperature, model_override, icon, is_default, created_at, updated_at
             FROM agent_personalities ORDER BY name"
        )?;
        let rows = stmt.query_map([], |row| {
            let is_default: i32 = row.get(10)?;
            Ok(PersonalityRow {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                role: row.get(3)?,
                goal: row.get(4)?,
                system_prompt: row.get(5)?,
                skills_markdown: row.get(6)?,
                temperature: row.get(7)?,
                model_override: row.get(8)?,
                icon: row.get(9)?,
                is_default: is_default != 0,
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
            })
        })?;
        rows.collect()
    }

    pub fn delete_personality(&self, id: &str) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute("DELETE FROM agent_personalities WHERE id = ?1", params![id])?;
        Ok(())
    }

    // -- Insights Events --

    pub fn insert_insights_event(
        &self,
        id: &str,
        event_type: &str,
        category: &str,
        value_num: Option<f64>,
        value_text: Option<&str>,
        session_id: Option<&str>,
        metadata_json: Option<&str>,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO insights_events (id, event_type, category, value_num, value_text, session_id, metadata_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))",
            params![id, event_type, category, value_num, value_text, session_id, metadata_json],
        )?;
        Ok(())
    }

    pub fn query_insights(
        &self,
        category: Option<&str>,
        event_type: Option<&str>,
        limit: i64,
    ) -> SqlResult<Vec<InsightsEventRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let sql = match (category, event_type) {
            (Some(_), Some(_)) => {
                "SELECT id, event_type, category, value_num, value_text, session_id, metadata_json, created_at FROM insights_events WHERE category = ?1 AND event_type = ?2 ORDER BY created_at DESC LIMIT ?3"
            }
            (Some(_), None) => {
                "SELECT id, event_type, category, value_num, value_text, session_id, metadata_json, created_at FROM insights_events WHERE category = ?1 ORDER BY created_at DESC LIMIT ?2"
            }
            (None, Some(_)) => {
                "SELECT id, event_type, category, value_num, value_text, session_id, metadata_json, created_at FROM insights_events WHERE event_type = ?1 ORDER BY created_at DESC LIMIT ?2"
            }
            (None, None) => {
                "SELECT id, event_type, category, value_num, value_text, session_id, metadata_json, created_at FROM insights_events ORDER BY created_at DESC LIMIT ?1"
            }
        };

        let mut stmt = conn.prepare(sql)?;
        let rows: Vec<InsightsEventRow> = match (category, event_type) {
            (Some(cat), Some(et)) => stmt
                .query_map(params![cat, et, limit], map_insights_row)?
                .collect::<SqlResult<Vec<_>>>()?,
            (Some(cat), None) => stmt
                .query_map(params![cat, limit], map_insights_row)?
                .collect::<SqlResult<Vec<_>>>()?,
            (None, Some(et)) => stmt
                .query_map(params![et, limit], map_insights_row)?
                .collect::<SqlResult<Vec<_>>>()?,
            (None, None) => stmt
                .query_map(params![limit], map_insights_row)?
                .collect::<SqlResult<Vec<_>>>()?,
        };
        Ok(rows)
    }

    #[allow(dead_code)]
    pub fn get_insights_summary(&self, days: i64) -> SqlResult<Vec<(String, String, i64, f64)>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT category, event_type, COUNT(*) as cnt, COALESCE(AVG(value_num), 0) as avg_val
             FROM insights_events WHERE created_at >= datetime('now', ?1 || ' days')
             GROUP BY category, event_type ORDER BY cnt DESC",
        )?;
        let rows = stmt.query_map(params![-days], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
        rows.collect()
    }

    // -- RPC Pipelines --

    pub fn upsert_rpc_pipeline(
        &self,
        id: &str,
        name: &str,
        description: Option<&str>,
        steps_json: &str,
        zero_context: bool,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO rpc_pipelines (id, name, description, steps_json, zero_context, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'), datetime('now'))
             ON CONFLICT(name) DO UPDATE SET
                description = excluded.description,
                steps_json = excluded.steps_json,
                zero_context = excluded.zero_context,
                updated_at = datetime('now')",
            params![id, name, description, steps_json, zero_context as i32],
        )?;
        Ok(())
    }

    pub fn list_rpc_pipelines(&self) -> SqlResult<Vec<RpcPipelineRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, name, description, steps_json, zero_context, created_at, updated_at
             FROM rpc_pipelines ORDER BY name",
        )?;
        let rows = stmt.query_map([], |row| {
            let zc: i32 = row.get(4)?;
            Ok(RpcPipelineRow {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                steps_json: row.get(3)?,
                zero_context: zc != 0,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    pub fn delete_rpc_pipeline(&self, id: &str) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute("DELETE FROM rpc_pipelines WHERE id = ?1", params![id])?;
        Ok(())
    }

    // -- Memory Providers --

    pub fn upsert_memory_provider(
        &self,
        id: &str,
        name: &str,
        provider_type: &str,
        config_json: &str,
        enabled: bool,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO memory_providers (id, name, provider_type, config_json, enabled, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'), datetime('now'))
             ON CONFLICT(name) DO UPDATE SET
                provider_type = excluded.provider_type,
                config_json = excluded.config_json,
                enabled = excluded.enabled,
                updated_at = datetime('now')",
            params![id, name, provider_type, config_json, enabled as i32],
        )?;
        Ok(())
    }

    pub fn list_memory_providers(&self) -> SqlResult<Vec<MemoryProviderRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, name, provider_type, config_json, enabled, last_sync_at, created_at, updated_at
             FROM memory_providers ORDER BY name"
        )?;
        let rows = stmt.query_map([], |row| {
            let enabled: i32 = row.get(4)?;
            Ok(MemoryProviderRow {
                id: row.get(0)?,
                name: row.get(1)?,
                provider_type: row.get(2)?,
                config_json: row.get(3)?,
                enabled: enabled != 0,
                last_sync_at: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })?;
        rows.collect()
    }

    pub fn delete_memory_provider(&self, id: &str) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute("DELETE FROM memory_providers WHERE id = ?1", params![id])?;
        Ok(())
    }

    // -- Tool Gateway --

    pub fn upsert_tool_gateway_entry(
        &self,
        id: &str,
        tool_type: &str,
        name: &str,
        config_json: &str,
        enabled: bool,
    ) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO tool_gateway_entries (id, tool_type, name, config_json, enabled, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'), datetime('now'))
             ON CONFLICT(name) DO UPDATE SET
                tool_type = excluded.tool_type,
                config_json = excluded.config_json,
                enabled = excluded.enabled,
                updated_at = datetime('now')",
            params![id, tool_type, name, config_json, enabled as i32],
        )?;
        Ok(())
    }

    pub fn list_tool_gateway_entries(&self) -> SqlResult<Vec<ToolGatewayRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, tool_type, name, config_json, enabled, created_at, updated_at
             FROM tool_gateway_entries ORDER BY name",
        )?;
        let rows = stmt.query_map([], |row| {
            let enabled: i32 = row.get(4)?;
            Ok(ToolGatewayRow {
                id: row.get(0)?,
                tool_type: row.get(1)?,
                name: row.get(2)?,
                config_json: row.get(3)?,
                enabled: enabled != 0,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    pub fn delete_tool_gateway_entry(&self, id: &str) -> SqlResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "DELETE FROM tool_gateway_entries WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    // -- Session Full-Text Search --

    pub fn fulltext_search_sessions(
        &self,
        query: &str,
        limit: i64,
    ) -> SqlResult<Vec<SessionSearchResultRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let pattern = format!("%{}%", query);
        let mut stmt = conn.prepare(
            "SELECT s.id, s.title, s.summary, s.started_at, s.ended_at,
                    m.id, m.content, m.role, m.timestamp
             FROM sessions s
             LEFT JOIN chat_threads t ON t.id = s.thread_id
             LEFT JOIN chat_messages m ON m.thread_id = t.id AND m.content LIKE ?1
             WHERE s.title LIKE ?1 OR s.summary LIKE ?1 OR m.content LIKE ?1
             ORDER BY s.started_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![pattern, limit], |row| {
            Ok(SessionSearchResultRow {
                session_id: row.get(0)?,
                session_title: row.get(1)?,
                session_summary: row.get(2)?,
                started_at: row.get(3)?,
                ended_at: row.get(4)?,
                matched_message_id: row.get(5)?,
                matched_content: row.get(6)?,
                matched_role: row.get(7)?,
                matched_timestamp: row.get(8)?,
            })
        })?;
        rows.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database_test_dir(label: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("open-cowork-db-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("database test directory should be created");
        root
    }

    fn insert_test_engine_run(db: &Database, id: &str) {
        db.insert_engine_run(
            id,
            None,
            None,
            None,
            "Test Run",
            Some("test input"),
            "pending",
            "queued",
            Some("C:/workspace"),
            Some("gpt-test"),
            Some("openai"),
            0,
            None,
            None,
            Some("{}"),
        )
        .unwrap();
    }

    #[test]
    fn migration_creates_tables() {
        let db = Database::open_in_memory().unwrap();
        db.insert_thread(
            "t1",
            "Test Thread",
            "2025-01-01T00:00:00",
            Some("{\"provider\":\"ollama\"}"),
            None,
        )
        .unwrap();
        let threads = db.list_threads().unwrap();
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].0, "t1");
        assert_eq!(threads[0].4.as_deref(), Some("{\"provider\":\"ollama\"}"));
    }

    #[test]
    fn connection_pragmas_enforce_integrity_and_contention_policy() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn.lock().unwrap();
        let foreign_keys: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        let busy_timeout: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        let synchronous: i64 = conn
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .unwrap();
        let temp_store: i64 = conn
            .query_row("PRAGMA temp_store", [], |row| row.get(0))
            .unwrap();

        assert_eq!(foreign_keys, 1);
        assert_eq!(busy_timeout, SQLITE_BUSY_TIMEOUT.as_millis() as i64);
        assert_eq!(synchronous, 2, "FULL synchronous mode must remain enabled");
        assert_eq!(temp_store, 2, "temporary data must stay in memory");
    }

    #[test]
    fn persistent_database_uses_wal_and_survives_reopen() {
        let root = database_test_dir("wal");
        {
            let db = Database::open(root.clone()).unwrap();
            let conn = db.conn.lock().unwrap();
            let journal_mode: String = conn
                .query_row("PRAGMA journal_mode", [], |row| row.get(0))
                .unwrap();
            let version: i64 = conn
                .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
                .unwrap();
            assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
            assert_eq!(version, LATEST_SCHEMA_VERSION);
        }

        let reopened = Database::open(root.clone()).unwrap();
        reopened.ensure_integrity().unwrap();
        drop(reopened);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn upgrade_creates_a_verified_pre_migration_backup() {
        let root = database_test_dir("backup");
        let db_path = root.join("open_cowork.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "PRAGMA foreign_keys=ON;
                 CREATE TABLE schema_version (version INTEGER NOT NULL);
                 INSERT INTO schema_version (version) VALUES (22);
                 CREATE TABLE chat_threads (id TEXT PRIMARY KEY);",
            )
            .unwrap();
        }

        let db = Database::open(root.clone()).unwrap();
        let current_version: i64 = db
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(current_version, LATEST_SCHEMA_VERSION);
        drop(db);

        let backup_dir = root.join("database-backups");
        let backups = std::fs::read_dir(&backup_dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("db"))
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 1);
        let backup = Connection::open(&backups[0]).unwrap();
        ensure_database_integrity(&backup).unwrap();
        let backup_version: i64 = backup
            .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
            .unwrap();
        let backup_has_work_tasks: i64 = backup
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM sqlite_master
                    WHERE type = 'table' AND name = 'work_tasks'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(backup_version, 22);
        assert_eq!(backup_has_work_tasks, 0);
        drop(backup);

        Database::open(root.clone()).unwrap();
        assert_eq!(std::fs::read_dir(&backup_dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn failed_migration_rolls_back_partial_schema_changes() {
        let conn = Connection::open_in_memory().unwrap();
        configure_connection(&conn, false).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER NOT NULL);
             INSERT INTO schema_version (version) VALUES (22);
             CREATE TABLE work_tasks (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL
             );",
        )
        .unwrap();
        let db = Database {
            conn: Mutex::new(conn),
        };

        assert!(db.migrate().is_err());
        let conn = db.conn.lock().unwrap();
        let version: i64 = conn
            .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
            .unwrap();
        let partial_index_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index' AND name = 'idx_work_tasks_created'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, 22);
        assert_eq!(partial_index_count, 0);
    }

    #[test]
    fn newer_schema_versions_are_rejected_without_modification() {
        let conn = Connection::open_in_memory().unwrap();
        configure_connection(&conn, false).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER NOT NULL);
             INSERT INTO schema_version (version) VALUES (24);",
        )
        .unwrap();
        let db = Database {
            conn: Mutex::new(conn),
        };

        let error = db.migrate().unwrap_err().to_string();
        assert!(error.contains("unsupported database schema version"));
        let version: i64 = db
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 24);
    }

    #[test]
    fn integrity_check_detects_foreign_key_violations() {
        let db = Database::open_in_memory().unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute_batch(
                "PRAGMA foreign_keys=OFF;
                 INSERT INTO project_resources (
                    id, project_id, kind, path, label, enabled, added_at
                 ) VALUES (
                    'orphan', 'missing-project', 'file', 'C:/missing.txt', NULL, 1, datetime('now')
                 );
                 PRAGMA foreign_keys=ON;",
            )
            .unwrap();
        }

        let error = db.ensure_integrity().unwrap_err().to_string();
        assert!(error.contains("foreign key integrity check failed"));
        assert!(error.contains("project_resources"));
    }

    #[test]
    fn malformed_schema_version_tables_are_rejected() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER NOT NULL);
             INSERT INTO schema_version (version) VALUES (22), (23);",
        )
        .unwrap();

        let error = current_schema_version(&conn).unwrap_err().to_string();
        assert!(error.contains("exactly one integer row"));
        assert!(ensure_supported_schema_version(-1).is_err());
    }

    #[test]
    fn corrupt_database_files_and_invalid_data_directories_are_rejected() {
        let corrupt_root = database_test_dir("corrupt");
        std::fs::write(
            corrupt_root.join("open_cowork.db"),
            b"not a sqlite database",
        )
        .unwrap();
        assert!(Database::open(corrupt_root.clone()).is_err());

        let invalid_parent = database_test_dir("invalid-path");
        let invalid_path = invalid_parent.join("app-data-file");
        std::fs::write(&invalid_path, b"file, not directory").unwrap();
        assert!(Database::open(invalid_path).is_err());

        let _ = std::fs::remove_dir_all(corrupt_root);
        let _ = std::fs::remove_dir_all(invalid_parent);
    }

    #[test]
    fn messages_round_trip() {
        let db = Database::open_in_memory().unwrap();
        db.insert_thread("t1", "Thread", "2025-01-01T00:00:00", None, None)
            .unwrap();
        db.insert_message("m1", "t1", "user", "Hello", 1000)
            .unwrap();
        db.insert_message("m2", "t1", "assistant", "Hi", 1001)
            .unwrap();
        let msgs = db.list_messages("t1").unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].1, "user");
        assert_eq!(msgs[1].1, "assistant");
    }

    #[test]
    fn session_search_finds_linked_persisted_chat_messages() {
        let db = Database::open_in_memory().unwrap();
        db.insert_thread(
            "thread-memory",
            "Memory thread",
            "2026-07-16T10:00:00Z",
            None,
            None,
        )
        .unwrap();
        db.insert_message(
            "message-memory",
            "thread-memory",
            "assistant",
            "The project selected SQLite for durable local memory.",
            1_752_660_000,
        )
        .unwrap();
        db.insert_session(
            "session-memory",
            Some("thread-memory"),
            "Memory decision",
            Some("{\"sessionId\":\"session-memory\"}"),
            Some("test-model"),
            Some("ollama"),
            None,
        )
        .unwrap();

        let matches = db.fulltext_search_sessions("SQLite", 10).unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].session_id, "session-memory");
        assert_eq!(
            matches[0].matched_content.as_deref(),
            Some("The project selected SQLite for durable local memory.")
        );
    }

    #[test]
    fn creating_an_existing_session_does_not_replace_its_frozen_snapshot() {
        let db = Database::open_in_memory().unwrap();
        db.insert_session(
            "session-frozen",
            None,
            "Original",
            Some("{\"version\":1}"),
            None,
            None,
            None,
        )
        .unwrap();
        db.insert_session(
            "session-frozen",
            None,
            "Later title",
            Some("{\"version\":2}"),
            None,
            None,
            None,
        )
        .unwrap();

        assert_eq!(
            db.get_session_memory_snapshot("session-frozen")
                .unwrap()
                .as_deref(),
            Some("{\"version\":1}")
        );
    }

    #[test]
    fn task_lifecycle() {
        let db = Database::open_in_memory().unwrap();
        db.insert_task(
            "task1",
            "Test",
            "Do stuff",
            "created",
            None,
            "2025-01-01T00:00:00",
        )
        .unwrap();
        db.update_task_status("task1", "planned").unwrap();
        let tasks = db.list_tasks().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].3, "planned");
    }

    #[test]
    fn steps_with_task() {
        let db = Database::open_in_memory().unwrap();
        db.insert_task(
            "task1",
            "Test",
            "Do stuff",
            "created",
            None,
            "2025-01-01T00:00:00",
        )
        .unwrap();
        db.insert_step("s1", "task1", 0, "Step 1", "pending", false, "low")
            .unwrap();
        db.insert_step("s2", "task1", 1, "Step 2", "pending", true, "medium")
            .unwrap();
        let steps = db.list_steps("task1").unwrap();
        assert_eq!(steps.len(), 2);
        assert!(steps[1].4); // requires_approval
    }

    #[test]
    fn work_task_lifecycle_round_trip() {
        let db = Database::open_in_memory().unwrap();
        let mut row = WorkTaskRow {
            id: "work-1".to_string(),
            title: "Weekly Report".to_string(),
            prompt: "Summarize the week".to_string(),
            expected_output: "Bullet report".to_string(),
            work_dir: "C:/workspace".to_string(),
            thread_id: None,
            runner: "model".to_string(),
            crew_id: None,
            model: "qwen3".to_string(),
            schedule_expr: "daily 09:00".to_string(),
            schedule_enabled: true,
            status: "idle".to_string(),
            output: None,
            error: None,
            last_run_at: None,
            created_at: "2026-07-02T08:00:00Z".to_string(),
            updated_at: "2026-07-02T08:00:00Z".to_string(),
        };

        db.upsert_work_task(&row).unwrap();
        let listed = db.list_work_tasks().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "work-1");
        assert!(listed[0].schedule_enabled);

        db.update_work_task_status(
            "work-1",
            "completed",
            Some("done"),
            None,
            Some("2026-07-02T08:10:00Z"),
            "2026-07-02T08:10:00Z",
        )
        .unwrap();
        let completed = db.get_work_task("work-1").unwrap().unwrap();
        assert_eq!(completed.status, "completed");
        assert_eq!(completed.output.as_deref(), Some("done"));
        assert_eq!(
            completed.last_run_at.as_deref(),
            Some("2026-07-02T08:10:00Z")
        );

        row.title = "Weekly Report Updated".to_string();
        row.status = "idle".to_string();
        row.schedule_enabled = false;
        row.updated_at = "2026-07-02T08:20:00Z".to_string();
        db.upsert_work_task(&row).unwrap();
        let updated = db.get_work_task("work-1").unwrap().unwrap();
        assert_eq!(updated.title, "Weekly Report Updated");
        assert!(!updated.schedule_enabled);
        assert_eq!(updated.created_at, "2026-07-02T08:00:00Z");
    }

    #[test]
    fn delete_work_task_removes_matching_schedule() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_work_task(&WorkTaskRow {
            id: "work-1".to_string(),
            title: "Scheduled Work".to_string(),
            prompt: "Run me".to_string(),
            expected_output: "".to_string(),
            work_dir: "".to_string(),
            thread_id: None,
            runner: "crew".to_string(),
            crew_id: Some("crew-1".to_string()),
            model: "".to_string(),
            schedule_expr: "daily 09:00".to_string(),
            schedule_enabled: true,
            status: "idle".to_string(),
            output: None,
            error: None,
            last_run_at: None,
            created_at: "2026-07-02T08:00:00Z".to_string(),
            updated_at: "2026-07-02T08:00:00Z".to_string(),
        })
        .unwrap();
        db.upsert_scheduled_task(
            "work-1",
            "Scheduled Work",
            "Run me",
            "daily 09:00",
            "crew",
            Some("crew-1"),
            Some("{}"),
            None,
            100,
            "[]",
            true,
            None,
            Some("2026-07-03T09:00:00Z"),
            "2026-07-02T08:00:00Z",
        )
        .unwrap();

        assert_eq!(db.list_scheduled_tasks().unwrap().len(), 1);
        db.delete_work_task("work-1").unwrap();
        assert!(db.get_work_task("work-1").unwrap().is_none());
        assert!(db.list_scheduled_tasks().unwrap().is_empty());
    }

    #[test]
    fn delete_thread_cascades() {
        let db = Database::open_in_memory().unwrap();
        db.insert_thread("t1", "Thread", "2025-01-01T00:00:00", None, None)
            .unwrap();
        db.insert_message("m1", "t1", "user", "Hello", 1000)
            .unwrap();
        db.delete_thread("t1").unwrap();
        let msgs = db.list_messages("t1").unwrap();
        assert_eq!(msgs.len(), 0);
    }

    #[test]
    fn projects_resources_and_threads_round_trip() {
        let db = Database::open_in_memory().unwrap();
        db.insert_thread("t1", "Thread", "2025-01-01T00:00:00", None, None)
            .unwrap();
        db.upsert_project(
            "p1",
            "Kundenanalyse",
            "Answer concisely.",
            "2026-05-11T09:00:00Z",
            "2026-05-11T09:00:00Z",
        )
        .unwrap();
        db.upsert_project_resource(
            "r1",
            "p1",
            "file",
            "C:/docs/spec.md",
            None,
            true,
            "2026-05-11T09:01:00Z",
        )
        .unwrap();
        db.upsert_project_resource(
            "r2",
            "p1",
            "link",
            "https://example.com",
            Some("Example"),
            false,
            "2026-05-11T09:02:00Z",
        )
        .unwrap();
        db.attach_project_thread("p1", "t1").unwrap();

        let projects = db.list_projects().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].instructions, "Answer concisely.");

        let resources = db.list_project_resources().unwrap();
        assert_eq!(resources.len(), 2);
        assert!(resources
            .iter()
            .any(|resource| resource.kind == "link" && !resource.enabled));

        let threads = db.list_project_threads().unwrap();
        assert_eq!(threads, vec![("p1".to_string(), "t1".to_string())]);
    }

    #[test]
    fn project_thread_assignment_is_exclusive_and_delete_can_remove_threads() {
        let db = Database::open_in_memory().unwrap();
        db.insert_thread("t1", "Thread", "2025-01-01T00:00:00", None, None)
            .unwrap();
        db.upsert_project(
            "p1",
            "Alpha",
            "",
            "2026-05-11T09:00:00Z",
            "2026-05-11T09:00:00Z",
        )
        .unwrap();
        db.upsert_project(
            "p2",
            "Beta",
            "",
            "2026-05-11T09:00:00Z",
            "2026-05-11T09:00:00Z",
        )
        .unwrap();

        db.attach_project_thread("p1", "t1").unwrap();
        db.attach_project_thread("p2", "t1").unwrap();
        assert_eq!(
            db.list_project_threads().unwrap(),
            vec![("p2".to_string(), "t1".to_string())]
        );

        let deleted_threads = db.delete_project("p2", true).unwrap();
        assert_eq!(deleted_threads, vec!["t1".to_string()]);
        assert!(db.list_threads().unwrap().is_empty());
    }

    #[test]
    fn audit_event_insert() {
        let db = Database::open_in_memory().unwrap();
        db.insert_audit_event(
            "a1",
            "2025-01-01T00:00:00",
            "task_created",
            Some("task"),
            Some("task1"),
            None,
        )
        .unwrap();
    }

    #[test]
    fn artifact_versions_round_trip() {
        let db = Database::open_in_memory().unwrap();
        db.insert_artifact_version(
            "v1",
            Some("run-1"),
            Some("initial"),
            "C:/tmp/report.md",
            "text/plain",
            123,
            "summary",
            "preview",
            "{}",
            "2026-04-16T19:00:00Z",
        )
        .unwrap();

        let rows = db.list_artifact_versions(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "v1");
        assert_eq!(rows[0].1.as_deref(), Some("run-1"));
        assert_eq!(rows[0].3, "C:/tmp/report.md");
    }

    #[test]
    fn artifact_exports_round_trip() {
        let db = Database::open_in_memory().unwrap();
        db.insert_artifact_version(
            "v1",
            Some("run-1"),
            Some("initial"),
            "C:/tmp/report.md",
            "text/plain",
            123,
            "summary",
            "preview",
            "{}",
            "2026-04-16T19:00:00Z",
        )
        .unwrap();

        db.insert_artifact_export(
            "e1",
            "v1",
            "json",
            "C:/tmp/export/report.json",
            512,
            "2026-04-16T19:01:00Z",
        )
        .unwrap();

        let rows = db.list_artifact_exports(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "e1");
        assert_eq!(rows[0].1, "v1");
        assert_eq!(rows[0].2, "json");
    }

    #[test]
    fn policy_settings_round_trip() {
        let db = Database::open_in_memory().unwrap();

        assert_eq!(
            db.get_policy_setting("activeToolsetPolicyId").unwrap(),
            None
        );

        db.set_policy_setting("activeToolsetPolicyId", "safe_research")
            .unwrap();
        assert_eq!(
            db.get_policy_setting("activeToolsetPolicyId")
                .unwrap()
                .as_deref(),
            Some("safe_research")
        );

        db.set_policy_setting("activeToolsetPolicyId", "code_edit")
            .unwrap();
        assert_eq!(
            db.get_policy_setting("activeToolsetPolicyId")
                .unwrap()
                .as_deref(),
            Some("code_edit")
        );
    }

    #[test]
    fn engine_runs_capture_gateway_metadata() {
        let db = Database::open_in_memory().unwrap();
        insert_test_engine_run(&db, "run-defaults");

        let defaults = db.get_engine_run("run-defaults").unwrap().unwrap();
        assert_eq!(defaults.source, "desktop");
        assert_eq!(defaults.runtime_mode, "host");
        assert_eq!(defaults.workspace_path, None);
        assert_eq!(defaults.provider_profile_id, None);
        assert_eq!(defaults.toolset_policy_id, None);

        db.insert_thread(
            "thread-1",
            "Gateway Thread",
            "2025-01-01T00:00:00",
            None,
            None,
        )
        .unwrap();

        db.insert_engine_run_with_gateway_metadata(
            "run-gateway",
            None,
            Some("thread-1"),
            None,
            "Gateway Run",
            None,
            "running",
            "planning",
            Some("C:/workspace"),
            Some("claude-sonnet"),
            Some("anthropic"),
            Some("cli"),
            Some("C:/workspace"),
            Some("anthropic-default"),
            Some("subprocess"),
            Some("policy-strict"),
            Some("desktop-thread"),
            Some("thread-1"),
            1,
            None,
            Some("{\"step\":1}"),
            Some("{\"gateway\":true}"),
        )
        .unwrap();

        let run = db.get_engine_run("run-gateway").unwrap().unwrap();
        assert_eq!(run.source, "cli");
        assert_eq!(run.workspace_path.as_deref(), Some("C:/workspace"));
        assert_eq!(
            run.provider_profile_id.as_deref(),
            Some("anthropic-default")
        );
        assert_eq!(run.runtime_mode, "subprocess");
        assert_eq!(run.toolset_policy_id.as_deref(), Some("policy-strict"));
        assert_eq!(run.channel_kind.as_deref(), Some("desktop-thread"));
        assert_eq!(run.channel_ref.as_deref(), Some("thread-1"));
        assert!(run.started_at.is_some());
    }

    #[test]
    fn engine_run_events_are_ordered_and_summarized() {
        let db = Database::open_in_memory().unwrap();
        insert_test_engine_run(&db, "run-events");

        db.insert_engine_run_event("event-1", "run-events", "started", None)
            .unwrap();
        db.insert_engine_run_event_with_details(
            "event-2",
            "run-events",
            "tool_call",
            Some("Shell command completed"),
            Some("{\"exitCode\":0}"),
            Some("metadata"),
        )
        .unwrap();

        let events = db.list_engine_run_events("run-events", 10).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].sequence, 1);
        assert_eq!(events[0].event_type, "started");
        assert_eq!(events[0].summary, "started");
        assert_eq!(events[0].redaction_level, "none");
        assert_eq!(events[1].sequence, 2);
        assert_eq!(events[1].summary, "Shell command completed");
        assert_eq!(events[1].payload_json.as_deref(), Some("{\"exitCode\":0}"));
        assert_eq!(events[1].redaction_level, "metadata");
    }

    #[test]
    fn engine_run_artifacts_round_trip_and_cascade() {
        let db = Database::open_in_memory().unwrap();
        insert_test_engine_run(&db, "run-artifacts");

        db.insert_engine_run_artifact(
            "artifact-1",
            "run-artifacts",
            "patch",
            "C:/workspace/changes.diff",
            Some("Proposed patch"),
            Some("Patch generated by the agent"),
        )
        .unwrap();
        db.insert_engine_run_event("event-1", "run-artifacts", "artifact_created", None)
            .unwrap();

        let artifacts = db.list_engine_run_artifacts("run-artifacts", 10).unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].kind, "patch");
        assert_eq!(artifacts[0].path, "C:/workspace/changes.diff");
        assert_eq!(artifacts[0].title.as_deref(), Some("Proposed patch"));

        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "DELETE FROM engine_runs WHERE id = ?1",
                rusqlite::params!["run-artifacts"],
            )
            .unwrap();
        }

        assert!(db
            .list_engine_run_artifacts("run-artifacts", 10)
            .unwrap()
            .is_empty());
        assert!(db
            .list_engine_run_events("run-artifacts", 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn diagnostic_database_sinks_redact_before_persistence() {
        let db = Database::open_in_memory().unwrap();
        insert_test_engine_run(&db, "run-redaction");

        db.insert_engine_run_event_with_details(
            "event-redaction",
            "run-redaction",
            "tool_result",
            Some("Authorization: Bearer event-summary-secret"),
            Some(r#"{"apiKey":"event-json-secret","stdout":"token=event-output-secret"}"#),
            None,
        )
        .unwrap();
        db.insert_audit_event(
            "audit-redaction",
            "2026-07-10T12:00:00Z",
            "tool_result",
            None,
            None,
            Some(r#"{"password":"audit-json-secret"}"#),
        )
        .unwrap();

        let event = db
            .list_engine_run_events("run-redaction", 10)
            .unwrap()
            .pop()
            .expect("event persists");
        let event_text = format!("{} {:?}", event.summary, event.payload_json);
        assert!(!event_text.contains("event-summary-secret"));
        assert!(!event_text.contains("event-json-secret"));
        assert!(!event_text.contains("event-output-secret"));
        assert_eq!(event.redaction_level, "automatic");

        let audit_details: String = db
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT details_json FROM audit_events WHERE id = ?1",
                params!["audit-redaction"],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!audit_details.contains("audit-json-secret"));
        assert!(audit_details.contains("[REDACTED]"));
    }

    #[test]
    fn engine_event_retention_keeps_the_latest_bounded_window() {
        let db = Database::open_in_memory().unwrap();
        insert_test_engine_run(&db, "run-retention");

        for index in 0..(MAX_ENGINE_EVENTS_PER_RUN + 5) {
            db.insert_engine_run_event(
                &format!("event-{index}"),
                "run-retention",
                "progress",
                None,
            )
            .unwrap();
        }

        let events = db
            .list_engine_run_events("run-retention", MAX_ENGINE_EVENTS_PER_RUN + 10)
            .unwrap();
        assert_eq!(events.len() as i64, MAX_ENGINE_EVENTS_PER_RUN);
        assert_eq!(events.first().map(|event| event.sequence), Some(6));
        assert_eq!(
            events.last().map(|event| event.sequence),
            Some(MAX_ENGINE_EVENTS_PER_RUN + 5)
        );
    }

    #[test]
    fn startup_recovery_reconciles_active_state_after_reopen_and_is_idempotent() {
        let root = database_test_dir("startup-recovery");
        {
            let db = Database::open(root.clone()).unwrap();
            db.insert_engine_run(
                "engine-active",
                None,
                None,
                None,
                "Active run",
                None,
                "running",
                "tool:shell",
                None,
                None,
                None,
                0,
                None,
                None,
                None,
            )
            .unwrap();
            {
                let conn = db.conn.lock().unwrap();
                conn.execute_batch(
                    "INSERT INTO tasks (id, title, prompt, status, created_at, updated_at)
                     VALUES ('legacy-active', 'Legacy active', 'prompt', 'running', '2026-07-10T10:00:00Z', '2026-07-10T10:00:00Z');
                     INSERT INTO task_steps (id, task_id, idx, title, state)
                     VALUES ('step-active', 'legacy-active', 0, 'Step', 'running');
                     INSERT INTO tasks (id, title, prompt, status, created_at, updated_at)
                     VALUES ('legacy-waiting', 'Legacy waiting', 'prompt', 'waiting_approval', '2026-07-10T10:00:00Z', '2026-07-10T10:00:00Z');
                     INSERT INTO work_tasks (
                       id, title, prompt, runner, status, created_at, updated_at
                     ) VALUES (
                       'work-active', 'Work active', 'prompt', 'model', 'running',
                       '2026-07-10T10:00:00Z', '2026-07-10T10:00:00Z'
                     );
                     INSERT INTO scheduled_tasks (
                       id, name, prompt, schedule_expr, active, created_at, updated_at
                     ) VALUES (
                       'schedule-active', 'Schedule', 'prompt', 'hourly', 1,
                       '2026-07-10T10:00:00Z', '2026-07-10T10:00:00Z'
                     );
                     INSERT INTO terminal_backends (
                       id, name, backend_type, config_json, status, created_at, updated_at
                     ) VALUES (
                       'backend-active', 'Backend', 'local', '{}', 'connected',
                       '2026-07-10T10:00:00Z', '2026-07-10T10:00:00Z'
                     );
                     INSERT INTO sessions (id, title, started_at)
                     VALUES ('session-open', 'Open session', '2026-07-10T10:00:00Z');
                     INSERT INTO crew_approvals (
                       id, approval_type, status, requested_at, created_at, updated_at
                     ) VALUES (
                       'approval-pending', 'run_gate', 'pending', '2026-07-10T10:00:00Z',
                       '2026-07-10T10:00:00Z', '2026-07-10T10:00:00Z'
                     );",
                )
                .unwrap();
            }
            db.insert_worker_sandbox(
                "sandbox-active",
                "engine-active",
                None,
                Some("backend-active"),
                "active",
                "workspace_copy",
                "C:/workspace",
                "C:/sandbox",
                "[]",
                None,
                true,
                true,
                true,
                false,
                false,
                false,
                None,
                None,
            )
            .unwrap();
            db.insert_crew_run(
                "crew-active",
                "crew-1",
                "Crew",
                "sequential",
                "running",
                None,
                None,
                "{}",
                "2026-07-10T10:00:00Z",
                None,
            )
            .unwrap();
            assert!(db
                .begin_scheduled_run(
                    "scheduled-active",
                    "schedule-active",
                    "2026-07-10T10:00:00Z",
                    Some("2026-07-10T11:00:00Z"),
                )
                .unwrap());
            db.insert_managed_process(
                "process-active",
                "Process",
                "test-command",
                Some("backend-active"),
                false,
            )
            .unwrap();
            db.update_process_status("process-active", "running", Some(42), None)
                .unwrap();
        }

        let db = Database::open(root.clone()).unwrap();
        let recovered_at = "2026-07-10T12:00:00Z";
        let report = db.recover_after_unclean_shutdown(recovered_at).unwrap();
        assert_eq!(report.total(), 9);
        assert_eq!(report.engine_runs, 1);
        assert_eq!(report.legacy_tasks, 1);
        assert_eq!(report.task_steps, 1);
        assert_eq!(report.work_tasks, 1);
        assert_eq!(report.scheduled_runs, 1);
        assert_eq!(report.crew_runs, 1);
        assert_eq!(report.worker_sandboxes, 1);
        assert_eq!(report.managed_processes, 1);
        assert_eq!(report.terminal_backends, 1);
        assert_eq!(
            db.recover_after_unclean_shutdown("2026-07-10T12:01:00Z")
                .unwrap()
                .total(),
            0
        );

        let conn = db.conn.lock().unwrap();
        for (table, id, expected) in [
            ("engine_runs", "engine-active", "interrupted"),
            ("tasks", "legacy-active", "failed"),
            ("work_tasks", "work-active", "failed"),
            ("scheduled_runs", "scheduled-active", "interrupted"),
            ("crew_runs", "crew-active", "interrupted"),
            ("worker_sandboxes", "sandbox-active", "interrupted"),
            ("managed_processes", "process-active", "interrupted"),
            ("terminal_backends", "backend-active", "disconnected"),
        ] {
            let status: String = conn
                .query_row(
                    &format!("SELECT status FROM {table} WHERE id = ?1"),
                    params![id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(status, expected);
        }
        let step_state: String = conn
            .query_row(
                "SELECT state FROM task_steps WHERE id = 'step-active'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(step_state, "failed");
        let waiting_status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE id = 'legacy-waiting'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(waiting_status, "waiting_approval");
        let session_ended: Option<String> = conn
            .query_row(
                "SELECT ended_at FROM sessions WHERE id = 'session-open'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(session_ended.is_none());
        let approval_status: String = conn
            .query_row(
                "SELECT status FROM crew_approvals WHERE id = 'approval-pending'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(approval_status, "pending");
        let engine_recovery_events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM engine_run_events
                 WHERE run_id = 'engine-active' AND event_type = 'run_interrupted'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let crew_recovery_events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM crew_run_events
                 WHERE run_id = 'crew-active' AND event_type = 'run_interrupted'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(engine_recovery_events, 1);
        assert_eq!(crew_recovery_events, 1);
        drop(conn);
        drop(db);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scheduled_run_claim_prevents_overlap_and_completion_updates_the_same_row() {
        let db = Database::open_in_memory().unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO scheduled_tasks (
                   id, name, prompt, schedule_expr, active, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5)",
                params![
                    "schedule-claim",
                    "Schedule",
                    "prompt",
                    "hourly",
                    "2026-07-10T10:00:00Z"
                ],
            )
            .unwrap();
        }

        assert!(db
            .begin_scheduled_run(
                "run-claim-1",
                "schedule-claim",
                "2026-07-10T10:00:00Z",
                Some("2026-07-10T11:00:00Z"),
            )
            .unwrap());
        assert!(!db
            .begin_scheduled_run(
                "run-claim-2",
                "schedule-claim",
                "2026-07-10T10:01:00Z",
                Some("2026-07-10T11:00:00Z"),
            )
            .unwrap());
        db.insert_scheduled_run(
            "run-claim-1",
            "schedule-claim",
            "succeeded",
            "2026-07-10T10:00:00Z",
            Some("2026-07-10T10:02:00Z"),
            Some("done"),
            None,
        )
        .unwrap();
        assert!(db
            .begin_scheduled_run(
                "run-claim-2",
                "schedule-claim",
                "2026-07-10T11:00:00Z",
                Some("2026-07-10T12:00:00Z"),
            )
            .unwrap());

        let runs = db.list_scheduled_runs(10).unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs.iter().filter(|run| run.2 == "running").count(), 1);
        assert_eq!(runs.iter().filter(|run| run.2 == "succeeded").count(), 1);
    }

    #[test]
    fn memory_search_ranks_natural_language_terms_instead_of_requiring_exact_phrase() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_memory_entry(
            "memory-api-contract",
            "shared",
            "knowledge",
            "API contract policy",
            "The scheduler API requires idempotent retries and explicit rollback behavior.",
            None,
            0.95,
        )
        .unwrap();
        db.upsert_memory_entry(
            "memory-scheduler",
            "shared",
            "knowledge",
            "Scheduler operations",
            "Nightly runs use bounded retries and preserve the previous successful result.",
            None,
            0.9,
        )
        .unwrap();
        db.upsert_memory_entry(
            "memory-unrelated",
            "shared",
            "knowledge",
            "Brand colors",
            "The interface uses neutral surfaces and green success indicators.",
            None,
            1.0,
        )
        .unwrap();

        let results = db
            .search_memory_entries(
                "latest API contracts and scheduler retry behavior for this crew task",
                None,
                None,
                5,
            )
            .unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "memory-api-contract");
        assert!(results.iter().any(|entry| entry.id == "memory-scheduler"));
        assert!(!results.iter().any(|entry| entry.id == "memory-unrelated"));
    }

    #[test]
    fn memory_search_applies_scope_and_category_before_limiting_results() {
        let db = Database::open_in_memory().unwrap();
        for index in 0..8 {
            db.upsert_memory_entry(
                &format!("agent-{index}"),
                "agent",
                "notes",
                &format!("agent-key-{index}"),
                "shared search term",
                None,
                1.0,
            )
            .unwrap();
        }
        db.upsert_memory_entry(
            "shared-match",
            "shared",
            "knowledge",
            "shared-key",
            "shared search term",
            None,
            1.0,
        )
        .unwrap();

        let results = db
            .search_memory_entries("shared search term", Some("shared"), Some("knowledge"), 1)
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "shared-match");
    }
}
