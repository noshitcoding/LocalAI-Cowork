#![recursion_limit = "512"]
mod agent_config;
mod model_requests;
pub use agent_config::{
    HYBRID_EXECUTION_MARKER, consecutive_tool_failures, freeze_behavior_defaults, hybrid_planning,
    merge_agent_values, narrow_access_mode, narrow_execution_mode, validate_agent_values,
};
mod model_tools;
use cowork_contracts::*;
pub use model_requests::{model_request_fingerprint, workflow_preparation_fingerprint};
pub use model_tools::model_tool_catalog;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
mod accounting;
pub use accounting::*;
pub mod builtin_tools;
pub mod environments;
pub mod execution_inputs;
pub mod execution_targets;
pub mod knowledge_provenance;
pub mod mcp;
pub mod processes;
pub mod run_workspaces;
pub mod structured_output;
pub mod workflows;
pub mod workspace_transfer;
pub use builtin_tools::{document_tool, document_write_tool, read_only_tool};
pub mod automation;
pub mod documents;
pub mod file_watch;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Validation(String),
    #[error("The requested object does not exist")]
    NotFound,
    /// Definitive absence from a read-only session inspection, never a lost reply.
    #[error("No sandbox session exists for this workspace")]
    SandboxSessionNotFound,
    #[error("The resource changed; refresh and review before retrying")]
    Conflict,
    #[error("Prepare and confirm an execution plan with a current client before starting")]
    ExecutionPlanRequired,
    #[error("The prepared execution plan expired; review a new plan")]
    ExecutionPlanExpired,
    #[error("Execution prerequisites changed; review a new plan")]
    ExecutionPlanChanged,
    #[error("The requested action is not permitted")]
    Forbidden,
    #[error("The selected workflow was revoked before execution started")]
    WorkflowRevoked,
    #[error(
        "This workflow has no verified execution clock; start a new run after reviewing its saved history"
    )]
    WorkflowClockUnverified,
    #[error(
        "The clock moved behind this workflow's last durable observation; new execution is blocked"
    )]
    WorkflowClockRollback,
    #[error("The execution lease is stale")]
    StaleLease,
    #[error("The run's execution budget has been exhausted")]
    BudgetExceeded,
    #[error(
        "Run token limits must cover a full configured context window and leave input capacity"
    )]
    BudgetConfiguration,
    #[error("Conversation and tool definitions exceed the configured conservative context limit")]
    ContextLimit,
    #[error("The folder is owned by another operation; retry when it is released")]
    WorkspaceBusy,
    #[error("File transfer capacity is occupied; retry after another transfer finishes")]
    TransferBusy,
    #[error("The file changed since it was read; review the current version")]
    FileConflict,
    #[error("The last external operation has an uncertain outcome and requires reconciliation")]
    UncertainEffect,
    #[error("The MCP session cannot continue; its connection state requires review")]
    McpSessionUnavailable,
    #[error("The native key is unavailable or changed; review the saved setup before continuing")]
    NativeKeyUnavailable,
    #[error("The configured provider is unavailable: {0}")]
    Provider(String),
    #[error("Storage operation failed")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),
}
pub type Result<T> = std::result::Result<T, Error>;
impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Validation(reason) => match reason.as_str() {
                "automation_inputs_invalid" => "automation_inputs_invalid",
                "automation_inputs_workflow_required" => "automation_inputs_workflow_required",
                "automation_inputs_value_missing" => "automation_inputs_value_missing",
                "automation_inputs_type_mismatch" => "automation_inputs_type_mismatch",
                "automation_inputs_workflow_unavailable" => {
                    "automation_inputs_workflow_unavailable"
                }
                "automation_inputs_snapshot_mismatch" => "automation_inputs_snapshot_mismatch",
                "workflow_control_invalid" => "workflow_control_invalid",
                "workflow_control_ambiguous_selection" => "workflow_control_ambiguous_selection",
                "workflow_data_invalid" => "workflow_data_invalid",
                "workflow_data_input_required" => "workflow_data_input_required",
                "workflow_data_dependency_missing" => "workflow_data_dependency_missing",
                "workflow_data_type_mismatch" => "workflow_data_type_mismatch",
                "workflow_data_value_missing" => "workflow_data_value_missing",
                "workflow_data_invalid_json" => "workflow_data_invalid_json",
                _ => "invalid_request",
            },
            Self::NotFound => "not_found",
            Self::SandboxSessionNotFound => "sandbox_session_not_found",
            Self::Conflict => "revision_conflict",
            Self::ExecutionPlanRequired => "execution_plan_required",
            Self::ExecutionPlanExpired => "execution_plan_expired",
            Self::ExecutionPlanChanged => "execution_plan_changed",
            Self::Forbidden => "forbidden",
            Self::WorkflowRevoked => "workflow_revoked",
            Self::WorkflowClockUnverified => "workflow_clock_unverified",
            Self::WorkflowClockRollback => "workflow_clock_rollback",
            Self::StaleLease => "stale_lease",
            Self::BudgetExceeded => "budget_exceeded",
            Self::BudgetConfiguration => "budget_configuration",
            Self::ContextLimit => "context_limit",
            Self::WorkspaceBusy => "workspace_busy",
            Self::TransferBusy => "transfer_busy",
            Self::FileConflict => "file_conflict",
            Self::UncertainEffect => "uncertain_effect",
            Self::McpSessionUnavailable => "mcp_session_unavailable",
            Self::NativeKeyUnavailable => "native_key_unavailable",
            Self::Provider(reason) => match reason.as_str() {
                "http_400" | "http_422" => "provider_request_rejected",
                "http_401" => "provider_authentication_failed",
                "http_403" => "provider_access_denied",
                "http_404" => "provider_endpoint_not_found",
                "http_429" => "provider_rate_limited",
                "connection_timeout" => "provider_connection_timeout",
                "tls_failed" => "provider_tls_failed",
                "connection_failed" => "provider_connection_failed",
                "model_missing" => "provider_model_missing",
                "credential_missing" => "provider_credential_missing",
                "invalid_model_catalog" => "provider_catalog_invalid",
                "model_catalog_interrupted" => "provider_catalog_interrupted",
                "model_catalog_too_large" => "provider_catalog_too_large",
                _ => "provider_unavailable",
            },
            Self::Storage(_) => "storage_unavailable",
        }
    }
}
pub fn storage_error(e: impl std::error::Error + Send + Sync + 'static) -> Error {
    Error::Storage(Box::new(e))
}
pub fn valid_text(value: &str, max: usize) -> Result<()> {
    if value.trim().is_empty() || value.len() > max || value.contains('\0') {
        return Err(Error::Validation(format!(
            "Text must contain 1–{max} bytes"
        )));
    }
    Ok(())
}
pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn canonical_hash(value: &impl Serialize) -> String {
    // serde_json maps are sorted (preserve_order is deliberately not enabled).
    hash(
        serde_json::to_string(&serde_json::to_value(value).expect("serializable domain value"))
            .expect("json")
            .as_bytes(),
    )
}
pub fn terminal(state: &RunState) -> bool {
    matches!(
        state,
        RunState::Completed | RunState::Failed | RunState::Canceled | RunState::Expired
    )
}
pub fn can_transition(from: &RunState, to: &RunState) -> bool {
    use RunState::*;
    matches!(
        (from, to),
        (
            Queued,
            Running
                | WaitingForExecutor
                | WaitingForSnapshot
                | CancelRequested
                | Canceled
                | Failed
                | Expired
        ) | (
            WaitingForExecutor | WaitingForSnapshot,
            Queued | Running | CancelRequested | Canceled | Failed | Expired
        ) | (WaitingForSnapshot, Completed | Interrupted)
            | (
                Running,
                WaitingApproval
                    | WaitingForSnapshot
                    | WaitingInput
                    | CancelRequested
                    | Interrupted
                    | Completed
                    | Failed
                    | Expired
            )
            | (
                WaitingApproval | WaitingInput,
                Queued | CancelRequested | Canceled | Expired
            )
            | (WaitingForTimer, Queued | Canceled | Expired | Failed)
            | (Running, WaitingForTimer)
            | (CancelRequested, Canceled | Interrupted)
            | (Interrupted, Queued | Canceled | Expired)
    )
}

