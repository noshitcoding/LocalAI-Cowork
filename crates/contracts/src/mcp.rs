use super::*;

/// A human-selected reference. The daemon resolves and freezes its definition.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct McpToolSelection {
    pub server_id: String,
    pub server_revision: i64,
    pub tool_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct McpServerConfiguration {
    pub name: String,
    pub endpoint: String,
    /// Identifier only; native credential material must never be supplied here.
    pub credential_reference: Option<String>,
    pub credential_revision: i64,
    pub catalog: Vec<Value>,
    pub selected_tools: Vec<String>,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigureMcpServer {
    pub server_id: String,
    pub base_revision: i64,
    pub idempotency_key: String,
    pub configuration: McpServerConfiguration,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpServer {
    pub id: String,
    pub project_id: String,
    pub revision: i64,
    pub configuration: McpServerConfiguration,
    pub configuration_sha256: String,
    pub catalog_sha256: String,
}

/// A frozen, explicitly selected remote tool. Credentials never enter this record.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct McpToolBinding {
    pub alias: String,
    pub server_id: String,
    pub server_revision: i64,
    pub endpoint: String,
    pub configuration_sha256: String,
    pub catalog_sha256: String,
    pub definition: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct McpResult {
    pub response: Value,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum McpAttemptState {
    /// Admission was committed. Neither remote execution nor its absence is proven.
    DispatchCommitted,
    OutcomeUncertain {
        code: String,
        response: Option<Value>,
        response_sha256: Option<String>,
    },
    Completed {
        result: McpResult,
    },
    ReconciledCompleted {
        result: McpResult,
        evidence_sha256: String,
    },
    ReconciledNotExecuted {
        evidence_sha256: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct McpAttempt {
    pub id: String,
    pub run_id: String,
    pub project_id: String,
    pub tool: ToolCall,
    pub binding: McpToolBinding,
    pub request_sha256: String,
    pub transport_request_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_task_ttl_ms: Option<u64>,
    pub rpc_request_id: String,
    /// Local connection nonce, not the secret MCP-Session-Id header.
    pub connection_id: String,
    pub state: McpAttemptState,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
}
