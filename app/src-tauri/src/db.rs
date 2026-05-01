use rusqlite::{Connection, OptionalExtension, Result as SqlResult, params, params_from_iter};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;
use serde::{Deserialize, Serialize};

pub struct Database {
    conn: Mutex<Connection>,
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
    pub system_prompt: String,
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
    pub status: String,
    pub phase: String,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
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
        std::fs::create_dir_all(&app_data_dir).ok();
        let db_path = app_data_dir.join("open_cowork.db");
        let conn = Connection::open(db_path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        let db = Self { conn: Mutex::new(conn) };
        db.migrate()?;
        Ok(db)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> SqlResult<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        let db = Self { conn: Mutex::new(conn) };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER NOT NULL
            );

            INSERT INTO schema_version (version)
            SELECT 0 WHERE NOT EXISTS (SELECT 1 FROM schema_version);"
        )?;

        let version: i64 = conn.query_row(
            "SELECT version FROM schema_version LIMIT 1",
            [],
            |row| row.get(0),
        )?;

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

                UPDATE schema_version SET version = 2;"
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

                UPDATE schema_version SET version = 3;"
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

                UPDATE schema_version SET version = 4;"
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
                    system_prompt TEXT NOT NULL,
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

                UPDATE schema_version SET version = 6;"
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

                UPDATE schema_version SET version = 7;"
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

                UPDATE schema_version SET version = 8;"
            )?;
        }

        if version < 9 {
            conn.execute_batch(
                "ALTER TABLE scheduled_tasks ADD COLUMN task_kind TEXT NOT NULL DEFAULT 'prompt';
                ALTER TABLE scheduled_tasks ADD COLUMN crew_id TEXT;
                ALTER TABLE scheduled_tasks ADD COLUMN crew_snapshot_json TEXT;

                UPDATE schema_version SET version = 9;"
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

                UPDATE schema_version SET version = 11;"
            )?;
        }

        if version < 12 {
            conn.execute_batch(
                "ALTER TABLE crew_runs ADD COLUMN crew_snapshot_json TEXT NOT NULL DEFAULT '{}';

                UPDATE schema_version SET version = 12;"
            )?;
        }

        if version < 13 {
            conn.execute_batch(
                "ALTER TABLE scheduled_tasks ADD COLUMN model_config_json TEXT;

                UPDATE schema_version SET version = 13;"
            )?;
        }

        if version < 14 {
            conn.execute_batch(
                "ALTER TABLE chat_threads ADD COLUMN provider_settings_json TEXT;

                UPDATE schema_version SET version = 14;"
            )?;
        }

        if version < 15 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS policy_tool_states (
                    tool_id TEXT PRIMARY KEY,
                    enabled INTEGER NOT NULL,
                    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
                );

                UPDATE schema_version SET version = 15;"
            )?;
        }

        if version < 16 {
            conn.execute_batch(
                "ALTER TABLE chat_threads ADD COLUMN permission_config_json TEXT;\n
                UPDATE schema_version SET version = 16;"
            )?;
        }

        Ok(())
    }

    // -- Chat Threads --

    pub fn insert_thread(&self, id: &str, title: &str, created_at: &str, provider_settings_json: Option<&str>, permission_config_json: Option<&str>) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO chat_threads (id, title, created_at, updated_at, provider_settings_json, permission_config_json) VALUES (?1, ?2, ?3, ?3, ?4, ?5)",
            params![id, title, created_at, provider_settings_json, permission_config_json],
        )?;
        Ok(())
    }

    pub fn list_threads(&self) -> SqlResult<Vec<(String, String, String, String, Option<String>, Option<String>)>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, title, created_at, updated_at, provider_settings_json, permission_config_json FROM chat_threads ORDER BY updated_at DESC"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
        })?;
        rows.collect()
    }

    pub fn update_thread_provider_settings(&self, id: &str, provider_settings_json: Option<&str>) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE chat_threads SET provider_settings_json = ?2, updated_at = datetime('now') WHERE id = ?1",
            params![id, provider_settings_json],
        )?;
        Ok(())
    }

    pub fn update_thread_permission_config(&self, id: &str, permission_config_json: Option<&str>) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE chat_threads SET permission_config_json = ?2, updated_at = datetime('now') WHERE id = ?1",
            params![id, permission_config_json],
        )?;
        Ok(())
    }

    pub fn delete_thread(&self, id: &str) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute("DELETE FROM chat_threads WHERE id = ?1", params![id])?;
        Ok(())
    }

    // -- Chat Messages --

    pub fn insert_message(
        &self, id: &str, thread_id: &str, role: &str, content: &str, timestamp: i64,
    ) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO crew_runs (id, crew_id, crew_name, process, status, manager_agent_id, error, crew_snapshot_json, started_at, finished_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, datetime('now'))",
            params![id, crew_id, crew_name, process, status, manager_agent_id, error, crew_snapshot_json, started_at, finished_at],
        )?;
        Ok(())
    }

    pub fn insert_crew_run_logs(&self, run_id: &str, logs: &[crate::CrewExecutionLogRow]) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "INSERT INTO crew_run_logs (id, run_id, crew_id, agent_id, task_id, action, result, timestamp, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, datetime('now'))"
        )?;

        for log in logs {
            stmt.execute(params![
                log.id,
                run_id,
                log.crew_id,
                log.agent_id,
                log.task_id,
                log.action,
                log.result,
                log.timestamp,
            ])?;
        }

        Ok(())
    }

    pub fn list_crew_runs(&self, crew_id: Option<&str>, limit: i64) -> SqlResult<Vec<(String, String, String, String, String, Option<String>, Option<String>, String, Option<String>)>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, crew_id, agent_id, task_id, action, result, timestamp
             FROM crew_run_logs
             WHERE run_id = ?1
             ORDER BY timestamp ASC"
        )?;

        let rows = stmt.query_map(params![run_id], |row| {
            Ok(crate::CrewExecutionLogRow {
                id: row.get(0)?,
                crew_id: row.get(1)?,
                agent_id: row.get(2)?,
                task_id: row.get(3)?,
                action: row.get(4)?,
                result: row.get(5)?,
                timestamp: row.get(6)?,
            })
        })?;

        rows.collect()
    }

    pub fn get_crew_run_snapshot(&self, run_id: &str) -> SqlResult<Option<String>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.query_row(
            "SELECT crew_snapshot_json FROM crew_runs WHERE id = ?1 LIMIT 1",
            params![run_id],
            |row| row.get(0),
        ).optional()
    }

    pub fn update_message_content(&self, id: &str, content: &str) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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

        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let placeholders = std::iter::repeat("?")
            .take(ids.len())
            .collect::<Vec<_>>()
            .join(", ");

        let thread_query = format!(
            "SELECT DISTINCT thread_id FROM chat_messages WHERE id IN ({})",
            placeholders
        );
        let mut stmt = conn.prepare(&thread_query)?;
        let thread_rows = stmt.query_map(params_from_iter(ids.iter()), |row| row.get::<_, String>(0))?;
        let thread_ids: Vec<String> = thread_rows.collect::<SqlResult<Vec<_>>>()?;
        drop(stmt);

        let delete_query = format!(
            "DELETE FROM chat_messages WHERE id IN ({})",
            placeholders
        );
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
        &self, id: &str, title: &str, prompt: &str, status: &str, thread_id: Option<&str>, created_at: &str,
    ) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO tasks (id, title, prompt, status, thread_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![id, title, prompt, status, thread_id, created_at],
        )?;
        Ok(())
    }

    pub fn update_task_status(&self, id: &str, status: &str) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE tasks SET status = ?2, updated_at = datetime('now') WHERE id = ?1",
            params![id, status],
        )?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn set_task_error(&self, id: &str, error: &str) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE tasks SET error = ?2, status = 'failed', updated_at = datetime('now') WHERE id = ?1",
            params![id, error],
        )?;
        Ok(())
    }

    pub fn list_tasks(&self) -> SqlResult<Vec<(String, String, String, String, Option<String>, String, String, Option<String>)>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, title, prompt, status, thread_id, created_at, updated_at, error FROM tasks ORDER BY created_at DESC"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?))
        })?;
        rows.collect()
    }

    // -- Task Steps --

    pub fn insert_step(
        &self, id: &str, task_id: &str, idx: i32, title: &str, state: &str,
        requires_approval: bool, risk_level: &str,
    ) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO task_steps (id, task_id, idx, title, state, requires_approval, risk_level) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, task_id, idx, title, state, requires_approval as i32, risk_level],
        )?;
        Ok(())
    }

    pub fn update_step_state(&self, id: &str, state: &str, output: Option<&str>) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE task_steps SET state = ?2, output = ?3 WHERE id = ?1",
            params![id, state, output],
        )?;
        Ok(())
    }

    pub fn list_steps(&self, task_id: &str) -> SqlResult<Vec<(String, i32, String, String, bool, String, Option<String>)>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, idx, title, state, requires_approval, risk_level, output FROM task_steps WHERE task_id = ?1 ORDER BY idx"
        )?;
        let rows = stmt.query_map(params![task_id], |row| {
            let approval: i32 = row.get(4)?;
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, approval != 0, row.get(5)?, row.get(6)?))
        })?;
        rows.collect()
    }

    // -- Audit --

    #[allow(dead_code)]
    pub fn insert_audit_event(
        &self, id: &str, ts: &str, event_type: &str,
        resource_type: Option<&str>, resource_id: Option<&str>, details_json: Option<&str>,
    ) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO audit_events (id, ts, event_type, resource_type, resource_id, details_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, ts, event_type, resource_type, resource_id, details_json],
        )?;
        Ok(())
    }

    // -- File Safety --

    pub fn add_allowed_folder(&self, path: &str) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT OR REPLACE INTO allowed_folders (path, created_at) VALUES (?1, datetime('now'))",
            params![path],
        )?;
        Ok(())
    }

    pub fn remove_allowed_folder(&self, path: &str) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute("DELETE FROM allowed_folders WHERE path = ?1", params![path])?;
        Ok(())
    }

    pub fn list_allowed_folders(&self) -> SqlResult<Vec<String>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare("SELECT path FROM allowed_folders ORDER BY created_at DESC")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect()
    }

    // -- Runtime Policy --

    pub fn set_policy_flag(&self, key: &str, value: bool) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare("SELECT key, value FROM policy_flags")?;
        let rows = stmt.query_map([], |row| {
            let value: i32 = row.get(1)?;
            Ok((row.get(0)?, value != 0))
        })?;
        rows.collect()
    }

    pub fn replace_policy_deny_rules(&self, rules: &[String]) -> SqlResult<()> {
        let mut conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare("SELECT rule FROM policy_deny_rules ORDER BY created_at DESC")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect()
    }

    pub fn replace_policy_tool_states(&self, states: &[(String, bool)]) -> SqlResult<()> {
        let mut conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare("SELECT tool_id, enabled FROM policy_tool_states ORDER BY tool_id ASC")?;
        let rows = stmt.query_map([], |row| {
            let enabled: i32 = row.get(1)?;
            Ok((row.get(0)?, enabled != 0))
        })?;
        rows.collect()
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
    ) -> SqlResult<Vec<(String, Option<String>, Option<String>, String, String, i64, String, String, String, String)>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
             LIMIT ?1"
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
    ) -> SqlResult<Option<(String, Option<String>, Option<String>, String, String, i64, String, String, String, String)>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
             LIMIT 1"
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO artifact_exports (
                id,
                artifact_version_id,
                export_format,
                target_path,
                size_bytes,
                created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, artifact_version_id, export_format, target_path, size_bytes, created_at],
        )?;
        Ok(())
    }

    pub fn list_artifact_exports(
        &self,
        limit: i64,
    ) -> SqlResult<Vec<(String, String, String, String, i64, String, String, Option<String>, Option<String>, String)>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
             LIMIT ?1"
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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

    pub fn list_scheduled_tasks(&self) -> SqlResult<Vec<(String, String, String, String, String, Option<String>, Option<String>, Option<String>, i64, String, bool, Option<String>, Option<String>, String, String)>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, name, prompt, schedule_expr, next_run_at, task_kind, crew_id, crew_snapshot_json, model_config_json, priority, depends_on_task_ids_json, last_run_at
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute("DELETE FROM scheduled_tasks WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn set_scheduled_task_active(&self, id: &str, active: bool, next_run_at: Option<&str>) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE scheduled_tasks
             SET active = ?2, next_run_at = ?3, updated_at = datetime('now')
             WHERE id = ?1",
            params![id, active as i32, next_run_at],
        )?;
        Ok(())
    }

    pub fn list_due_scheduled_tasks(&self, now: &str) -> SqlResult<Vec<(String, String, String, String, Option<String>, String, Option<String>, Option<String>, Option<String>, i64, String, Option<String>)>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, name, prompt, schedule_expr, next_run_at, task_kind, crew_id, crew_snapshot_json, model_config_json, priority, depends_on_task_ids_json, last_run_at
             FROM scheduled_tasks
             WHERE active = 1 AND next_run_at IS NOT NULL AND next_run_at <= ?1
             ORDER BY priority DESC, next_run_at ASC"
        )?;

        let rows = stmt.query_map(params![now], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?, row.get(9)?, row.get(10)?, row.get(11)?))
        })?;

        rows.collect()
    }

    pub fn latest_scheduled_run_status(&self, task_id: &str) -> SqlResult<Option<(String, Option<String>)>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.query_row(
            "SELECT status, finished_at
             FROM scheduled_runs
             WHERE task_id = ?1
             ORDER BY started_at DESC
             LIMIT 1",
            params![task_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()
    }

    pub fn update_scheduled_task_runtime(&self, id: &str, last_run_at: Option<&str>, next_run_at: Option<&str>) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE scheduled_tasks
             SET last_run_at = ?2, next_run_at = ?3, updated_at = datetime('now')
             WHERE id = ?1",
            params![id, last_run_at, next_run_at],
        )?;
        Ok(())
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO scheduled_runs (id, task_id, status, started_at, finished_at, result, error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, task_id, status, started_at, finished_at, result, error],
        )?;
        Ok(())
    }

    pub fn list_scheduled_runs(&self, limit: i64) -> SqlResult<Vec<(String, String, String, String, Option<String>, Option<String>, Option<String>)>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, task_id, status, started_at, finished_at, result, error
             FROM scheduled_runs
             ORDER BY started_at DESC
             LIMIT ?1"
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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

    pub fn get_memory_entry(&self, scope: &str, category: &str, key: &str) -> SqlResult<Option<MemoryEntryRow>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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

    pub fn list_memory_entries(&self, scope: &str, category: Option<&str>, limit: i64) -> SqlResult<Vec<MemoryEntryRow>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let (sql, params_vec): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(cat) = category {
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
        let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();
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

    pub fn search_memory_entries(&self, query: &str, limit: i64) -> SqlResult<Vec<MemoryEntryRow>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let pattern = format!("%{}%", query);
        let mut stmt = conn.prepare(
            "SELECT id, scope, category, key, content, source_session_id, confidence, access_count, last_accessed_at, created_at, updated_at
             FROM memory_entries WHERE content LIKE ?1 OR key LIKE ?1 ORDER BY updated_at DESC LIMIT ?2"
        )?;
        let rows = stmt.query_map(params![pattern, limit], |row| {
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

    pub fn delete_memory_entry(&self, id: &str) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute("DELETE FROM memory_entries WHERE id = ?1", params![id])?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn touch_memory_entry(&self, id: &str) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE memory_entries SET access_count = access_count + 1, last_accessed_at = datetime('now') WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn compact_memory(&self, scope: &str, min_confidence: f64, max_age_days: i64) -> SqlResult<usize> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let deleted = conn.execute(
            "DELETE FROM memory_entries WHERE scope = ?1 AND confidence < ?2 AND updated_at < datetime('now', ?3 || ' days')",
            params![scope, min_confidence, -max_age_days],
        )?;
        Ok(deleted)
    }

    // -- User Profile --

    pub fn upsert_user_profile(&self, id: &str, key: &str, value: &str, source: &str, confidence: f64) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, name, description, prompt_template, trigger_pattern, run_mode, version,
                    usage_count, success_count, fail_count, avg_quality, auto_generated,
                    parent_skill_id, source_task_ids, created_at, updated_at
             FROM skills ORDER BY usage_count DESC, updated_at DESC LIMIT ?1"
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, name, description, prompt_template, trigger_pattern, run_mode, version,
                    usage_count, success_count, fail_count, avg_quality, auto_generated,
                    parent_skill_id, source_task_ids, created_at, updated_at
             FROM skills WHERE name = ?1 LIMIT 1"
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let old_version: i32 = conn.query_row(
            "SELECT version FROM skills WHERE id = ?1", params![skill_id], |row| row.get(0)
        )?;
        let new_version = old_version + 1;
        let old_prompt: String = conn.query_row(
            "SELECT prompt_template FROM skills WHERE id = ?1", params![skill_id], |row| row.get(0)
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO sessions (id, thread_id, title, memory_snapshot_json, model_used, provider, personality, started_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))",
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE sessions SET summary = ?2, total_messages = ?3, total_tokens_est = ?4,
                    outcome = ?5, task_ids = ?6, skill_ids_used = ?7, ended_at = datetime('now')
             WHERE id = ?1",
            params![id, summary, total_messages, total_tokens_est, outcome, task_ids, skill_ids_used],
        )?;
        Ok(())
    }

    pub fn list_sessions(&self, limit: i64) -> SqlResult<Vec<SessionRow>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, thread_id, title, summary, model_used, provider, personality,
                    total_messages, total_tokens_est, outcome, started_at, ended_at
             FROM sessions ORDER BY started_at DESC LIMIT ?1"
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute("DELETE FROM sessions WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn get_session(&self, id: &str) -> SqlResult<Option<SessionRow>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, thread_id, title, summary, model_used, provider, personality,
                    total_messages, total_tokens_est, outcome, started_at, ended_at
             FROM sessions WHERE id = ?1 LIMIT 1"
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT memory_snapshot_json FROM sessions WHERE id = ?1 LIMIT 1"
        )?;
        let mut rows = stmt.query(params![session_id])?;
        if let Some(row) = rows.next()? {
            Ok(row.get(0)?)
        } else {
            Ok(None)
        }
    }

    pub fn save_session_snapshot(&self, session_id: &str, snapshot_json: &str) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE sessions SET memory_snapshot_json = ?2 WHERE id = ?1",
            params![session_id, snapshot_json],
        )?;
        Ok(())
    }

    // -- Engine Runs --

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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO engine_runs (
                id, parent_run_id, thread_id, session_id, title, input_summary, status, phase,
                cwd, model, provider, retry_count, resumed_from_run_id, checkpoint_json,
                metadata_json, created_at, updated_at, started_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                ?9, ?10, ?11, ?12, ?13, ?14,
                ?15, datetime('now'), datetime('now'),
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
                retry_count,
                resumed_from_run_id,
                checkpoint_json,
                metadata_json
            ],
        )?;
        Ok(())
    }

    pub fn get_engine_run(&self, id: &str) -> SqlResult<Option<EngineRunRow>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, parent_run_id, thread_id, session_id, title, input_summary, status, phase,
                    cwd, model, provider, retry_count, resumed_from_run_id, checkpoint_json,
                    result_summary, error, metadata_json, created_at, updated_at, started_at, ended_at, canceled_at
             FROM engine_runs WHERE id = ?1 LIMIT 1"
        )?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(EngineRunRow {
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
            }))
        } else {
            Ok(None)
        }
    }

    pub fn list_engine_runs(&self, limit: i64, status: Option<&str>) -> SqlResult<Vec<EngineRunRow>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let rows = if let Some(status_filter) = status {
            let mut stmt = conn.prepare(
                "SELECT id, parent_run_id, thread_id, session_id, title, input_summary, status, phase,
                        cwd, model, provider, retry_count, resumed_from_run_id, checkpoint_json,
                        result_summary, error, metadata_json, created_at, updated_at, started_at, ended_at, canceled_at
                 FROM engine_runs
                 WHERE status = ?1
                 ORDER BY updated_at DESC
                 LIMIT ?2"
            )?;
            let mapped = stmt.query_map(params![status_filter, limit], |row| {
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
                })
            })?;
            mapped.collect::<SqlResult<Vec<_>>>()?
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, parent_run_id, thread_id, session_id, title, input_summary, status, phase,
                        cwd, model, provider, retry_count, resumed_from_run_id, checkpoint_json,
                        result_summary, error, metadata_json, created_at, updated_at, started_at, ended_at, canceled_at
                 FROM engine_runs
                 ORDER BY updated_at DESC
                 LIMIT ?1"
            )?;
            let mapped = stmt.query_map(params![limit], |row| {
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
                })
            })?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;

        let next_status = status.unwrap_or(existing.status.as_str()).to_string();
        let next_phase = phase.unwrap_or(existing.phase.as_str()).to_string();
        let next_checkpoint = checkpoint_json.or(existing.checkpoint_json.as_deref());
        let next_result = result_summary.or(existing.result_summary.as_deref());
        let next_error = error.or(existing.error.as_deref());
        let next_metadata = metadata_json.or(existing.metadata_json.as_deref());
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO engine_run_events (id, run_id, event_type, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))",
            params![id, run_id, event_type, payload_json],
        )?;
        Ok(())
    }

    pub fn insert_engine_run_checkpoint(
        &self,
        id: &str,
        run_id: &str,
        label: &str,
        snapshot_json: &str,
    ) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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

    pub fn list_engine_run_checkpoints(&self, run_id: &str, limit: i64) -> SqlResult<Vec<EngineRunCheckpointRow>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, run_id, label, snapshot_json, created_at
             FROM engine_run_checkpoints
             WHERE run_id = ?1
             ORDER BY created_at DESC
             LIMIT ?2"
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
            params![id, scope_type, scope_ref, title, content, enabled as i32, priority],
        )?;
        Ok(())
    }

    pub fn delete_runtime_instruction(&self, id: &str) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute("DELETE FROM runtime_instructions WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn list_runtime_instructions(
        &self,
        scope_type: Option<&str>,
        enabled_only: bool,
    ) -> SqlResult<Vec<RuntimeInstructionRow>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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

    pub fn list_worker_sandboxes(&self, limit: i64, status: Option<&str>) -> SqlResult<Vec<WorkerSandboxRow>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
                let mapped = stmt.query_map(params![filter_status, limit], map_worker_sandbox_row)?;
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
        let ended_at = if ["completed", "failed", "canceled", "destroyed"].contains(&next_status) {
            Some(chrono::Utc::now().to_rfc3339())
        } else {
            None
        };

        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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

    #[allow(dead_code)]
    pub fn delete_worker_sandbox(&self, id: &str) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO learning_outcomes (id, session_id, task_id, outcome_type, description, learned_pattern, confidence, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))",
            params![id, session_id, task_id, outcome_type, description, learned_pattern, confidence],
        )?;
        Ok(())
    }

    pub fn list_learning_outcomes(&self, limit: i64) -> SqlResult<Vec<LearningOutcomeRow>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO managed_processes (id, label, command, backend_id, requires_admin, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
            params![id, label, command, backend_id, requires_admin as i32],
        )?;
        Ok(())
    }

    pub fn update_process_status(&self, id: &str, status: &str, pid: Option<i64>, exit_code: Option<i32>) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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

    pub fn approve_process_admin(&self, id: &str) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "UPDATE managed_processes SET admin_approved = 1 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    pub fn list_managed_processes(&self) -> SqlResult<Vec<ManagedProcessRow>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute("DELETE FROM managed_processes WHERE id = ?1", params![id])?;
        Ok(())
    }

    // -- Agent Personalities --

    pub fn upsert_personality(
        &self,
        id: &str,
        name: &str,
        description: &str,
        system_prompt: &str,
        temperature: Option<f64>,
        model_override: Option<&str>,
        icon: Option<&str>,
        is_default: bool,
    ) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        if is_default {
            conn.execute("UPDATE agent_personalities SET is_default = 0", [])?;
        }
        conn.execute(
            "INSERT INTO agent_personalities (id, name, description, system_prompt, temperature, model_override, icon, is_default, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, datetime('now'), datetime('now'))
             ON CONFLICT(name) DO UPDATE SET
                description = excluded.description,
                system_prompt = excluded.system_prompt,
                temperature = excluded.temperature,
                model_override = excluded.model_override,
                icon = excluded.icon,
                is_default = excluded.is_default,
                updated_at = datetime('now')",
            params![id, name, description, system_prompt, temperature, model_override, icon, is_default as i32],
        )?;
        Ok(())
    }

    pub fn list_personalities(&self) -> SqlResult<Vec<PersonalityRow>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, name, description, system_prompt, temperature, model_override, icon, is_default, created_at, updated_at
             FROM agent_personalities ORDER BY name"
        )?;
        let rows = stmt.query_map([], |row| {
            let is_default: i32 = row.get(7)?;
            Ok(PersonalityRow {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                system_prompt: row.get(3)?,
                temperature: row.get(4)?,
                model_override: row.get(5)?,
                icon: row.get(6)?,
                is_default: is_default != 0,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            })
        })?;
        rows.collect()
    }

    pub fn delete_personality(&self, id: &str) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute(
            "INSERT INTO insights_events (id, event_type, category, value_num, value_text, session_id, metadata_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))",
            params![id, event_type, category, value_num, value_text, session_id, metadata_json],
        )?;
        Ok(())
    }

    pub fn query_insights(&self, category: Option<&str>, event_type: Option<&str>, limit: i64) -> SqlResult<Vec<InsightsEventRow>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let sql = match (category, event_type) {
            (Some(_), Some(_)) => "SELECT id, event_type, category, value_num, value_text, session_id, metadata_json, created_at FROM insights_events WHERE category = ?1 AND event_type = ?2 ORDER BY created_at DESC LIMIT ?3",
            (Some(_), None) => "SELECT id, event_type, category, value_num, value_text, session_id, metadata_json, created_at FROM insights_events WHERE category = ?1 ORDER BY created_at DESC LIMIT ?2",
            (None, Some(_)) => "SELECT id, event_type, category, value_num, value_text, session_id, metadata_json, created_at FROM insights_events WHERE event_type = ?1 ORDER BY created_at DESC LIMIT ?2",
            (None, None) => "SELECT id, event_type, category, value_num, value_text, session_id, metadata_json, created_at FROM insights_events ORDER BY created_at DESC LIMIT ?1",
        };

        let mut stmt = conn.prepare(sql)?;
        let rows: Vec<InsightsEventRow> = match (category, event_type) {
            (Some(cat), Some(et)) =>
                stmt.query_map(params![cat, et, limit], map_insights_row)?.collect::<SqlResult<Vec<_>>>()?,
            (Some(cat), None) =>
                stmt.query_map(params![cat, limit], map_insights_row)?.collect::<SqlResult<Vec<_>>>()?,
            (None, Some(et)) =>
                stmt.query_map(params![et, limit], map_insights_row)?.collect::<SqlResult<Vec<_>>>()?,
            (None, None) =>
                stmt.query_map(params![limit], map_insights_row)?.collect::<SqlResult<Vec<_>>>()?,
        };
        Ok(rows)
    }

    #[allow(dead_code)]
    pub fn get_insights_summary(&self, days: i64) -> SqlResult<Vec<(String, String, i64, f64)>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT category, event_type, COUNT(*) as cnt, COALESCE(AVG(value_num), 0) as avg_val
             FROM insights_events WHERE created_at >= datetime('now', ?1 || ' days')
             GROUP BY category, event_type ORDER BY cnt DESC"
        )?;
        let rows = stmt.query_map(params![-days], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
        rows.collect()
    }

    // -- RPC Pipelines --

    pub fn upsert_rpc_pipeline(&self, id: &str, name: &str, description: Option<&str>, steps_json: &str, zero_context: bool) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, name, description, steps_json, zero_context, created_at, updated_at
             FROM rpc_pipelines ORDER BY name"
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute("DELETE FROM rpc_pipelines WHERE id = ?1", params![id])?;
        Ok(())
    }

    // -- Memory Providers --

    pub fn upsert_memory_provider(&self, id: &str, name: &str, provider_type: &str, config_json: &str, enabled: bool) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute("DELETE FROM memory_providers WHERE id = ?1", params![id])?;
        Ok(())
    }

    // -- Tool Gateway --

    pub fn upsert_tool_gateway_entry(&self, id: &str, tool_type: &str, name: &str, config_json: &str, enabled: bool) -> SqlResult<()> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut stmt = conn.prepare(
            "SELECT id, tool_type, name, config_json, enabled, created_at, updated_at
             FROM tool_gateway_entries ORDER BY name"
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
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.execute("DELETE FROM tool_gateway_entries WHERE id = ?1", params![id])?;
        Ok(())
    }

    // -- Session Full-Text Search --

    pub fn fulltext_search_sessions(&self, query: &str, limit: i64) -> SqlResult<Vec<SessionSearchResultRow>> {
        let conn = self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        let pattern = format!("%{}%", query);
        let mut stmt = conn.prepare(
            "SELECT s.id, s.title, s.summary, s.started_at, s.ended_at,
                    m.id, m.content, m.role, m.timestamp
             FROM sessions s
             LEFT JOIN chat_threads t ON t.id = s.thread_id
             LEFT JOIN chat_messages m ON m.thread_id = t.id AND m.content LIKE ?1
             WHERE s.title LIKE ?1 OR s.summary LIKE ?1 OR m.content LIKE ?1
             ORDER BY s.started_at DESC LIMIT ?2"
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

    #[test]
    fn migration_creates_tables() {
        let db = Database::open_in_memory().unwrap();
        db.insert_thread("t1", "Test Thread", "2025-01-01T00:00:00", Some("{\"provider\":\"ollama\"}")).unwrap();
        let threads = db.list_threads().unwrap();
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].0, "t1");
        assert_eq!(threads[0].4.as_deref(), Some("{\"provider\":\"ollama\"}"));
    }

    #[test]
    fn messages_round_trip() {
        let db = Database::open_in_memory().unwrap();
        db.insert_thread("t1", "Thread", "2025-01-01T00:00:00", None).unwrap();
        db.insert_message("m1", "t1", "user", "Hello", 1000).unwrap();
        db.insert_message("m2", "t1", "assistant", "Hi", 1001).unwrap();
        let msgs = db.list_messages("t1").unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].1, "user");
        assert_eq!(msgs[1].1, "assistant");
    }

    #[test]
    fn task_lifecycle() {
        let db = Database::open_in_memory().unwrap();
        db.insert_task("task1", "Test", "Do stuff", "created", None, "2025-01-01T00:00:00").unwrap();
        db.update_task_status("task1", "planned").unwrap();
        let tasks = db.list_tasks().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].3, "planned");
    }

    #[test]
    fn steps_with_task() {
        let db = Database::open_in_memory().unwrap();
        db.insert_task("task1", "Test", "Do stuff", "created", None, "2025-01-01T00:00:00").unwrap();
        db.insert_step("s1", "task1", 0, "Step 1", "pending", false, "low").unwrap();
        db.insert_step("s2", "task1", 1, "Step 2", "pending", true, "medium").unwrap();
        let steps = db.list_steps("task1").unwrap();
        assert_eq!(steps.len(), 2);
        assert!(steps[1].4); // requires_approval
    }

    #[test]
    fn delete_thread_cascades() {
        let db = Database::open_in_memory().unwrap();
        db.insert_thread("t1", "Thread", "2025-01-01T00:00:00", None).unwrap();
        db.insert_message("m1", "t1", "user", "Hello", 1000).unwrap();
        db.delete_thread("t1").unwrap();
        let msgs = db.list_messages("t1").unwrap();
        assert_eq!(msgs.len(), 0);
    }

    #[test]
    fn audit_event_insert() {
        let db = Database::open_in_memory().unwrap();
        db.insert_audit_event("a1", "2025-01-01T00:00:00", "task_created", Some("task"), Some("task1"), None).unwrap();
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
}