pub fn remaining_run_millis(run: &RunRecord, timestamp: i64) -> u64 {
    let maximum = run.spec.budgets.max_seconds.saturating_mul(1000);
    let Some(start) = run.started_at else {
        return maximum;
    };
    if timestamp < start {
        return 0;
    }
    maximum.saturating_sub(timestamp.saturating_sub(start) as u64)
}
pub fn time_budget_exhausted(run: &RunRecord, timestamp: i64) -> bool {
    run.started_at.is_some() && remaining_run_millis(run, timestamp) == 0
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Lease {
    pub token: String,
    pub until: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Checkpoint {
    #[serde(default)]
    pub completion_pending: bool,
    pub messages: Vec<Value>,
    pub pending_tools: Vec<ToolCall>,
    pub approvals: Vec<Approval>,
    pub input_request: Option<ToolCall>,
    pub in_flight: Option<ToolCall>,
    #[serde(default)]
    pub pending_file_change: Option<FileChange>,
    #[serde(default)]
    pub pending_file_operation: Option<FileOperation>,
    pub last_model_content: String,
}

pub fn workspace_snapshot_hash(snapshot: &WorkspaceSnapshot) -> String {
    canonical_hash(
        &json!({"run_id":snapshot.run_id,"order_id":snapshot.order_id,"entries":snapshot.entries}),
    )
}

pub fn verify_workspace_snapshot(snapshot: &WorkspaceSnapshot) -> Result<()> {
    if workspace_snapshot_hash(snapshot) != snapshot.sha256 {
        return Err(Error::Validation(
            "Workspace snapshot integrity check failed".into(),
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredRun {
    pub record: RunRecord,
    pub checkpoint: Checkpoint,
    pub lease: Option<Lease>,
}
impl StoredRun {
    pub fn transition(&mut self, state: RunState) -> Result<()> {
        if !can_transition(&self.record.state, &state) {
            return Err(Error::Conflict);
        }
        self.record.state = state;
        Ok(())
    }
    pub fn assert_lease(&self, token: &str, now: i64) -> Result<()> {
        match &self.lease {
            Some(lease) if lease.token == token && lease.until > now => Ok(()),
            _ => Err(Error::StaleLease),
        }
    }
}
pub fn approval_fingerprint(run: &RunRecord, tool: &ToolCall) -> String {
    let mut bound = json!({"run": run.id, "tool": tool,
        "target": run.spec.executor, "policy_revision": run.spec.policy_revision});
    // Preserve historical fingerprints for runs without an execution environment.
    if let Some(environment) = &run.spec.environment {
        bound["environment"] = json!(environment);
    }
    canonical_hash(&bound)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_runs_cannot_restart_and_cancel_is_acknowledged() {
        assert!(!can_transition(&RunState::Completed, &RunState::Running));
        assert!(!can_transition(
            &RunState::CancelRequested,
            &RunState::Completed
        ));
        assert!(can_transition(
            &RunState::CancelRequested,
            &RunState::Canceled
        ));
        assert!(!can_transition(
            &RunState::WaitingApproval,
            &RunState::Completed
        ));
    }
    #[test]
    fn hashing_ignores_json_object_key_order() {
        let a: Value = serde_json::from_str(r#"{"b":2,"a":1}"#).unwrap();
        let b: Value = serde_json::from_str(r#"{"a":1,"b":2}"#).unwrap();
        assert_eq!(canonical_hash(&a), canonical_hash(&b));
    }
}
