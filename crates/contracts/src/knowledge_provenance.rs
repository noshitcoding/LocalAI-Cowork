//! Content provenance is context, never an execution or file-access grant.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeOrigin {
    UserAuthored,
    Imported,
    Retrieved,
    #[default]
    LegacyUnknown,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeSourceVerification {
    #[default]
    Declared,
    SnapshotVerified,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeSource {
    pub format: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
    pub revision: String,
    pub sha256: String,
    pub locator: String,
    #[serde(default)]
    pub verification: KnowledgeSourceVerification,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeProvenance {
    pub format: u32,
    pub origin: KnowledgeOrigin,
    pub sources: Vec<KnowledgeSource>,
}
impl Default for KnowledgeProvenance {
    fn default() -> Self {
        Self {
            format: 1,
            origin: KnowledgeOrigin::LegacyUnknown,
            sources: vec![],
        }
    }
}
impl KnowledgeProvenance {
    pub fn is_legacy_unknown(&self) -> bool {
        self == &Self::default()
    }
    pub fn user_authored() -> Self {
        Self {
            origin: KnowledgeOrigin::UserAuthored,
            ..Self::default()
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeRevision {
    pub document_id: String,
    pub revision: i64,
    pub content_sha256: String,
    pub provenance_sha256: String,
    pub provenance: KnowledgeProvenance,
    pub document: Value,
    pub recorded_at: i64,
}
