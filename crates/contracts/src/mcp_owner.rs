//! Explicit owner/native MCP configuration. Actor sessions retain their scoped adapters.
use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigureOwnerMcpStdio {
    pub project_id: String,
    pub server_id: String,
    pub program: String,
    pub arguments: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// Write-only values. Omission preserves encrypted variables; an empty map clears them.
    #[serde(default)]
    pub environment: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub expected_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeMcpFactory {
    pub server_id: String,
    pub project_id: String,
    pub program: String,
    pub arguments: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// Variable names and encrypted credential revisions only. Never plaintext values.
    #[serde(default)]
    pub environment: BTreeMap<String, i64>,
    #[serde(default = "native_default")]
    pub native_only: bool,
    pub endpoint: String,
    pub credential_reference: String,
    pub credential_revision: i64,
    pub revision: i64,
    pub enabled: bool,
}
fn native_default() -> bool {
    true
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigureOwnerMcpCredential {
    pub project_id: String,
    pub server_id: String,
    pub endpoint: String,
    pub bearer: String,
    pub expected_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OwnerMcpCredential {
    pub credential_reference: String,
    pub credential_revision: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OwnerMcpDiscovery {
    pub project_id: String,
    pub server_id: String,
    pub endpoint: String,
    #[serde(default)]
    pub credential_reference: Option<String>,
    #[serde(default)]
    pub credential_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OwnerMcpCatalog {
    pub catalog: Vec<Value>,
    pub catalog_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_revision: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub factory_revision: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OwnerMcpCapabilities {
    pub http: bool,
    pub stdio: bool,
    pub programs: Vec<String>,
    pub stdio_cwd_policy: OwnerMcpCwdPolicy,
    pub environment: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OwnerMcpCwdPolicy {
    ManagedRelative,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OwnerMcpRevision {
    pub revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OwnerMcpExpectedRevision {
    pub expected_revision: i64,
}
