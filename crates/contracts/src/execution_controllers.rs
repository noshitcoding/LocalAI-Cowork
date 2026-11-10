use super::*;

/// Review identity of the current native FileBroker installation. Neither this
/// reference nor its generation authenticates a caller or qualifies a sandbox.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BrokerControllerRef {
    pub id: String,
    pub generation: i64,
}
