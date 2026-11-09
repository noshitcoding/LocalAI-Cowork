//! Task protocol primitives. A task handle exists only after an original tools/call
//! ACK; it never grants replay of that call or proves absence of remote effects.
use crate::{CallFailure, CallResult, Error, PROTOCOL_VERSION, Result, Session, digest};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct TaskCapabilities {
    pub tools_call: bool,
    pub cancel: bool,
}
impl TaskCapabilities {
    pub(crate) fn negotiated(protocol: &str, capabilities: &Value) -> Result<Self> {
        let Some(tasks) = capabilities.get("tasks") else {
            return Ok(Self::default());
        };
        if !tasks.is_object() {
            return Err(Error::Protocol);
        }
        // Older protocol versions do not carry the2025-11-25 task contract.
        if protocol != PROTOCOL_VERSION {
            return Ok(Self::default());
        }
        let call = tasks.pointer("/requests/tools/call");
        let cancel = tasks.get("cancel");
        if call.is_some_and(|v| !v.is_object()) || cancel.is_some_and(|v| !v.is_object()) {
            return Err(Error::Protocol);
        }
        Ok(Self {
            tools_call: call.is_some(),
            cancel: cancel.is_some(),
        })
    }
    pub(crate) fn augment(&self, definition: &Value) -> Result<bool> {
        let support = definition
            .pointer("/execution/taskSupport")
            .and_then(Value::as_str);
        match support {
            None | Some("forbidden") => Ok(false),
            Some("optional") => Ok(self.tools_call),
            Some("required") if self.tools_call => Ok(true),
            Some("required") => Err(Error::SessionUnavailable),
            _ => Err(Error::Protocol),
        }
    }
}
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskOperation {
    Get,
    Result,
    Cancel,
}
impl TaskOperation {
    pub fn method(&self) -> &'static str {
        match self {
            Self::Get => "tasks/get",
            Self::Result => "tasks/result",
            Self::Cancel => "tasks/cancel",
        }
    }
}
/// One actual RPC opportunity. Persist its intent before passing it back to the
/// same original Session. It is never Clone or Deserialize.
pub struct PreparedTaskOperation {
    connection: String,
    epoch: u64,
    original: PendingTask,
    rpc_request_id: String,
    operation: TaskOperation,
    request_sha256: String,
    params: Value,
    deadline_ms: u64,
}
impl PreparedTaskOperation {
    pub fn connection_id(&self) -> &str {
        &self.connection
    }
    pub fn rpc_request_id(&self) -> &str {
        &self.rpc_request_id
    }
    pub fn request_sha256(&self) -> &str {
        &self.request_sha256
    }
    pub fn task_id(&self) -> &str {
        &self.original.task_id
    }
    pub fn original_rpc_id(&self) -> &str {
        &self.original.original_rpc_id
    }
    pub fn original_request_sha256(&self) -> &str {
        &self.original.original_request_sha256
    }
    pub fn operation(&self) -> TaskOperation {
        self.operation
    }
    pub fn deadline_ms(&self) -> u64 {
        self.deadline_ms
    }
}
pub enum TaskOperationResult {
    Status(TaskAcknowledgement),
    Result(CallResult),
    Error { response: Value, sha256: String },
}
impl Session {
    pub fn prepare_task_operation(
        &self,
        ack: &TaskAcknowledgement,
        operation: TaskOperation,
    ) -> Result<PreparedTaskOperation> {
        self.prepare_task_operation_bounded(ack, operation, self.transport.limits.timeout_ms)
    }
    pub fn prepare_task_operation_bounded(
        &self,
        ack: &TaskAcknowledgement,
        operation: TaskOperation,
        deadline_ms: u64,
    ) -> Result<PreparedTaskOperation> {
        if !(1..=86_400_000).contains(&deadline_ms) {
            return Err(Error::Configuration);
        }
        self.assert_ready()?;
        ack.validate()?;
        let original = self
            .pending_task
            .as_ref()
            .filter(|p| p.task_id == ack.task_id)
            .ok_or(Error::SessionUnavailable)?
            .clone();
        if self.transport.protocol_version() != PROTOCOL_VERSION
            || !self
                .task_capabilities
                .as_ref()
                .is_some_and(|c| c.tools_call)
            || operation == TaskOperation::Cancel
                && !self.task_capabilities.as_ref().is_some_and(|c| c.cancel)
            || operation == TaskOperation::Cancel && ack.status.terminal()
        {
            return Err(Error::SessionUnavailable);
        }
        let rpc_request_id = uuid::Uuid::new_v4().to_string();
        let params = json!({"taskId":ack.task_id});
        let request_sha256 = digest(&json!([
            1,
            self.transport.endpoint_identity(),
            self.identity,
            self.epoch,
            PROTOCOL_VERSION,
            original.original_rpc_id,
            original.original_request_sha256,
            operation.method(),
            rpc_request_id,
            params,
            deadline_ms
        ]));
        Ok(PreparedTaskOperation {
            connection: self.connection_id(),
            epoch: self.epoch,
            original,
            rpc_request_id,
            operation,
            request_sha256,
            params,
            deadline_ms,
        })
    }
    pub async fn task_operation(
        &mut self,
        prepared: PreparedTaskOperation,
        ack: &TaskAcknowledgement,
    ) -> std::result::Result<TaskOperationResult, CallFailure> {
        self.task_operation_inner(prepared, ack, None).await
    }
    pub async fn task_operation_interactive(
        &mut self,
        prepared: PreparedTaskOperation,
        ack: &TaskAcknowledgement,
        handler: &dyn crate::TaskInteractionHandler,
    ) -> std::result::Result<TaskOperationResult, CallFailure> {
        let handler = if self.task_interactions {
            Some(handler)
        } else {
            None
        };
        self.task_operation_inner(prepared, ack, handler).await
    }
    async fn task_operation_inner(
        &mut self,
        prepared: PreparedTaskOperation,
        ack: &TaskAcknowledgement,
        handler: Option<&dyn crate::TaskInteractionHandler>,
    ) -> std::result::Result<TaskOperationResult, CallFailure> {
        let before = || CallFailure {
            code: Error::SessionUnavailable,
            outcome_uncertain: false,
            response: None,
        };
        if !self.ready
            || self.connection_id() != prepared.connection
            || self.epoch != prepared.epoch
            || self.pending_task.as_ref() != Some(&prepared.original)
            || ack.task_id != prepared.original.task_id
        {
            return Err(before());
        }
        self.ready = false;
        let context = crate::interactions::TaskRequestContext {
            connection_id: prepared.connection.clone(),
            connection_epoch: prepared.epoch,
            task_id: ack.task_id.clone(),
            operation_rpc_id: prepared.rpc_request_id.clone(),
        };
        let reply = if let Some(handler) = handler {
            self.transport
                .request_task_interactive(
                    prepared.operation.method(),
                    prepared.params,
                    prepared.rpc_request_id,
                    prepared.deadline_ms,
                    context,
                    handler,
                )
                .await
        } else {
            self.transport
                .request_task_with_id(
                    prepared.operation.method(),
                    prepared.params,
                    prepared.rpc_request_id,
                    prepared.deadline_ms,
                )
                .await
        }
        .map_err(|code| CallFailure {
            code,
            outcome_uncertain: true,
            response: None,
        })?;
        let response = match reply.result {
            Ok(response) => response,
            Err(code) => {
                if prepared.operation == TaskOperation::Result
                    && let Some(response) = reply.error_evidence.as_ref()
                {
                    let response = response.clone();
                    let ids = [
                        "/_meta/io.modelcontextprotocol~1related-task/taskId",
                        "/error/data/_meta/io.modelcontextprotocol~1related-task/taskId",
                    ]
                    .into_iter()
                    .filter_map(|p| response.pointer(p))
                    .collect::<Vec<_>>();
                    if ids.is_empty()
                        || ids
                            .iter()
                            .any(|id| id.as_str() != Some(ack.task_id.as_str()))
                    {
                        return Err(CallFailure {
                            code: Error::Protocol,
                            outcome_uncertain: true,
                            response: Some(response),
                        });
                    }
                    return Ok(TaskOperationResult::Error {
                        sha256: digest(&response),
                        response,
                    });
                }
                return Err(CallFailure {
                    code,
                    outcome_uncertain: true,
                    response: reply.error_evidence,
                });
            }
        };
        let invalid = |code| CallFailure {
            code,
            outcome_uncertain: true,
            response: Some(response.clone()),
        };
        if reply.catalog_changed {
            return Err(invalid(Error::CatalogChanged));
        }
        if prepared.operation != TaskOperation::Result {
            let next = TaskAcknowledgement::from_wire(response.clone(), Some(&ack.task_id))
                .map_err(invalid)?;
            ack.assert_next(&next).map_err(invalid)?;
            if prepared.operation == TaskOperation::Cancel && next.status != TaskStatus::Cancelled {
                return Err(invalid(Error::Protocol));
            }
            self.ready = true;
            return Ok(TaskOperationResult::Status(next));
        }
        ack.check_result(&response).map_err(invalid)?;
        self.catalog
            .as_ref()
            .ok_or_else(before)?
            .validate_result(&prepared.original.tool_name, &response)
            .map_err(invalid)?;
        self.pending_task = None;
        self.ready = true;
        Ok(TaskOperationResult::Result(CallResult {
            response_sha256: digest(&response),
            response,
            catalog_changed: false,
            task: None,
        }))
    }
    pub fn has_pending_task(&self) -> bool {
        self.pending_task.is_some()
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Working,
    InputRequired,
    Completed,
    Failed,
    Cancelled,
}
impl TaskStatus {
    pub fn terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// Bounded original wire fact, privately encrypted by the product. Deliberately
/// has no Debug implementation: taskId can carry confidential remote identity.
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TaskAcknowledgement {
    pub(crate) task_id: String,
    pub status: TaskStatus,
    pub created_at_ms: i64,
    pub last_updated_at_ms: i64,
    pub ttl_ms: Option<u64>,
    pub poll_interval_ms: u64,
    pub(crate) original: Value,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct PendingTask {
    pub task_id: String,
    pub tool_name: String,
    pub original_rpc_id: String,
    pub original_request_sha256: String,
}
impl TaskAcknowledgement {
    pub fn task_id(&self) -> &str {
        &self.task_id
    }
    pub fn original(&self) -> &Value {
        &self.original
    }
    pub fn expires_at_ms(&self) -> Option<i64> {
        self.ttl_ms
            .and_then(|t| i64::try_from(t).ok())
            .and_then(|t| self.created_at_ms.checked_add(t))
    }
    pub fn validate(&self) -> Result<()> {
        let reconstructed = Self::from_wire(self.original.clone(), Some(&self.task_id))?;
        if &reconstructed != self {
            return Err(Error::Protocol);
        }
        Ok(())
    }
    pub(crate) fn from_wire(value: Value, expected: Option<&str>) -> Result<Self> {
        let id = value
            .get("taskId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 1024 && !id.chars().any(char::is_control))
            .ok_or(Error::Protocol)?
            .to_owned();
        if expected.is_some_and(|expected| expected != id) {
            return Err(Error::Protocol);
        }
        let status = match value.get("status").and_then(Value::as_str) {
            Some("working") => TaskStatus::Working,
            Some("input_required") => TaskStatus::InputRequired,
            Some("completed") => TaskStatus::Completed,
            Some("failed") => TaskStatus::Failed,
            Some("cancelled") => TaskStatus::Cancelled,
            _ => return Err(Error::Protocol),
        };
        let date = |name: &str| -> Result<i64> {
            let text = value
                .get(name)
                .and_then(Value::as_str)
                .ok_or(Error::Protocol)?;
            chrono::DateTime::parse_from_rfc3339(text)
                .map(|d| d.timestamp_millis())
                .map_err(|_| Error::Protocol)
        };
        let created_at_ms = date("createdAt")?;
        let last_updated_at_ms = date("lastUpdatedAt")?;
        if created_at_ms <= 0 || last_updated_at_ms < created_at_ms {
            return Err(Error::Protocol);
        }
        let ttl_ms = match value.get("ttl") {
            Some(Value::Null) => None,
            Some(v) => Some(
                v.as_u64()
                    .filter(|t| *t <= i64::MAX as u64)
                    .ok_or(Error::Protocol)?,
            ),
            None => return Err(Error::Protocol),
        };
        if ttl_ms.is_some_and(|t| created_at_ms.checked_add(t as i64).is_none()) {
            return Err(Error::Protocol);
        }
        let poll_interval_ms = match value.get("pollInterval") {
            None => 1000,
            Some(v) => v
                .as_u64()
                .filter(|v| *v > 0 && *v <= 86_400_000)
                .ok_or(Error::Protocol)?,
        };
        if value
            .get("statusMessage")
            .is_some_and(|s| s.as_str().is_none_or(|s| s.len() > 16_384))
        {
            return Err(Error::Protocol);
        }
        Ok(Self {
            task_id: id,
            status,
            created_at_ms,
            last_updated_at_ms,
            ttl_ms,
            poll_interval_ms,
            original: value,
        })
    }
    pub(crate) fn assert_next(&self, next: &Self) -> Result<()> {
        if self.task_id != next.task_id
            || self.created_at_ms != next.created_at_ms
            || self.ttl_ms != next.ttl_ms
            || next.last_updated_at_ms < self.last_updated_at_ms
            || (self.status.terminal() && self.status != next.status)
        {
            return Err(Error::Protocol);
        }
        Ok(())
    }
    pub(crate) fn check_result(&self, value: &Value) -> Result<()> {
        if value
            .pointer("/_meta/io.modelcontextprotocol~1related-task/taskId")
            .and_then(Value::as_str)
            != Some(self.task_id.as_str())
        {
            return Err(Error::Protocol);
        }
        Ok(())
    }
}
