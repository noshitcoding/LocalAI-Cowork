//! Actor-private workflow selection. Definitions are content, never execution authority.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AndroidWorkflowEngine {
    Workflow,
    Crew,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidWorkflowVersionRef {
    pub id: Uuid,
    pub revision: i64,
    pub definition_sha256: String,
    pub created_at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidWorkflowEntry {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub engine: AndroidWorkflowEngine,
    pub revision: i64,
    pub revoked: bool,
    pub published_version: Option<AndroidWorkflowVersionRef>,
    pub source_entity_id: Option<Uuid>,
    pub source_revision: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidWorkflowVersion {
    pub workflow: AndroidWorkflowEntry,
    pub version: AndroidWorkflowVersionRef,
    /// Original secret-free V1 Crew or typed V2 WorkflowDefinition JSON.
    pub definition: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidWorkflowPage {
    pub items: Vec<AndroidWorkflowEntry>,
    pub next_cursor: Option<Uuid>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidWorkflowVersionPage {
    pub versions: Vec<AndroidWorkflowVersionRef>,
    pub next_revision: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidWorkflowWrite {
    pub request_id: Uuid,
    pub project_id: Uuid,
    pub expected_revision: i64,
    pub name: String,
    pub engine: AndroidWorkflowEngine,
    pub definition: Value,
    #[serde(default)]
    pub publish: bool,
    #[serde(default)]
    pub source_entity_id: Option<Uuid>,
    #[serde(default)]
    pub source_revision: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidWorkflowChange {
    pub request_id: Uuid,
    pub expected_revision: i64,
    pub version_id: Option<Uuid>,
    pub revoke: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidWorkflowSelection {
    pub workflow_id: Uuid,
    pub version_id: Uuid,
    pub definition_sha256: String,
    pub registry_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidWorkflowPreflight {
    pub selection: AndroidWorkflowSelection,
    pub engine: AndroidWorkflowEngine,
    pub plan_sha256: String,
    pub required_capabilities: Vec<String>,
    pub roles: Vec<Value>,
    pub max_parallel: u16,
    pub ready: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidWorkflowOperationReceipt {
    pub request_id: Uuid,
    pub owner_id: Uuid,
    pub project_id: Uuid,
    pub request_sha256: String,
    pub workflow: AndroidWorkflowEntry,
    pub version: Option<AndroidWorkflowVersionRef>,
    pub committed_at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidWorkflowNodeView {
    pub node_id: String,
    pub role_id: String,
    pub state: String,
    pub dependencies: Vec<String>,
    pub model_steps: u64,
    pub tool_steps: u64,
    pub result_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_artifact_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_size: Option<u64>,
    pub original_receipt_sha256: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidWorkflowRunView {
    pub run_id: Uuid,
    pub owner_id: Uuid,
    pub project_id: Uuid,
    pub selection: AndroidWorkflowSelection,
    pub engine: AndroidWorkflowEngine,
    pub revision: i64,
    pub plan_sha256: String,
    pub checkpoint_sha256: String,
    pub nodes: Vec<AndroidWorkflowNodeView>,
    pub next_cursor: Option<String>,
}
