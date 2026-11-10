//! Versioned offline delegation. Public envelopes contain no bearer or private key.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OfflineDevice {
    pub id: Uuid,
    pub public_key: String,
    pub name: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OfflineDeviceProofRequest {
    pub issuer_origin: String,
    pub team_id: Uuid,
    pub project_id: Uuid,
    pub order_id: Uuid,
    pub idempotency_key: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OfflineDeviceProof {
    pub device: OfflineDevice,
    pub request: OfflineDeviceProofRequest,
    pub signature: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OfflineSource {
    pub id: String,
    pub revision: i64,
    pub name: String,
    pub content: String,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OfflineScopeRevision {
    pub identity_id: Uuid,
    pub identity_revision: i64,
    pub authorization_epoch: i64,
    pub membership_revision: i64,
    pub resource_revision: i64,
    pub content_grant_revision: i64,
    pub project_revision: i64,
    pub order_revision: i64,
    pub provider_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OfflineGrant {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<crate::StructuredOutput>,
    pub version: u32,
    pub id: Uuid,
    pub issuer_instance: Uuid,
    pub issuer_origin: String,
    pub issuer_public_key: String,
    pub device: OfflineDevice,
    pub team_id: Uuid,
    pub project_id: Uuid,
    pub order_id: Uuid,
    pub scope: OfflineScopeRevision,
    pub provider_base_url: String,
    pub provider_model: String,
    pub model: String,
    pub offline_base_url: String,
    pub prompt: String,
    pub sources: Vec<OfflineSource>,
    pub source_sha256: String,
    pub allowed_tools: Vec<String>,
    pub issued_at: i64,
    pub expires_at: i64,
    pub maximum_runs: u32,
    pub maximum_duration_seconds: u32,
    pub maximum_model_requests: u32,
    pub maximum_artifact_bytes: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SignedOfflineGrant {
    pub grant: OfflineGrant,
    pub signature: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OfflineGrantView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<crate::StructuredOutput>,
    pub id: Uuid,
    pub issuer_origin: String,
    pub issuer_public_key: String,
    pub team_id: Uuid,
    pub project_id: Uuid,
    pub order_id: Uuid,
    pub device: OfflineDevice,
    pub provider_base_url: String,
    pub provider_model: String,
    pub offline_base_url: String,
    pub model: String,
    pub expires_at: i64,
    pub maximum_runs: u32,
    pub maximum_duration_seconds: u32,
    pub allowed_tools: Vec<String>,
    pub source_count: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OfflineLocalGrantView {
    pub grant: OfflineGrantView,
    pub used_runs: u32,
    pub revoked: bool,
    pub key_configured: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueOfflineGrant {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<crate::StructuredOutput>,
    pub proof: OfflineDeviceProof,
    pub prompt: String,
    pub offline_base_url: String,
    pub offline_model: String,
    pub allowed_tools: Vec<String>,
    pub expires_in_seconds: u32,
    pub maximum_runs: u32,
    pub maximum_duration_seconds: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstallOfflineGrant {
    pub ticket: SignedOfflineGrant,
    /// Trusted native transfer must prove HTTPS origin or obtain native pin confirmation.
    pub confirmed_issuer_origin: String,
    pub confirmed_issuer_public_key: String,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigureOfflineCredential {
    pub grant_id: Uuid,
    pub api_key: Option<String>,
    pub explicitly_allow_no_key: bool,
}
/// Local device authority only. No team credential is transferred by this request.
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigureOfflineTools {
    pub grant_id: Uuid,
    pub allowed_tools: Vec<String>,
    pub programs: Vec<String>,
    pub github_token: Option<String>,
    pub mcp_connections: Vec<OfflineMcpConnectionInput>,
    pub explicitly_allow_native_owner_rights: bool,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OfflineMcpConnectionInput {
    pub id: String,
    pub endpoint: String,
    pub allowed_tools: Vec<String>,
    pub bearer: Option<String>,
    pub explicitly_allow_no_key: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OfflineMcpConnectionView {
    pub id: String,
    pub endpoint: String,
    pub allowed_tools: Vec<String>,
    pub catalog_sha256: String,
    pub definitions: Vec<Value>,
    pub credential_configured: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OfflineToolConfigurationView {
    pub grant_id: Uuid,
    pub configured: bool,
    pub allowed_tools: Vec<String>,
    pub programs: Vec<String>,
    pub available_programs: Vec<String>,
    pub browser_available: bool,
    pub office_available: bool,
    pub github_credential_configured: bool,
    pub mcp_connections: Vec<OfflineMcpConnectionView>,
    pub configuration_sha256: String,
    pub filesystem_isolation: bool,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OfflineWorkspaceUpload {
    pub path: String,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StartOfflineRun {
    pub grant_id: Uuid,
    pub idempotency_key: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OfflineJournalEntry {
    pub sequence: u32,
    pub at: i64,
    pub kind: String,
    pub payload: Value,
    pub previous_sha256: String,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OfflinePendingTool {
    pub id: String,
    pub name: String,
    pub arguments: Value,
    pub request_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OfflineArtifact {
    pub id: Uuid,
    pub name: String,
    pub media_type: String,
    pub content: String,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OfflineRun {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_contract_sha256: Option<String>,
    pub version: u32,
    pub id: Uuid,
    pub grant_id: Uuid,
    pub device_id: Uuid,
    pub ordinal: u32,
    pub state: String,
    pub revision: i64,
    pub started_at: i64,
    pub deadline_at: i64,
    pub finished_at: Option<i64>,
    pub response: Option<String>,
    pub error: Option<String>,
    pub model_requests: u32,
    pub pending_tools: Vec<OfflinePendingTool>,
    pub artifacts: Vec<OfflineArtifact>,
    pub journal: Vec<OfflineJournalEntry>,
    pub reconciled: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApproveOfflineTools {
    pub expected_revision: i64,
    pub request_sha256: Vec<String>,
    pub approve: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OfflineRunExport {
    pub ticket: SignedOfflineGrant,
    pub run: OfflineRun,
    pub device_signature: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OfflineGrantStatusRequest {
    pub grant_id: Uuid,
    pub nonce: Uuid,
    pub device_signature: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OfflineGrantStatus {
    pub grant_id: Uuid,
    pub nonce: Uuid,
    pub issuer_origin: String,
    pub checked_at: i64,
    pub allowed: bool,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SignedOfflineGrantStatus {
    pub status: OfflineGrantStatus,
    pub server_signature: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OfflineRunSummary {
    pub id: Uuid,
    pub grant_id: Uuid,
    pub state: String,
    pub revision: i64,
    pub ordinal: u32,
    pub started_at: i64,
    pub error: Option<String>,
    pub artifact_count: u32,
    pub journal_count: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OfflineRunPage {
    pub runs: Vec<OfflineRunSummary>,
    pub next_cursor: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OfflineReconciliationReceipt {
    pub id: Uuid,
    pub grant_id: Uuid,
    pub local_run_id: Uuid,
    pub team_id: Uuid,
    pub project_id: Uuid,
    pub order_id: Uuid,
    pub report_sha256: String,
    pub state: String,
    pub recorded_at: i64,
    pub response: Option<String>,
    pub artifacts: Vec<OfflineArtifact>,
}
