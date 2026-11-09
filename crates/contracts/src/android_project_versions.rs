//! Additive actor/team content versions. V1 route bodies retain their original fields.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidProjectVersion {
    pub schema_version: u16,
    pub id: Uuid,
    pub project_id: Uuid,
    pub revision: i64,
    pub parent_version_id: Option<Uuid>,
    pub merge_base_version_id: Option<Uuid>,
    pub snapshot_manifest_id: Uuid,
    pub created_by_run_id: Option<Uuid>,
    pub created_at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidCreateProjectVersion {
    pub snapshot_manifest_id: Uuid,
    pub parent_version_id: Option<Uuid>,
    pub merge_base_version_id: Option<Uuid>,
    pub created_by_run_id: Option<Uuid>,
    #[serde(default)]
    pub diff_summary: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidApplyProjectVersion {
    pub expected_project_revision: i64,
    pub expected_current_version_id: Option<Uuid>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AndroidMergeFileStatus {
    Unchanged,
    Added,
    Deleted,
    CurrentOnly,
    ResultOnly,
    IdenticalChange,
    AutoMerged,
    TextConflict,
    BinaryConflict,
    Renamed,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidMergeFileReview {
    pub path: String,
    pub renamed_from: Option<String>,
    pub status: AndroidMergeFileStatus,
    pub base_digest: Option<String>,
    pub current_digest: Option<String>,
    pub result_digest: Option<String>,
    pub auto_mergeable: bool,
    pub conflict_preview: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidProjectMergeReview {
    pub schema_version: u16,
    pub project_id: Uuid,
    pub base_version_id: Uuid,
    pub current_version_id: Uuid,
    pub result_version_id: Uuid,
    pub files: Vec<AndroidMergeFileReview>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AndroidMergeResolutionChoice {
    Current,
    Result,
    Delete,
    AutoMerged,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidMergeFileResolution {
    pub path: String,
    pub choice: AndroidMergeResolutionChoice,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidApplyProjectMerge {
    pub base_version_id: Uuid,
    pub current_version_id: Uuid,
    pub result_version_id: Uuid,
    pub expected_project_revision: i64,
    #[serde(default)]
    pub resolutions: Vec<AndroidMergeFileResolution>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AndroidForkDestination {
    PrivateLocal { name: String },
    TeamManaged { team_id: Uuid, name: String },
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidForkProject {
    pub source_version_id: Uuid,
    pub expected_source_project_revision: i64,
    pub destination: AndroidForkDestination,
    pub idempotency_key: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidForkObjectMapping {
    pub kind: String,
    pub source_id: Uuid,
    pub destination_id: Uuid,
    pub original_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidProjectForkReceipt {
    pub format: String,
    pub idempotency_key: String,
    pub source_project_id: Uuid,
    pub source_version_id: Uuid,
    pub destination_project_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_version_id: Option<Uuid>,
    pub destination_team_id: Option<Uuid>,
    pub mappings: Vec<AndroidForkObjectMapping>,
    pub original_content_sha256: String,
    pub execution_authority_copied: bool,
    pub committed_at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidForkThread {
    pub project_id: Uuid,
    pub title: String,
    pub forked_from_thread_id: Option<Uuid>,
    pub forked_from_message_id: Option<Uuid>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidProjectVersionContent {
    pub kind: String,
    pub source_id: Uuid,
    pub payload: Value,
    pub original_sha256: String,
}
