use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
/// Frozen with the exact request and provider. This constrains the final answer,
/// while intermediate tool calls continue through their existing approvals.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StructuredOutput {
    pub name: String,
    pub schema: Value,
}
