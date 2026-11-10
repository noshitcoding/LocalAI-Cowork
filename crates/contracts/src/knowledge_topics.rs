//! Topic hierarchy and reproducible, metadata-backed knowledge graphs.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeFolder {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeFolders {
    pub revision: i64,
    pub folders: Vec<KnowledgeFolder>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteKnowledgeFolders {
    pub expected_revision: i64,
    pub folders: Vec<KnowledgeFolder>,
}

/// Omitted on an edit = retain the current location; null folder = unfile.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeLocation {
    pub folder_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BuildKnowledgeGraph {
    pub folder_id: String,
    pub include_descendants: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeNodeKind {
    Topic,
    Document,
    Tag,
    Source,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeGraphNode {
    pub id: String,
    pub label: String,
    pub kind: KnowledgeNodeKind,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeGraphEdge {
    pub from: String,
    pub to: String,
    /// contains, tagged, or cites. These are structural links, not factual claims.
    pub relation: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeTopicGraph {
    pub folder_id: String,
    pub include_descendants: bool,
    pub fingerprint: String,
    pub built_at: i64,
    pub stale: bool,
    pub documents: usize,
    pub nodes: Vec<KnowledgeGraphNode>,
    pub edges: Vec<KnowledgeGraphEdge>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchKnowledgeGraph {
    pub folder_id: String,
    pub include_descendants: bool,
    pub query: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeGraphHit {
    pub document: KnowledgeDocument,
    pub score: usize,
    /// Direct match or the explicit tag/source node used to reach this document.
    pub via: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeGraphResults {
    pub fingerprint: String,
    pub hits: Vec<KnowledgeGraphHit>,
}
