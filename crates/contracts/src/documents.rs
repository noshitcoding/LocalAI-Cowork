use crate::{AccessMode, ToolCall};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A model-free document action. The server freezes all source hashes before review.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentActionRequest {
    pub version: u32,
    pub base_revision: i64,
    pub idempotency_key: String,
    pub access_mode: AccessMode,
    pub tool: ToolCall,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentPreparation {
    pub id: String,
    pub order_id: String,
    pub base_revision: i64,
    pub idempotency_key: String,
    pub access_mode: AccessMode,
    pub tool: ToolCall,
    pub fingerprint: String,
    pub engine_revision: String,
    pub requires_approval: bool,
    pub created_at: i64,
    pub run_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveDocumentDraft {
    pub id: String,
    pub base_revision: i64,
    pub idempotency_key: String,
    pub content: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct DocumentDraft {
    pub id: String,
    pub order_id: String,
    pub revision: i64,
    pub sha256: String,
    pub content: serde_json::Value,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct DocumentDraftSnapshot {
    pub id: String,
    pub draft_id: String,
    pub order_id: String,
    pub base_revision: i64,
    pub sha256: String,
    pub content: serde_json::Value,
    pub conflict: bool,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct DocumentDraftSave {
    pub saved: bool,
    pub draft: DocumentDraft,
    pub snapshot: DocumentDraftSnapshot,
}

/// One SHA-verified input copy that a document engine read.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DocumentProvenanceInput {
    pub folder_id: String,
    #[schemars(length(min = 1, max = 4096))]
    pub path: String,
    #[schemars(regex(pattern = "^[0-9a-f]{64}$"))]
    pub sha256: String,
}

/// Which built-in document tool produced a generated file, from which exact inputs.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DocumentProvenance {
    #[schemars(length(min = 1, max = 64))]
    pub tool: String,
    #[schemars(length(min = 1, max = 64))]
    pub operation: String,
    #[schemars(length(min = 1, max = 128))]
    pub engine_version: String,
    #[schemars(length(max = 16))]
    pub inputs: Vec<DocumentProvenanceInput>,
    /// SHA-256 of the canonical engine receipt that listed the published bytes.
    #[schemars(regex(pattern = "^[0-9a-f]{64}$"))]
    pub receipt_sha256: String,
}
