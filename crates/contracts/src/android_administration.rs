//! Administrative wire is data about actual current V2 identities, never a grant.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidProjectMemberWrite {
    pub user_id: Uuid,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidProjectMemberView {
    pub schema_version: i64,
    pub project_id: Uuid,
    pub user_id: Uuid,
    pub membership_id: Uuid,
    pub display_name: String,
    pub role: String,
    pub permissions: Vec<String>,
    pub active: bool,
    pub revision: i64,
    pub membership_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidProjectMemberPage {
    pub items: Vec<AndroidProjectMemberView>,
    pub next_cursor: Option<Uuid>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AndroidContentAccess {
    pub schema_version: i64,
    pub actor_user_id: Uuid,
    pub organisation_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub managed: bool,
    pub instance_viewer: bool,
    pub local_membership_allowed: bool,
    pub automatic_content_access: bool,
    pub support_access: bool,
    pub permissions: Vec<String>,
}
