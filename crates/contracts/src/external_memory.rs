//! Explicit HTTP memory retrieval; retrieved text never grants runtime authority.
use super::*;
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExternalMemoryDefinition {
    pub name: String,
    pub endpoint: String,
    pub method: String,
    pub request_body: Option<Value>,
    pub items_pointer: String,
    pub content_field: String,
    pub id_field: Option<String>,
    pub auth_scheme: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExternalMemoryProvider {
    pub id: String,
    pub revision: i64,
    pub value: ExternalMemoryDefinition,
    pub has_secret: bool,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteExternalMemoryProvider {
    pub request_id: uuid::Uuid,
    pub expected_revision: i64,
    pub value: ExternalMemoryDefinition,
    pub api_key: Option<String>,
    #[serde(default)]
    pub remove_key: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RetrieveExternalMemory {
    pub request_id: uuid::Uuid,
    pub expected_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteExternalMemoryProvider {
    pub request_id: uuid::Uuid,
    pub expected_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExternalMemoryItem {
    pub id: String,
    pub content: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExternalMemoryRetrieval {
    pub request_id: uuid::Uuid,
    pub provider_id: String,
    pub provider_revision: i64,
    pub endpoint: String,
    pub response_sha256: String,
    pub retrieved_at: i64,
    pub items: Vec<ExternalMemoryItem>,
}
