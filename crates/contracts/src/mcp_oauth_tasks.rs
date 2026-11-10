use super::*;
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct McpOAuthConfiguration {
    pub resource: String,
    pub authorization_server: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
    pub enabled: bool,
    /// Explicit shared integration consent applies only to a Team profile.
    pub shared_team_consent: bool,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigureMcpOAuth {
    pub profile_id: String,
    pub server_id: String,
    pub server_revision: i64,
    pub expected_revision: i64,
    pub idempotency_key: String,
    pub metadata_sha256: String,
    pub configuration: McpOAuthConfiguration,
    /// Explicit containment acknowledgment allows a new configuration epoch
    /// after a lost token response; it never replays that original exchange.
    #[serde(default)]
    pub acknowledge_unknown: bool,
    /// Sent only to the private authenticated configuration route; never echoed.
    pub client_secret: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct McpOAuthProfile {
    pub id: String,
    pub owner_kind: String,
    pub owner_id: String,
    pub project_id: String,
    pub server_id: String,
    pub revision: i64,
    pub configuration: McpOAuthConfiguration,
    pub metadata_sha256: String,
    pub binding_sha256: String,
    pub state: String,
    pub expires_at: Option<i64>,
    pub reauthorization_required: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct McpOAuthFlow {
    pub id: String,
    pub profile_id: String,
    pub profile_revision: i64,
    pub state: String,
    pub authorization_url: Option<String>,
    pub expires_at: i64,
    pub callback_received: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct McpOAuthRequest {
    pub expected_revision: i64,
    pub idempotency_key: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct McpOAuthDiscovery {
    pub resource: String,
    pub server_id: String,
    pub server_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct McpOAuthMetadata {
    pub resource: String,
    pub resource_metadata_uri: String,
    pub authorization_servers: Vec<Value>,
    pub scopes_supported: Vec<String>,
    pub suggested_scopes: Vec<String>,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct McpAsyncTask {
    pub id: String,
    pub run_id: String,
    pub tool_call_id: String,
    pub server_id: String,
    pub revision: i64,
    pub state: String,
    pub protocol: String,
    pub expires_at: Option<i64>,
    pub poll_interval_ms: u64,
    pub operation_pending: bool,
    /// Protocol cancellation and failure do not establish physical no-effect.
    pub effects_unknown: bool,
    pub result_sha256: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct McpAsyncTaskRequest {
    pub expected_revision: i64,
    pub idempotency_key: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct McpTaskDetail {
    pub task: McpAsyncTask,
    pub original_result: Option<Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct McpTaskInteraction {
    pub id: String,
    pub task_record_id: String,
    pub run_id: String,
    pub server_id: String,
    pub revision: i64,
    pub method: String,
    pub state: String,
    pub request_sha256: String,
    pub original_request: Value,
    pub generated_response: Option<Value>,
    pub expires_at: i64,
    pub response_sha256: Option<String>,
    pub effects_unknown: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnswerMcpTaskInteraction {
    pub expected_revision: i64,
    pub idempotency_key: String,
    /// accept/decline/cancel for forms; approve_sampling/release_sample/decline_sample for model IO.
    pub action: String,
    pub content: Option<Value>,
    pub edited_messages: Option<Vec<Value>>,
}
