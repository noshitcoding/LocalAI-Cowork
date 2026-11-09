use super::*;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SandboxLifecycleOperation {
    Ensure,
    Inspect,
    Suspend,
    Resume,
    Stop,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SandboxLifecycleState {
    Provisioning,
    Ready,
    Suspended,
    Stopping,
    Stopped,
    Unknown,
}

/// Only an existing project environment can be selected. No host paths, namespaces,
/// manifests or execution rights are accepted from the caller.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManageSandboxSession {
    pub environment: ExecutionEnvironmentRef,
    pub idempotency_key: String,
    pub operation: SandboxLifecycleOperation,
    pub expected_sandbox_uid: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SandboxSession {
    pub project_id: String,
    pub order_id: String,
    pub environment: ExecutionEnvironmentRef,
    pub operation_id: String,
    pub sandbox_name: String,
    pub sandbox_uid: Option<String>,
    pub pod_uid: Option<String>,
    pub state: SandboxLifecycleState,
    pub workspace_preserved: bool,
    pub observed_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CreateSandboxPreview {
    pub environment: ExecutionEnvironmentRef,
    pub expected_sandbox_uid: String,
    pub port: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SandboxPreview {
    pub url: String,
    pub expires_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SandboxSessionOperation {
    pub id: String,
    pub request: ManageSandboxSession,
    pub state: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionEnvironmentRef {
    pub environment_id: String,
    pub version_id: String,
    pub definition_sha256: String,
    pub qualification_sha256: String,
}

/// The environment and managed working directory come from the frozen run, not the model.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProcessInvocation {
    pub program: String,
    pub arguments: Vec<String>,
    pub timeout_seconds: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProcessCapture {
    pub bytes: Vec<u8>,
    pub sha256: String,
    pub total_bytes: u64,
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProcessTermination {
    Exited,
    Canceled,
    TimedOut,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProcessResult {
    pub termination: ProcessTermination,
    /// Diagnostic only. A PID by itself is not a runner identity or a reconciliation proof.
    pub process_id: Option<u32>,
    pub exit_code: Option<i64>,
    pub signal: Option<i32>,
    pub stdout: ProcessCapture,
    pub stderr: ProcessCapture,
    pub duration_ms: u64,
    pub output_complete: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProcessAttemptState {
    /// Launch permission was committed. This does not prove the process actually started.
    LaunchCommitted,
    Completed {
        result: ProcessResult,
    },
    ReconciledCompleted {
        result: ProcessResult,
        evidence_sha256: String,
    },
    ReconciledNotStarted {
        evidence_sha256: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProcessAttempt {
    pub id: String,
    pub run_id: String,
    pub project_id: String,
    pub tool: ToolCall,
    pub environment: ExecutionEnvironmentRef,
    pub workspace_resource_id: String,
    pub request_sha256: String,
    pub state: ProcessAttemptState,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
}
