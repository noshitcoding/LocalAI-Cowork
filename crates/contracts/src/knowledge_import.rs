//! Knowledge import and manual editing share one public document format.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeKind {
    Knowledge,
    Memory,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemoryScope {
    User,
    #[default]
    Project,
    Chat {
        order_id: String,
    },
    Agent {
        agent_id: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct MemoryMetadata {
    pub scope: MemoryScope,
    pub category: String,
    pub confidence: Option<f64>,
    pub profile_key: Option<String>,
    pub enabled: bool,
}
impl Default for MemoryMetadata {
    fn default() -> Self {
        Self {
            scope: MemoryScope::Project,
            category: "general".into(),
            confidence: None,
            profile_key: None,
            enabled: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeDocument {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryMetadata>,
    #[serde(
        default,
        skip_serializing_if = "KnowledgeProvenance::is_legacy_unknown"
    )]
    pub provenance: KnowledgeProvenance,
    pub id: String,
    pub project_id: String,
    pub kind: KnowledgeKind,
    pub title: String,
    pub content: String,
    pub tags: Vec<String>,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteKnowledge {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<KnowledgeLocation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<KnowledgeProvenance>,
    pub id: Option<String>,
    pub expected_revision: i64,
    pub kind: KnowledgeKind,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeImportRequest {
    pub idempotency_key: uuid::Uuid,
    pub documents: Vec<WriteKnowledge>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeImportResult {
    pub project_id: String,
    pub documents: Vec<KnowledgeDocument>,
}
