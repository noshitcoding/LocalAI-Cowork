//! Team contracts contain no owner-native authority or returned credentials.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TeamRole {
    Owner,
    Admin,
    Member,
    Viewer,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Team {
    pub id: Uuid,
    pub name: String,
    pub state: String,
    pub revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateTeam {
    pub name: String,
    pub idempotency_key: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateTeam {
    pub expected_revision: i64,
    pub name: String,
    #[serde(default)]
    pub closed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamMember {
    pub id: Uuid,
    pub team_id: Uuid,
    pub identity_id: Uuid,
    pub role: TeamRole,
    pub active: bool,
    pub revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddTeamMember {
    pub identity_id: Uuid,
    pub role: TeamRole,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateTeamMember {
    pub expected_revision: i64,
    pub role: TeamRole,
    pub active: bool,
}
/// Input-only. Deliberately no Debug implementation and never returned by GET.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigureTeamProvider {
    pub expected_revision: i64,
    pub base_url: String,
    pub model: String,
    pub api_key: String,
}

/// Metadata only. Credentials are accepted solely by the write DTOs below.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamMcpServer {
    pub id: Uuid,
    pub team_id: Uuid,
    pub name: String,
    pub endpoint: String,
    pub revision: i64,
    pub enabled: bool,
    pub credential_configured: bool,
    pub catalog_sha256: Option<String>,
    pub tool_count: usize,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigureTeamMcpServer {
    #[serde(default)]
    pub id: Option<Uuid>,
    pub expected_revision: i64,
    pub name: String,
    pub endpoint: String,
    /// None preserves existing auth; an empty string explicitly removes it.
    #[serde(default)]
    pub bearer: Option<String>,
    pub enabled: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamMcpTool {
    pub name: String,
    pub remote_name: String,
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamGithubConfiguration {
    pub team_id: Uuid,
    pub revision: i64,
    pub hostname: String,
    pub enabled: bool,
    pub key_configured: bool,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigureTeamGithub {
    pub expected_revision: i64,
    pub hostname: String,
    /// An empty token retains an existing credential, including while disabled.
    pub token: String,
    pub enabled: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamProvider {
    pub team_id: Uuid,
    pub base_url: String,
    pub model: String,
    pub revision: i64,
    pub key_configured: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamProject {
    pub id: Uuid,
    pub team_id: Uuid,
    pub name: String,
    pub revision: i64,
    pub archived: bool,
    pub created_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateTeamProject {
    pub name: String,
    pub idempotency_key: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateTeamProject {
    pub name: String,
    pub expected_revision: i64,
    pub archived: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamProjectGrant {
    pub membership_id: Uuid,
    pub expected_revision: i64,
    pub read: bool,
    pub write: bool,
    pub execute: bool,
    pub share: bool,
    pub active: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamProjectGrantView {
    pub membership_id: Uuid,
    pub revision: i64,
    pub read: bool,
    pub write: bool,
    pub execute: bool,
    pub share: bool,
    pub active: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamOrder {
    pub id: Uuid,
    pub team_id: Uuid,
    pub project_id: Uuid,
    pub title: String,
    pub prompt: String,
    pub revision: i64,
    pub archived: bool,
    pub created_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateTeamOrder {
    pub title: String,
    pub prompt: String,
    pub idempotency_key: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateTeamOrder {
    pub title: String,
    pub prompt: String,
    pub expected_revision: i64,
    pub archived: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TeamDocumentKind {
    Skill,
    Knowledge,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamDocument {
    #[serde(
        default,
        skip_serializing_if = "crate::KnowledgeProvenance::is_legacy_unknown"
    )]
    pub provenance: crate::KnowledgeProvenance,
    pub id: Uuid,
    pub team_id: Uuid,
    pub project_id: Uuid,
    pub kind: TeamDocumentKind,
    pub name: String,
    pub content: String,
    pub revision: i64,
    pub archived: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveTeamDocument {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<crate::KnowledgeProvenance>,
    pub id: Option<Uuid>,
    pub expected_revision: i64,
    pub kind: TeamDocumentKind,
    pub name: String,
    pub content: String,
    #[serde(default)]
    pub archived: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TeamWorkflowMode {
    Sequential,
    Parallel,
    Hierarchical,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamAgent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<crate::StructuredOutput>,
    pub id: String,
    pub instructions: String,
    #[serde(default)]
    pub skills: Vec<Uuid>,
    #[serde(default)]
    pub knowledge: Vec<Uuid>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamWorkflowStep {
    pub id: String,
    pub agent_id: String,
    pub prompt: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamWorkflowPlan {
    pub mode: TeamWorkflowMode,
    pub agents: Vec<TeamAgent>,
    pub steps: Vec<TeamWorkflowStep>,
    #[serde(default)]
    pub manager_agent_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamRunStep {
    pub id: String,
    pub state: String,
    pub response: Option<String>,
    pub error: Option<String>,
    pub updated_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct TeamRunPlan {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<crate::StructuredOutput>,
    #[serde(default)]
    pub workflow: Option<TeamWorkflowPlan>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub skills: Vec<Uuid>,
    #[serde(default)]
    pub knowledge: Vec<Uuid>,
    /// Also supply the account-wide memories (personal knowledge base) of the person
    /// starting the run, frozen at admission. Requires the administrator's separate
    /// approval because results the team reads may then contain private content.
    /// Omitted when false, so request hashes of earlier runs stay unchanged.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub personal_knowledge: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateTeamRun {
    pub order_id: Uuid,
    pub prompt: String,
    pub idempotency_key: String,
    #[serde(default)]
    pub plan: TeamRunPlan,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TeamRunStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Unknown,
    Cancelled,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamRun {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<crate::StructuredOutput>,
    pub id: Uuid,
    pub team_id: Uuid,
    pub project_id: Uuid,
    pub order_id: Uuid,
    pub status: TeamRunStatus,
    pub provider_revision: i64,
    pub model: String,
    pub response: Option<String>,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamToolApproval {
    pub id: Uuid,
    pub team_id: Uuid,
    pub project_id: Uuid,
    pub order_id: Uuid,
    pub run_id: Uuid,
    pub tool: String,
    pub action: serde_json::Value,
    pub request_sha256: String,
    pub state: String,
    pub job_id: Option<String>,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub expires_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApproveTeamTool {
    pub request_sha256: String,
    pub approved: bool,
}
