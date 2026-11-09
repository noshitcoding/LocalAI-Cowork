//! Current personal access has an original content owner, not an organization membership.
use super::*;
use uuid::Uuid;
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidPrivateProjectMemberView {
    pub project_id: Uuid,
    pub source_owner_id: Uuid,
    pub user_id: Uuid,
    pub display_name: String,
    pub role: String,
    pub permissions: Vec<String>,
    pub revision: i64,
    pub active: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidPrivateProjectMemberPage {
    pub schema_version: u16,
    pub items: Vec<AndroidPrivateProjectMemberView>,
    pub next_cursor: Option<Uuid>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidPrivateProjectMemberReceipt {
    pub schema_version: u16,
    pub project_id: Uuid,
    pub source_owner_id: Uuid,
    pub user_id: Uuid,
    pub permissions: Vec<String>,
    pub revision: i64,
    pub active: bool,
    pub request_sha256: String,
}
