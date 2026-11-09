use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserEngine {
    #[default]
    Chrome,
    Firefox,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserMode {
    #[default]
    Managed,
    Extension,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateBrowserSession {
    #[serde(default)]
    pub engine: BrowserEngine,
    #[serde(default)]
    pub mode: BrowserMode,
    #[serde(default)]
    pub connection_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserCapabilities {
    pub live_view: bool,
    pub pointer: bool,
    pub keyboard: bool,
    pub developer_tools: bool,
    pub selected_tabs_only: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserSessionView {
    pub id: String,
    pub project_id: String,
    pub order_id: String,
    pub device_id: String,
    pub engine: BrowserEngine,
    pub mode: BrowserMode,
    pub epoch: String,
    pub manual: bool,
    pub connected: bool,
    pub connection_id: Option<String>,
    pub capabilities: BrowserCapabilities,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserCommand {
    pub operation: String,
    #[serde(default)]
    pub tab_id: Option<String>,
    #[serde(default)]
    pub document_id: Option<String>,
    #[serde(default)]
    pub selector: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub x: Option<f64>,
    #[serde(default)]
    pub y: Option<f64>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserExtensionConnection {
    pub id: String,
    pub engine: BrowserEngine,
    pub label: String,
    pub tabs: Vec<BrowserTab>,
    pub connected: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserTab {
    pub id: String,
    pub document_id: String,
    pub url: String,
    pub title: String,
    pub active: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserInput {
    pub idempotency_key: String,
    pub epoch: String,
    pub command: BrowserCommand,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserTakeover {
    pub manual: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteBrowserProfile {
    pub engine: BrowserEngine,
}
