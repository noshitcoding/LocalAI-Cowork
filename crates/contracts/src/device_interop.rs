//! Additive protocol-two device interop shapes. Inventory is never runtime authority.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeviceOfficeRequest {
    pub application: String,
    #[serde(default = "export_pdf")]
    pub action: String,
    pub source: String,
    pub output: String,
    #[serde(default)]
    pub preview_output: Option<String>,
    #[serde(default = "parameters")]
    pub parameters: Value,
}
fn export_pdf() -> String {
    "export_pdf".into()
}
fn parameters() -> Value {
    serde_json::json!({})
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeviceCrewAgent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_config: Option<crate::EffectiveAgentConfig>,
    pub id: String,
    pub role: String,
    pub goal: String,
    #[serde(default)]
    pub backstory: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub mcp_server_names: Vec<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub allow_delegation: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeviceCrewTask {
    pub id: String,
    pub agent_id: String,
    pub description: String,
    pub expected_output: String,
    #[serde(default)]
    pub context: Vec<String>,
    #[serde(default)]
    pub async_execution: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeviceCrewDefinition {
    pub format_version: u32,
    pub name: String,
    pub process: String,
    pub agents: Vec<DeviceCrewAgent>,
    pub tasks: Vec<DeviceCrewTask>,
    #[serde(default)]
    pub manager_agent_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeviceInteropAdapter {
    pub name: String,
    pub version: String,
    pub operations: Vec<String>,
    pub configured: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeviceInteropInventory {
    pub format: String,
    pub adapters: Vec<DeviceInteropAdapter>,
    pub automatic_replay: bool,
    pub adopted_processes: bool,
}
