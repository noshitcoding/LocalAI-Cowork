//! Stable, versioned contracts shared by every Open Cowork runtime.
//!
//! The wire format is intentionally data-oriented. New optional fields and new
//! capability strings can be introduced without forcing every client to update
//! at once. Breaking changes require a new `schema_version`.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

pub const API_VERSION: &str = "v1";
pub const SCHEMA_VERSION: u16 = 2;
pub const MIN_COMPATIBLE_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Capability(pub String);

impl Capability {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl From<&str> for Capability {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

pub mod capabilities {
    use super::Capability;

    pub fn files() -> Capability {
        Capability::from("files")
    }
    pub fn shell() -> Capability {
        Capability::from("shell")
    }
    pub fn git() -> Capability {
        Capability::from("git")
    }
    pub fn mcp() -> Capability {
        Capability::from("mcp")
    }
    pub fn browser_headless() -> Capability {
        Capability::from("browser.headless")
    }
    pub fn browser_visible() -> Capability {
        Capability::from("browser.visible")
    }
    pub fn desktop_linux() -> Capability {
        Capability::from("desktop.linux")
    }
    pub fn desktop_windows() -> Capability {
        Capability::from("desktop.windows")
    }
    pub fn office_ooxml() -> Capability {
        Capability::from("office.ooxml")
    }
    pub fn office_libreoffice() -> Capability {
        Capability::from("office.libreoffice")
    }
    pub fn office_microsoft() -> Capability {
        Capability::from("office.microsoft")
    }
    pub fn model_external() -> Capability {
        Capability::from("model.external")
    }
    pub fn model_ollama() -> Capability {
        Capability::from("model.ollama")
    }
    pub fn model_vllm() -> Capability {
        Capability::from("model.vllm")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExecutorTarget {
    ServerLinux { pool_id: Option<Uuid> },
    ManagedWindowsPool { pool_id: Uuid },
    PersonalDevice { device_id: Uuid },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorKind {
    ServerLinux,
    ManagedWindows,
    PersonalDevice,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonalDeviceRemoteControlMode {
    Off,
    #[default]
    ConfirmEachSession,
    Unattended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Queued,
    WaitingForExecutor,
    WaitingForSnapshot,
    Running,
    WaitingApproval,
    WaitingInput,
    Interrupted,
    Completed,
    Failed,
    Canceled,
    Expired,
}

impl RunState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Canceled | Self::Expired
        )
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        use RunState::*;
        if self == next {
            return true;
        }
        matches!(
            (self, next),
            (
                Queued,
                WaitingForExecutor | WaitingForSnapshot | Running | Canceled | Expired
            ) | (WaitingForExecutor, Queued | Running | Canceled | Expired)
                | (
                    WaitingForSnapshot,
                    Queued | WaitingForExecutor | Canceled | Expired
                )
                | (
                    Running,
                    WaitingApproval | WaitingInput | Interrupted | Completed | Failed | Canceled
                )
                | (
                    WaitingApproval,
                    Running | Interrupted | Failed | Canceled | Expired
                )
                | (
                    WaitingInput,
                    Running | Interrupted | Failed | Canceled | Expired
                )
                | (Interrupted, Queued | Failed | Canceled | Expired)
        )
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectPrivacy {
    PrivateLocal,
    #[default]
    TeamManaged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrozenReference {
    pub id: Uuid,
    pub revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSpec {
    pub schema_version: u16,
    pub id: Uuid,
    pub thread_id: Uuid,
    pub project_id: Uuid,
    pub project: FrozenReference,
    pub project_privacy: ProjectPrivacy,
    pub task: Option<FrozenReference>,
    pub creator_user_id: Uuid,
    pub executor_target: ExecutorTarget,
    #[serde(default)]
    pub required_capabilities: Vec<Capability>,
    pub input: Value,
    pub model_profile_id: Option<Uuid>,
    pub snapshot_id: Option<Uuid>,
    pub idempotency_key: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub spec: RunSpec,
    pub state: RunState,
    pub revision: i64,
    pub etag: String,
    pub assigned_executor_id: Option<Uuid>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub result: Option<Value>,
    pub error: Option<RunError>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default)]
    pub details: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunEvent {
    pub schema_version: u16,
    pub run_id: Uuid,
    pub sequence: i64,
    pub event_id: Uuid,
    pub kind: RunEventKind,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunArtifact {
    pub schema_version: u16,
    pub id: Uuid,
    pub run_id: Uuid,
    pub revision: i64,
    pub kind: String,
    pub media_type: String,
    pub name: String,
    pub digest: String,
    pub size_bytes: u64,
    #[serde(default)]
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunEventKind {
    Created,
    StateChanged,
    ModelStarted,
    ModelDelta,
    ModelCompleted,
    ToolStarted,
    ToolCompleted,
    ToolFailed,
    CheckpointCreated,
    ApprovalRequested,
    ApprovalResolved,
    InputRequested,
    InputReceived,
    ArtifactCreated,
    DesktopSessionChanged,
    ExecutorAssigned,
    ExecutorHeartbeat,
    Warning,
    Failed,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRunRequest {
    pub thread_id: Uuid,
    pub project_id: Uuid,
    pub project_revision: i64,
    #[serde(default)]
    pub project_privacy: ProjectPrivacy,
    pub task: Option<FrozenReference>,
    pub executor_target: ExecutorTarget,
    #[serde(default)]
    pub required_capabilities: Vec<Capability>,
    pub input: Value,
    pub model_profile_id: Option<Uuid>,
    pub snapshot_id: Option<Uuid>,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListRunsResponse {
    pub items: Vec<RunRecord>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityDescriptor {
    pub schema_version: u16,
    pub name: Capability,
    pub version: String,
    #[serde(default)]
    pub attributes: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorRegistration {
    pub schema_version: u16,
    pub executor_id: Uuid,
    pub kind: ExecutorKind,
    pub pool_id: Option<Uuid>,
    pub owner_user_id: Option<Uuid>,
    pub display_name: String,
    pub protocol_version: u16,
    pub capabilities: Vec<CapabilityDescriptor>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub personal_device_remote_control: Option<PersonalDeviceRemoteControlMode>,
    pub max_concurrent_runs: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorRecord {
    pub registration: ExecutorRegistration,
    pub online: bool,
    pub draining: bool,
    pub active_runs: u16,
    pub last_seen_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorHeartbeat {
    pub protocol_version: u16,
    pub active_run_ids: Vec<Uuid>,
    pub health: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateExecutorCredentialRequest {
    pub label: String,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorCredentialSecret {
    pub schema_version: u16,
    pub credential_id: Uuid,
    pub executor_id: Uuid,
    pub token: String,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutorClientMessage {
    Heartbeat {
        heartbeat: ExecutorHeartbeat,
    },
    LeaseHeartbeat {
        run_id: Uuid,
        lease_token: Uuid,
    },
    Event {
        run_id: Uuid,
        request: AppendRunEventRequest,
    },
    Complete {
        run_id: Uuid,
        request: CompleteRunRequest,
    },
    Fail {
        run_id: Uuid,
        request: FailRunRequest,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutorServerMessage {
    Hello {
        schema_version: u16,
        executor_id: Uuid,
        heartbeat_interval_seconds: u64,
    },
    Lease {
        lease: Box<RunLease>,
    },
    DesktopStreamRequested {
        run_id: Uuid,
        session_id: Uuid,
        stream_id: Uuid,
        control: bool,
    },
    Ack {
        operation: String,
        run_id: Option<Uuid>,
    },
    Error {
        code: String,
        message: String,
        run_id: Option<Uuid>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunLease {
    pub schema_version: u16,
    pub run: RunRecord,
    pub lease_token: Uuid,
    pub lease_expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseHeartbeat {
    pub lease_token: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendRunEventRequest {
    pub lease_token: Uuid,
    #[serde(default)]
    pub source_event_id: Option<Uuid>,
    pub kind: RunEventKind,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompleteRunRequest {
    pub lease_token: Uuid,
    pub result: Value,
    #[serde(default)]
    pub result_snapshot_manifest_id: Option<Uuid>,
    #[serde(default)]
    pub result_diff_summary: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailRunRequest {
    pub lease_token: Uuid,
    pub error: RunError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapAdminRequest {
    pub email: String,
    pub display_name: String,
    pub password: String,
    pub device_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasswordLoginRequest {
    pub email: String,
    pub password: String,
    pub device_id: Uuid,
    pub second_factor: Option<String>,
}

/// Starts a native-app authorization-code flow. Native clients keep the
/// verifier locally and send only its S256 challenge with the credentials.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeAuthorizationRequest {
    pub email: String,
    pub password: String,
    pub device_id: Uuid,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub second_factor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TotpSetupResponse {
    pub schema_version: u16,
    pub secret: String,
    pub otpauth_uri: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyTotpRequest {
    pub code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TotpRecoveryCodes {
    pub schema_version: u16,
    pub recovery_codes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TotpStatus {
    pub schema_version: u16,
    pub enabled: bool,
    pub unused_recovery_codes: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisableTotpRequest {
    pub password: String,
    pub second_factor: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasskeyChallenge {
    pub schema_version: u16,
    pub challenge_id: Uuid,
    pub public_key: Value,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinishPasskeyRegistrationRequest {
    pub challenge_id: Uuid,
    pub label: String,
    pub credential: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartPasskeyAuthenticationRequest {
    pub email: String,
    pub device_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinishPasskeyAuthenticationRequest {
    pub challenge_id: Uuid,
    pub credential: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartNativePasskeyAuthenticationRequest {
    pub email: String,
    pub device_id: Uuid,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub state: String,
    pub redirect_uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativePasskeyChallenge {
    pub schema_version: u16,
    pub challenge_id: Uuid,
    pub authorization_id: Uuid,
    pub public_key: Value,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinishNativePasskeyAuthenticationRequest {
    pub challenge_id: Uuid,
    pub authorization_id: Uuid,
    pub credential: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativePasskeyAuthorizationResult {
    pub schema_version: u16,
    pub redirect_url: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasskeyRecord {
    pub schema_version: u16,
    pub id: Uuid,
    pub label: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeletePasskeyRequest {
    pub password: String,
    pub second_factor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeAuthorizationCode {
    pub schema_version: u16,
    pub code: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeTokenRequest {
    pub code: String,
    pub code_verifier: String,
    pub device_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshSessionRequest {
    pub refresh_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateInvitationRequest {
    pub email: String,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvitationSecret {
    pub schema_version: u16,
    pub invitation_id: Uuid,
    pub email: String,
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptInvitationRequest {
    pub token: String,
    pub display_name: String,
    pub password: String,
    pub device_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthTokens {
    pub schema_version: u16,
    pub access_token: String,
    pub access_expires_at: DateTime<Utc>,
    pub refresh_token: String,
    pub refresh_expires_at: DateTime<Utc>,
    pub user_id: Uuid,
    pub session_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcConfiguration {
    pub schema_version: u16,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartOidcAuthorizationRequest {
    pub device_id: Uuid,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub client_state: String,
    pub redirect_uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcAuthorization {
    pub schema_version: u16,
    pub authorization_url: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum PushSubscriptionRegistration {
    Fcm {
        token: String,
    },
    WebPush {
        endpoint: String,
        p256dh: String,
        auth: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterPushSubscriptionRequest {
    pub device_id: Uuid,
    #[serde(flatten)]
    pub subscription: PushSubscriptionRegistration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushSubscriptionRecord {
    pub schema_version: u16,
    pub id: Uuid,
    pub device_id: Uuid,
    pub provider: String,
    pub created_at: DateTime<Utc>,
    pub last_success_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushConfiguration {
    pub schema_version: u16,
    pub fcm_enabled: bool,
    pub web_push_public_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTerminalSessionRequest {
    pub columns: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalSessionTicket {
    pub schema_version: u16,
    pub session_id: Uuid,
    pub token: String,
    pub expires_at: DateTime<Utc>,
    pub protocol: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxImage {
    Core,
    Gui,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxNetwork {
    #[default]
    None,
    FilteredEgress,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxLimits {
    pub memory_bytes: u64,
    pub cpu_nanos: u64,
    pub pids: u32,
    pub timeout_seconds: u64,
    pub tmpfs_bytes: u64,
    pub output_bytes: u64,
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            memory_bytes: 2 * 1024 * 1024 * 1024,
            cpu_nanos: 2_000_000_000,
            pids: 256,
            timeout_seconds: 15 * 60,
            tmpfs_bytes: 512 * 1024 * 1024,
            output_bytes: 4 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxRunSpec {
    pub schema_version: u16,
    pub run_id: Uuid,
    pub image: SandboxImage,
    pub argv: Vec<String>,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub stdin_base64: Option<String>,
    #[serde(default)]
    pub network: SandboxNetwork,
    #[serde(default)]
    pub limits: SandboxLimits,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxRunResult {
    pub schema_version: u16,
    pub run_id: Uuid,
    pub container_name: String,
    pub workspace_volume: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
    pub output_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxDesktopSessionSpec {
    pub schema_version: u16,
    pub session_id: Uuid,
    pub run_id: Uuid,
    pub dimensions: DesktopDimensions,
    #[serde(default)]
    pub network: SandboxNetwork,
    #[serde(default)]
    pub limits: SandboxLimits,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxDesktopSessionResult {
    pub schema_version: u16,
    pub session_id: Uuid,
    pub run_id: Uuid,
    pub container_name: String,
    pub workspace_volume: String,
    pub dimensions: DesktopDimensions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorPool {
    pub schema_version: u16,
    pub id: Uuid,
    pub revision: i64,
    pub etag: String,
    pub name: String,
    pub kind: ExecutorKind,
    pub team_id: Option<Uuid>,
    #[serde(default)]
    pub policy: Value,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamRole {
    Owner,
    Admin,
    Member,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectRole {
    Viewer,
    Runner,
    Editor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamRecord {
    pub schema_version: u16,
    pub id: Uuid,
    pub revision: i64,
    pub etag: String,
    pub name: String,
    pub owner_user_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTeamRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetTeamMemberRequest {
    pub user_id: Uuid,
    pub role: TeamRole,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRecord {
    pub schema_version: u16,
    pub id: Uuid,
    pub revision: i64,
    pub etag: String,
    pub owner_user_id: Uuid,
    pub team_id: Option<Uuid>,
    pub privacy: ProjectPrivacy,
    pub name: String,
    pub description: String,
    pub preferred_executor_target: Option<ExecutorTarget>,
    pub current_version_id: Option<Uuid>,
    pub policy: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateProjectRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub privacy: ProjectPrivacy,
    pub team_id: Option<Uuid>,
    pub preferred_executor_target: Option<ExecutorTarget>,
    #[serde(default)]
    pub policy: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetProjectMemberRequest {
    pub user_id: Uuid,
    pub role: ProjectRole,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateThreadRequest {
    pub project_id: Uuid,
    pub title: String,
    pub forked_from_thread_id: Option<Uuid>,
    pub forked_from_message_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSupportGrantRequest {
    pub support_user_id: Uuid,
    pub project_id: Option<Uuid>,
    pub thread_id: Option<Uuid>,
    pub reason: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupportGrantRecord {
    pub schema_version: u16,
    pub id: Uuid,
    pub granted_by: Uuid,
    pub support_user_id: Uuid,
    pub project_id: Option<Uuid>,
    pub thread_id: Option<Uuid>,
    pub reason: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaScopeType {
    User,
    Team,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetQuotaLimitsRequest {
    pub storage_bytes: Option<u64>,
    pub concurrent_runs: Option<u32>,
    pub monthly_tokens: Option<u64>,
    pub monthly_cost_micros: Option<u64>,
    #[serde(default = "default_true")]
    pub hard_cost_limit: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaLimitsRecord {
    pub schema_version: u16,
    pub scope_type: QuotaScopeType,
    pub scope_id: Uuid,
    pub storage_bytes: Option<u64>,
    pub concurrent_runs: Option<u32>,
    pub monthly_tokens: Option<u64>,
    pub monthly_cost_micros: Option<u64>,
    pub hard_cost_limit: bool,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaUsageRecord {
    pub schema_version: u16,
    pub scope_type: QuotaScopeType,
    pub scope_id: Uuid,
    pub period_start: String,
    pub storage_bytes: u64,
    pub running_runs: u32,
    pub tokens: u64,
    pub cost_micros: u64,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaStatus {
    pub schema_version: u16,
    pub limits: QuotaLimitsRecord,
    pub usage: QuotaUsageRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateExecutorPoolRequest {
    pub name: String,
    pub kind: ExecutorKind,
    pub team_id: Option<Uuid>,
    #[serde(default)]
    pub policy: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrantExecutorPoolRequest {
    pub project_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDefinition {
    pub schema_version: u16,
    pub id: Uuid,
    pub revision: i64,
    pub etag: String,
    pub project_id: Uuid,
    pub name: String,
    pub instructions: String,
    #[serde(default)]
    pub required_capabilities: Vec<Capability>,
    pub default_target: Option<ExecutorTarget>,
    pub config: Value,
    pub released: bool,
    pub created_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTaskDefinitionRequest {
    pub project_id: Uuid,
    pub name: String,
    pub instructions: String,
    #[serde(default)]
    pub required_capabilities: Vec<Capability>,
    pub default_target: Option<ExecutorTarget>,
    #[serde(default)]
    pub config: Value,
    #[serde(default)]
    pub release: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTaskVersionRequest {
    pub base_revision: i64,
    pub name: String,
    pub instructions: String,
    #[serde(default)]
    pub required_capabilities: Vec<Capability>,
    pub default_target: Option<ExecutorTarget>,
    #[serde(default)]
    pub config: Value,
    #[serde(default)]
    pub release: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseTaskVersionRequest {
    pub revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadRecord {
    pub schema_version: u16,
    pub id: Uuid,
    pub revision: i64,
    pub etag: String,
    pub project_id: Uuid,
    pub forked_from_thread_id: Option<Uuid>,
    pub forked_from_message_id: Option<Uuid>,
    pub title: String,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleRecord {
    pub schema_version: u16,
    pub id: Uuid,
    pub revision: i64,
    pub etag: String,
    pub task_id: Uuid,
    pub project_id: Uuid,
    pub thread_id: Uuid,
    pub cron: String,
    pub timezone: String,
    pub executor_target: ExecutorTarget,
    pub input: Value,
    pub model_profile_id: Option<Uuid>,
    pub enabled: bool,
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_triggered_at: Option<DateTime<Utc>>,
    pub blocked_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateScheduleRequest {
    pub task_id: Uuid,
    pub project_id: Uuid,
    pub thread_id: Uuid,
    pub cron: String,
    pub timezone: String,
    pub executor_target: ExecutorTarget,
    #[serde(default)]
    pub input: Value,
    pub model_profile_id: Option<Uuid>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateScheduleRequest {
    pub expected_revision: i64,
    pub cron: String,
    pub timezone: String,
    pub executor_target: ExecutorTarget,
    #[serde(default)]
    pub input: Value,
    pub model_profile_id: Option<Uuid>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectVersion {
    pub schema_version: u16,
    pub id: Uuid,
    pub project_id: Uuid,
    pub revision: i64,
    pub parent_version_id: Option<Uuid>,
    pub merge_base_version_id: Option<Uuid>,
    pub snapshot_manifest_id: Uuid,
    pub created_by_run_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotManifest {
    pub schema_version: u16,
    pub id: Uuid,
    pub project_id: Uuid,
    pub total_bytes: u64,
    pub files: Vec<SnapshotFile>,
    pub encryption_key_id: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotFile {
    pub path: String,
    pub size: u64,
    pub mode: u32,
    pub modified_at: DateTime<Utc>,
    pub chunks: Vec<SnapshotChunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotChunk {
    pub digest: String,
    pub plaintext_size: u64,
    pub ciphertext_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BeginSnapshotUploadRequest {
    pub project_id: Uuid,
    pub total_bytes: u64,
    pub files: Vec<SnapshotUploadFile>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotUploadFile {
    pub path: String,
    pub size: u64,
    pub mode: u32,
    pub modified_at: DateTime<Utc>,
    pub chunks: Vec<SnapshotUploadChunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotUploadChunk {
    pub digest: String,
    pub plaintext_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotUploadSession {
    pub schema_version: u16,
    pub manifest_id: Uuid,
    pub missing_chunks: Vec<String>,
    pub max_chunk_bytes: u64,
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotChunkReceipt {
    pub schema_version: u16,
    pub manifest_id: Uuid,
    pub digest: String,
    pub plaintext_size: u64,
    pub ciphertext_size: u64,
    pub deduplicated: bool,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateProjectVersionRequest {
    pub snapshot_manifest_id: Uuid,
    pub parent_version_id: Option<Uuid>,
    pub merge_base_version_id: Option<Uuid>,
    pub created_by_run_id: Option<Uuid>,
    #[serde(default)]
    pub diff_summary: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyProjectVersionRequest {
    pub expected_project_revision: i64,
    pub expected_current_version_id: Option<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeFileStatus {
    Unchanged,
    Added,
    Deleted,
    CurrentOnly,
    ResultOnly,
    IdenticalChange,
    AutoMerged,
    TextConflict,
    BinaryConflict,
    Renamed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeFileReview {
    pub path: String,
    pub renamed_from: Option<String>,
    pub status: MergeFileStatus,
    pub base_digest: Option<String>,
    pub current_digest: Option<String>,
    pub result_digest: Option<String>,
    pub auto_mergeable: bool,
    pub conflict_preview: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMergeReview {
    pub schema_version: u16,
    pub project_id: Uuid,
    pub base_version_id: Uuid,
    pub current_version_id: Uuid,
    pub result_version_id: Uuid,
    pub files: Vec<MergeFileReview>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeResolutionChoice {
    Current,
    Result,
    Delete,
    AutoMerged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeFileResolution {
    pub path: String,
    pub choice: MergeResolutionChoice,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyProjectMergeRequest {
    pub base_version_id: Uuid,
    pub current_version_id: Uuid,
    pub result_version_id: Uuid,
    pub expected_project_revision: i64,
    #[serde(default)]
    pub resolutions: Vec<MergeFileResolution>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncOperation {
    Upsert,
    Delete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncChange {
    pub schema_version: u16,
    pub operation_id: Uuid,
    pub device_id: Uuid,
    pub entity_type: String,
    pub entity_id: Uuid,
    pub base_revision: i64,
    pub operation: SyncOperation,
    pub payload: Option<Value>,
    pub client_timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    Pending,
    Approved,
    Rejected,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub schema_version: u16,
    pub id: Uuid,
    pub run_id: Uuid,
    pub revision: i64,
    pub etag: String,
    pub requested_action: Value,
    pub state: ApprovalState,
    pub requested_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub resolved_by_user_id: Option<Uuid>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateApprovalRequest {
    pub lease_token: Uuid,
    #[serde(default)]
    pub source_request_id: Option<Uuid>,
    pub requested_action: Value,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approved,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveApprovalRequest {
    pub expected_revision: i64,
    pub decision: ApprovalDecision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputRequestState {
    Pending,
    Submitted,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunInputRequest {
    pub schema_version: u16,
    pub id: Uuid,
    pub run_id: Uuid,
    pub revision: i64,
    pub etag: String,
    pub prompt: Value,
    pub state: InputRequestState,
    pub response: Option<Value>,
    pub requested_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub responded_by_user_id: Option<Uuid>,
    pub responded_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateInputRequest {
    pub lease_token: Uuid,
    #[serde(default)]
    pub source_request_id: Option<Uuid>,
    pub prompt: Value,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitInputResponseRequest {
    pub expected_revision: i64,
    pub response: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunCheckpoint {
    pub schema_version: u16,
    pub id: Uuid,
    pub run_id: Uuid,
    pub sequence: i64,
    pub safe_to_resume: bool,
    pub executor_state: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateCheckpointRequest {
    pub lease_token: Uuid,
    #[serde(default)]
    pub source_checkpoint_id: Option<Uuid>,
    pub safe_to_resume: bool,
    pub executor_state: Value,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopSessionState {
    Starting,
    AgentControlled,
    UserControlled,
    Paused,
    Ended,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopSession {
    pub schema_version: u16,
    pub id: Uuid,
    pub run_id: Uuid,
    pub executor_id: Uuid,
    pub state: DesktopSessionState,
    pub stream_protocol: String,
    pub dimensions: Option<DesktopDimensions>,
    pub controller_user_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopDimensions {
    pub width: u32,
    pub height: u32,
    pub scale_factor: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateDesktopSessionRequest {
    #[serde(default = "default_desktop_width")]
    pub width: u32,
    #[serde(default = "default_desktop_height")]
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReauthenticateRequest {
    pub password: String,
    pub purpose: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReauthenticationGrant {
    pub schema_version: u16,
    pub token: String,
    pub purpose: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopStreamTicketRequest {
    #[serde(default)]
    pub control: bool,
    pub reauthentication_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopStreamTicket {
    pub schema_version: u16,
    pub token: String,
    pub control: bool,
    pub expires_at: DateTime<Utc>,
}

fn default_desktop_width() -> u32 {
    1440
}

fn default_desktop_height() -> u32 {
    900
}

#[derive(Debug, Error)]
pub enum ContractError {
    #[error("unsupported schema version {actual}; supported range is {minimum}..={maximum}")]
    UnsupportedSchemaVersion {
        actual: u16,
        minimum: u16,
        maximum: u16,
    },
    #[error("invalid run state transition from {from:?} to {to:?}")]
    InvalidRunTransition { from: RunState, to: RunState },
}

pub fn ensure_compatible(schema_version: u16) -> Result<(), ContractError> {
    if (MIN_COMPATIBLE_SCHEMA_VERSION..=SCHEMA_VERSION).contains(&schema_version) {
        Ok(())
    } else {
        Err(ContractError::UnsupportedSchemaVersion {
            actual: schema_version,
            minimum: MIN_COMPATIBLE_SCHEMA_VERSION,
            maximum: SCHEMA_VERSION,
        })
    }
}

pub fn ensure_run_transition(from: RunState, to: RunState) -> Result<(), ContractError> {
    if from.can_transition_to(to) {
        Ok(())
    } else {
        Err(ContractError::InvalidRunTransition { from, to })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executor_target_wire_shape_is_stable() {
        let pool_id = Uuid::nil();
        let value = serde_json::to_value(ExecutorTarget::ManagedWindowsPool { pool_id }).unwrap();
        assert_eq!(value["kind"], "managed_windows_pool");
        assert_eq!(value["pool_id"], pool_id.to_string());
    }

    #[test]
    fn terminal_runs_cannot_restart() {
        assert!(RunState::Completed.is_terminal());
        assert!(!RunState::Completed.can_transition_to(RunState::Running));
        assert!(RunState::Interrupted.can_transition_to(RunState::Queued));
    }

    #[test]
    fn capability_names_are_forward_compatible() {
        let capability: Capability = serde_json::from_str("\"future.quantum\"").unwrap();
        assert_eq!(capability.0, "future.quantum");
    }

    #[test]
    fn personal_remote_control_mode_has_a_safe_wire_default() {
        assert_eq!(
            PersonalDeviceRemoteControlMode::default(),
            PersonalDeviceRemoteControlMode::ConfirmEachSession
        );
        assert_eq!(
            serde_json::to_value(PersonalDeviceRemoteControlMode::Off).unwrap(),
            "off"
        );
        assert_eq!(
            serde_json::from_value::<PersonalDeviceRemoteControlMode>(serde_json::json!(
                "unattended"
            ))
            .unwrap(),
            PersonalDeviceRemoteControlMode::Unattended
        );
    }

    #[test]
    fn current_protocol_accepts_exactly_n_minus_one() {
        assert_eq!(SCHEMA_VERSION, 2);
        assert_eq!(MIN_COMPATIBLE_SCHEMA_VERSION, 1);
        assert!(ensure_compatible(1).is_ok());
        assert!(ensure_compatible(2).is_ok());
        assert!(ensure_compatible(0).is_err());
        assert!(ensure_compatible(3).is_err());
    }
}
