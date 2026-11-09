//! Private MCP transport primitive. This is NOT an authorization or retry layer.
//! Callers must durably admit each exact request through the shared policy broker
//! before dispatch, retain uncertain outcomes, and never reconstruct a dispatch
//! permit after restart. No daemon/model tool is exposed by this crate.
mod catalog;
mod checkpoint;
mod interactions;
pub mod oauth;
pub use interactions::{
    TaskInteractionHandler, TaskServerRequest, TaskServerResponse, sampling_text_messages,
    validate_task_form,
};
mod tasks;
mod transport;
mod wire;

pub use catalog::{Catalog, ToolDefinition};
pub use checkpoint::{CheckpointScope, SealedSession, SessionCipher};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::Duration;
pub use tasks::{
    PreparedTaskOperation, TaskAcknowledgement, TaskCapabilities, TaskOperation,
    TaskOperationResult, TaskStatus,
};
use transport::Transport;
pub use transport::{Endpoint, Limits, public_https_addresses};
use uuid::Uuid;

pub use cowork_contracts::MCP_PROTOCOL_VERSION as PROTOCOL_VERSION;

/// Stable, intentionally content-free diagnostics. Server bodies, URLs and
/// credentials never become error messages or Debug output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("invalid_mcp_configuration")]
    Configuration,
    #[error("mcp_transport_failed")]
    Transport,
    #[error("mcp_deadline_exceeded")]
    Deadline,
    #[error("invalid_mcp_protocol")]
    Protocol,
    #[error("mcp_limit_exceeded")]
    Limit,
    #[error("unsupported_mcp_schema")]
    Schema,
    #[error("invalid_mcp_arguments")]
    Arguments,
    #[error("mcp_catalog_changed")]
    CatalogChanged,
    #[error("mcp_session_unavailable")]
    SessionUnavailable,
    #[error("mcp_remote_error")]
    Remote,
    #[error("mcp_tool_reported_error")]
    ToolError,
}
pub type Result<T> = std::result::Result<T, Error>;

pub struct CallFailure {
    pub code: Error,
    /// True means the caller MUST retain its uncertain-effect marker. A network
    /// error, JSON-RPC error, or isError result is not proof that nothing changed.
    /// False only rejects THIS dispatch before I/O; it never resolves an older attempt.
    pub outcome_uncertain: bool,
    /// Untrusted evidence, for protected persistence/review, never a log message.
    response: Option<Value>,
}
impl std::fmt::Debug for CallFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallFailure")
            .field("code", &self.code)
            .field("outcome_uncertain", &self.outcome_uncertain)
            .field("has_protected_response", &self.response.is_some())
            .finish()
    }
}
impl CallFailure {
    pub fn response(&self) -> Option<&Value> {
        self.response.as_ref()
    }
}

/// Neither Clone nor Deserialize: one prepared call has one dispatch opportunity.
pub struct PreparedCall {
    endpoint: String,
    rpc_request_id: String,
    timeout_ms: u64,
    session: Uuid,
    epoch: u64,
    catalog_sha256: String,
    tool_name: String,
    arguments: Value,
    request_sha256: String,
    task_ttl_ms: Option<u64>,
    protocol: String,
}
impl PreparedCall {
    pub fn task_ttl_ms(&self) -> Option<u64> {
        self.task_ttl_ms
    }
    pub fn protocol(&self) -> &str {
        &self.protocol
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn connection_id(&self) -> String {
        self.session.to_string()
    }
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
    pub fn rpc_request_id(&self) -> &str {
        &self.rpc_request_id
    }
    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }
    pub fn arguments(&self) -> &Value {
        &self.arguments
    }
    pub fn request_sha256(&self) -> &str {
        &self.request_sha256
    }
    pub fn catalog_sha256(&self) -> &str {
        &self.catalog_sha256
    }
}

