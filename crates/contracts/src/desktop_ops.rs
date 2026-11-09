use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Owner host execution only. No actor or model authorization is implied.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OwnerTerminalLaunch {
    pub idempotency_key: String,
    pub program: String,
    pub rows: u16,
    pub cols: u16,
    pub approval_sha256: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct OwnerTerminalPreview {
    pub approval_sha256: String,
    pub executable: String,
    pub cwd: String,
    pub boundary: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct OwnerTerminalSession {
    pub id: String,
    pub order_id: String,
    pub program: String,
    pub state: String,
    pub pid: Option<u32>,
    pub rows: u16,
    pub cols: u16,
    pub created_at: i64,
    pub exit_code: Option<u32>,
    pub input_sequence: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct OwnerTerminalStatus {
    pub active_sessions: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct OwnerTerminalOutput {
    pub session: OwnerTerminalSession,
    pub first_sequence: u64,
    pub next_sequence: u64,
    pub bytes_base64: String,
    pub truncated: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OwnerTerminalInput {
    pub sequence: u64,
    pub bytes_base64: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OwnerTerminalResize {
    pub rows: u16,
    pub cols: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct DatabaseBackup {
    pub id: String,
    pub created_at: i64,
    pub bytes: u64,
    pub sha256: String,
    pub format: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OwnerDatabaseBackupRequest {
    pub idempotency_key: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct MaintenanceStatus {
    pub preferences: crate::PreferencesRecord,
    pub backend: String,
    pub backup_available: bool,
    pub backups: Vec<DatabaseBackup>,
    pub last_error: Option<String>,
    pub last_cleanup_at: Option<i64>,
    pub cleaned_chats: u64,
}

/// Explicit backup-only ceiling; does not relax ordinary file/upload limits.
pub const MAX_DATABASE_BACKUP_BYTES: u64 = 1024 * 1024 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeSaveDatabaseBackup {
    pub id: String,
    pub expected_bytes: u64,
    pub expected_sha256: String,
    pub language: crate::UiLanguage,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum DatabaseBackupSaveOutcome {
    Canceled,
    Saved { bytes: u64, sha256: String },
}
