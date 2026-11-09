use crate::McpTaskInteraction;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
/// Explicit keyset pages keep full original server requests and samples within
/// the private Native IPC response bound. No record bytes are abbreviated.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OfflineMcpInteractionPage {
    pub items: Vec<McpTaskInteraction>,
    pub next_cursor: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OfflineMcpResultBytes {
    pub bytes_base64: String,
    pub sha256: String,
    pub size: u64,
}