pub struct CallResult {
    /// Bounded, schema-checked but still untrusted content. Resource URLs are not fetched.
    pub response: Value,
    pub response_sha256: String,
    pub catalog_changed: bool,
    /// Present only for an actually negotiated, original tools/call task ACK.
    pub task: Option<TaskAcknowledgement>,
}

pub struct Session {
    transport: Transport,
    identity: Uuid,
    epoch: u64,
    ready: bool,
    catalog: Option<Catalog>,
    task_capabilities: Option<TaskCapabilities>,
    pending_task: Option<tasks::PendingTask>,
    task_interactions: bool,
}
impl Session {
    /// The caller revalidates the original OAuth profile and current run authority
    /// before replacing only the bearer of this still-held HTTP session.
    /// No endpoint, protocol, session, catalogue or connection epoch is changed.
    pub fn replace_bearer(&mut self, bearer: &str) -> Result<()> {
        self.assert_ready()?;
        self.transport.replace_bearer(bearer)
    }
    pub fn connection_id(&self) -> String {
        self.identity.to_string()
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub async fn connect(endpoint: Endpoint, limits: Limits) -> Result<Self> {
        Self::connect_capabilities(endpoint, limits, false, false).await
    }
    /// Product paths that actually supply original task-bound handlers may
    /// advertise basic sampling and form elicitation. No tools/context/URL
    /// sampling capability is implied by this constructor.
    pub async fn connect_interactive(endpoint: Endpoint, limits: Limits) -> Result<Self> {
        Self::connect_capabilities(endpoint, limits, true, true).await
    }
    /// Form-only product path. Sampling is not advertised without an original model context.
    pub async fn connect_form_interactive(endpoint: Endpoint, limits: Limits) -> Result<Self> {
        Self::connect_capabilities(endpoint, limits, true, false).await
    }
    async fn connect_capabilities(
        endpoint: Endpoint,
        limits: Limits,
        interactions: bool,
        sampling: bool,
    ) -> Result<Self> {
        let mut transport = Transport::new(endpoint, limits)?;
        let protocol = transport.protocol_version();
        let reply = transport
            .request(
                "initialize",
                json!({
                    "protocolVersion":protocol,
                    "capabilities":if protocol==PROTOCOL_VERSION&&interactions&&sampling{json!({"tasks":{},"sampling":{},"elicitation":{"form":{}}})}else if protocol==PROTOCOL_VERSION&&interactions{json!({"tasks":{},"elicitation":{"form":{}}})}else if protocol==PROTOCOL_VERSION{json!({"tasks":{}})}else{json!({})},
                    "clientInfo":{"name":"LocalAI Cowork V2","version":env!("CARGO_PKG_VERSION")}
                }),
                true,
            )
            .await?;
        let value = reply.result?;
        transport.negotiate_protocol(
            value
                .get("protocolVersion")
                .and_then(Value::as_str)
                .ok_or(Error::Protocol)?,
        )?;
        let task_capabilities = if value.pointer("/capabilities/tasks").is_some() {
            Some(TaskCapabilities::negotiated(
                transport.protocol_version(),
                &value["capabilities"],
            )?)
        } else {
            None
        };
        if !value
            .get("capabilities")
            .and_then(|c| c.get("tools"))
            .is_some_and(Value::is_object)
            || !value
                .get("serverInfo")
                .and_then(|s| s.get("name"))
                .is_some_and(Value::is_string)
            || !value
                .get("serverInfo")
                .and_then(|s| s.get("version"))
                .is_some_and(Value::is_string)
        {
            return Err(Error::Protocol);
        }
        // Server instructions/annotations never grant permissions or become system prompts.
        transport
            .notify(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await?;
        let task_interactions = interactions && transport.protocol_version() == PROTOCOL_VERSION;
        Ok(Self {
            transport,
            identity: Uuid::new_v4(),
            epoch: 0,
            ready: true,
            catalog: None,
            task_capabilities,
            pending_task: None,
            task_interactions,
        })
    }

    /// Explicit catalogue discovery only; no discovery/reconnect during a tool call.
    pub async fn discover(&mut self) -> Result<&Catalog> {
        self.assert_ready()?;
        if self.pending_task.is_some() {
            return Err(Error::SessionUnavailable);
        }
        self.ready = false;
        self.catalog = None;
        self.epoch = self.epoch.checked_add(1).ok_or(Error::SessionUnavailable)?;
        let limits = self.transport.limits;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(limits.timeout_ms);
        let mut cursor: Option<String> = None;
        let mut seen = std::collections::HashSet::new();
        let mut definitions = Vec::new();
        let mut bytes = 0;
        for _ in 0..limits.max_pages {
            let params = cursor.as_ref().map_or(json!({}), |c| json!({"cursor":c}));
            let reply = tokio::time::timeout_at(
                deadline,
                self.transport.request("tools/list", params, false),
            )
            .await
            .map_err(|_| Error::Deadline)??;
            if reply.catalog_changed {
                return Err(Error::CatalogChanged);
            }
            let value = reply.result?;
            bytes += serde_json::to_vec(&value)
                .map_err(|_| Error::Protocol)?
                .len();
            if bytes > limits.max_catalog_bytes {
                return Err(Error::Limit);
            }
            let list = value
                .get("tools")
                .and_then(Value::as_array)
                .ok_or(Error::Protocol)?;
            if definitions.len() + list.len() > limits.max_tools {
                return Err(Error::Limit);
            }
            definitions.extend(list.iter().cloned());
            cursor = match value.get("nextCursor") {
                None => None,
                Some(Value::String(c)) if !c.is_empty() && c.len() <= 1024 => Some(c.clone()),
                _ => return Err(Error::Protocol),
            };
            if let Some(c) = &cursor {
                if !seen.insert(c.clone()) {
                    return Err(Error::Protocol);
                }
            } else {
                let catalog = Catalog::build(definitions)?;
                if tokio::time::Instant::now() >= deadline {
                    return Err(Error::Deadline);
                }
                self.catalog = Some(catalog);
                self.ready = true;
                return Ok(self.catalog.as_ref().expect("catalogue just installed"));
            }
        }
        Err(Error::Limit)
    }
    pub fn catalog(&self) -> Option<&Catalog> {
        self.catalog.as_ref()
    }
    pub fn is_usable(&self) -> bool {
        self.ready
    }
    pub fn protocol_version(&self) -> &'static str {
        self.transport.protocol_version()
    }
    pub fn task_capabilities(&self) -> Option<&TaskCapabilities> {
        self.task_capabilities.as_ref()
    }

    pub fn prepare(
        &self,
        catalog_sha256: &str,
        tool_name: &str,
        arguments: Value,
    ) -> Result<PreparedCall> {
        self.assert_ready()?;
        if self.pending_task.is_some() {
            return Err(Error::SessionUnavailable);
        }
        let catalog = self.catalog.as_ref().ok_or(Error::CatalogChanged)?;
        if catalog.sha256() != catalog_sha256 {
            return Err(Error::CatalogChanged);
        }
        catalog.validate_arguments(tool_name, &arguments)?;
        let rpc_request_id = Uuid::new_v4().to_string();
        let definition = catalog
            .tools()
            .find(|(name, _)| *name == tool_name)
            .ok_or(Error::Arguments)?
            .1
            .definition();
        let task_ttl_ms = if self
            .task_capabilities
            .as_ref()
            .unwrap_or(&TaskCapabilities::default())
            .augment(definition)?
        {
            Some(300_000)
        } else {
            None
        };
        let mut hash_input = json!({"endpoint":self.transport.endpoint_identity(),"session":self.identity,"rpc_request_id":rpc_request_id,
            "protocol":self.transport.protocol_version(),"catalog_sha256":catalog_sha256,
            "name":tool_name,"arguments":arguments});
        if let Some(ttl) = task_ttl_ms {
            hash_input["task"] = json!({"ttl":ttl});
        }
        let request_sha256 = digest(&hash_input);
        Ok(PreparedCall {
            endpoint: self.transport.endpoint_identity().into(),
            rpc_request_id,
            timeout_ms: self.transport.limits.timeout_ms,
            session: self.identity,
            epoch: self.epoch,
            catalog_sha256: catalog_sha256.into(),
            tool_name: tool_name.into(),
            arguments,
            request_sha256,
            task_ttl_ms,
            protocol: self.transport.protocol_version().into(),
        })
    }

    /// Must follow durable exact policy admission. Dropping this future after its
    /// first poll poisons the session before network I/O; it cannot resend/reconnect.
    pub async fn call(
        &mut self,
        call: PreparedCall,
    ) -> std::result::Result<CallResult, CallFailure> {
        if !self.ready
            || self.pending_task.is_some()
            || call.session != self.identity
            || call.epoch != self.epoch
            || self.catalog.as_ref().map(Catalog::sha256) != Some(call.catalog_sha256.as_str())
        {
            return Err(CallFailure {
                code: Error::SessionUnavailable,
                outcome_uncertain: false,
                response: None,
            });
        }
        self.ready = false;
        self.epoch = self.epoch.checked_add(1).ok_or(CallFailure {
            code: Error::SessionUnavailable,
            outcome_uncertain: false,
            response: None,
        })?;
        let mut params = json!({"name":call.tool_name,"arguments":call.arguments});
        if let Some(ttl) = call.task_ttl_ms {
            params["task"] = json!({"ttl":ttl});
        }
        let original_rpc_id = call.rpc_request_id.clone();
        let reply = self
            .transport
            .request_with_id("tools/call", params, false, call.rpc_request_id)
            .await
            .map_err(|code| CallFailure {
                code,
                outcome_uncertain: true,
                response: None,
            })?;
        let response = reply.result.map_err(|code| CallFailure {
            code,
            outcome_uncertain: true,
            response: reply.error_evidence,
        })?;
        if call.task_ttl_ms.is_some() {
            let ack = TaskAcknowledgement::from_wire(
                response.get("task").cloned().ok_or(CallFailure {
                    code: Error::Protocol,
                    outcome_uncertain: true,
                    response: Some(response.clone()),
                })?,
                None,
            )
            .map_err(|code| CallFailure {
                code,
                outcome_uncertain: true,
                response: Some(response.clone()),
            })?;
            self.pending_task = Some(tasks::PendingTask {
                task_id: ack.task_id().into(),
                tool_name: call.tool_name,
                original_rpc_id,
                original_request_sha256: call.request_sha256,
            });
            self.ready = !reply.catalog_changed;
            return Ok(CallResult {
                response_sha256: digest(&response),
                response,
                catalog_changed: reply.catalog_changed,
                task: Some(ack),
            });
        }
        if response.get("isError") == Some(&Value::Bool(true)) {
            return Err(CallFailure {
                code: Error::ToolError,
                outcome_uncertain: true,
                response: Some(response),
            });
        }
        if let Err(code) = self
            .catalog
            .as_ref()
            .expect("bound catalogue")
            .validate_result(&call.tool_name, &response)
        {
            return Err(CallFailure {
                code,
                outcome_uncertain: true,
                response: Some(response),
            });
        }
        if reply.catalog_changed {
            self.catalog = None;
        }
        self.ready = true;
        Ok(CallResult {
            response_sha256: digest(&response),
            response,
            catalog_changed: reply.catalog_changed,
            task: None,
        })
    }
    fn assert_ready(&self) -> Result<()> {
        if self.ready {
            Ok(())
        } else {
            Err(Error::SessionUnavailable)
        }
    }
}

fn digest(value: &Value) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).expect("JSON value"))
    )
}
