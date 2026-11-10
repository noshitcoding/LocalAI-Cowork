//! Additive original terminal wire shapes; a ticket is one original stream, never execution identity.
use super::*;
use uuid::Uuid;
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidCreateTerminalSession {
    pub columns: u16,
    pub rows: u16,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidTerminalSessionTicket {
    pub schema_version: u16,
    pub session_id: Uuid,
    pub token: String,
    pub expires_at: String,
    pub protocol: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidTerminalSession {
    pub schema_version: u16,
    pub session_id: Uuid,
    pub run_id: Uuid,
    pub state: String,
    pub columns: u16,
    pub rows: u16,
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub created_at: String,
    pub frames: Vec<Value>,
    #[serde(default)]
    pub observations: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_receipt: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication: Option<Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidTerminalSessionPage {
    pub items: Vec<AndroidTerminalSession>,
    pub after: u64,
    pub observed_at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidTerminalResize {
    pub operation: String,
    pub columns: u16,
    pub rows: u16,
}
