//! A staged upload is input content, never a reusable execution permission.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrepareImportExecutionPlanV2 {
    pub target: OwnerExecutionTargetV2,
    pub request: CreateImportRun,
    pub input: ExecutionInputRef,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportExecutionPreflightV2 {
    pub ready: bool,
    pub executor: String,
    pub import: ImportFile,
    pub upload: FileUpload,
    pub folders: Vec<FolderGrant>,
    pub allowed_tools: Vec<String>,
    pub access_mode: AccessMode,
    pub budgets: Budgets,
}
