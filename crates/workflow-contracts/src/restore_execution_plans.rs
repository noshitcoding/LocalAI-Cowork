//! Deterministic restoration participates in the same confirmed copy admission.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrepareRestoreExecutionPlanV2 {
    pub target: OwnerExecutionTargetV2,
    pub request: CreateRestoreRun,
    pub input: ExecutionInputRef,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RestoreContentBindingV2 {
    pub kind: FileNodeKind,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RestoreExecutionPreflightV2 {
    pub ready: bool,
    pub executor: String,
    /// The server fills the exact retained-content hash before review.
    pub restore: RestoreFile,
    pub content: RestoreContentBindingV2,
    pub folders: Vec<FolderGrant>,
    pub allowed_tools: Vec<String>,
    pub access_mode: AccessMode,
    pub budgets: Budgets,
}
