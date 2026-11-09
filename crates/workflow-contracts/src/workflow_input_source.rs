//! References to immutable native workflow input. These convey data, never rights.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInputSource {
    pub run_id: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInputSnapshot {
    pub source: WorkflowInputSource,
    pub project_id: String,
    pub workflow_id: String,
    pub version_id: String,
    pub definition_sha256: String,
    /// Exact canonical native JSON; browsers must not parse and reserialize it.
    pub canonical_json: String,
}
