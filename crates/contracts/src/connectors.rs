use super::*;

/// Secret-free connector metadata. Destination paths and bearer material are write-only.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OwnerConnector {
    pub id: String,
    pub project_id: String,
    pub revision: i64,
    pub name: String,
    pub note: String,
    pub destination: String,
    pub enabled: bool,
    pub deleted: bool,
    pub native_only: bool,
    pub server_id: String,
    pub has_bearer: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveOwnerConnector {
    pub id: String,
    pub idempotency_key: String,
    pub expected_revision: i64,
    pub name: String,
    pub note: String,
    pub enabled: bool,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub bearer: Option<String>,
    #[serde(default)]
    pub remove_bearer: bool,
    /// Recover a previously staged request without resupplying secrets.
    #[serde(default)]
    pub recover_only: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteOwnerConnector {
    pub idempotency_key: String,
    pub expected_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct OwnerConnectorProbe {
    pub destination: String,
    pub status: u16,
    pub reachable: bool,
}
