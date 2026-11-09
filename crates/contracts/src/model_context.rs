use crate::*;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelRegistryRef {
    pub id: String,
    pub version: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportModelQualification {
    pub idempotency_key: String,
    pub qualification_bundle: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ModelRegistryView {
    pub binding: ModelRegistryRef,
    pub provider_id: String,
    pub provider_revision: i64,
    pub provider_sha256: String,
    pub deployment_sha256: String,
    pub model_sha256: String,
    pub tokenizer_sha256: String,
    pub chat_template_sha256: String,
    pub evidence_sha256: String,
    pub context_tokens: u64,
    pub wire_mode: String,
    pub reference_implementation: String,
    pub registered_at: i64,
    pub revoked: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ContextGroupView {
    pub id: String,
    pub sha256: String,
    pub required: bool,
    pub message_count: u32,
    pub characters: u64,
    pub preview: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewModelContext {
    pub request: CreateRun,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompactModelContext {
    pub request: CreateRun,
    pub idempotency_key: String,
    pub snapshot_sha256: String,
    pub remove_groups: Vec<String>,
    pub summary: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ModelContextView {
    pub order_id: String,
    pub order_revision: i64,
    pub snapshot_sha256: String,
    pub groups: Vec<ContextGroupView>,
    pub count_mode: String,
    pub input_tokens: Option<u64>,
    pub conservative_bytes: Option<u64>,
    pub output_allowance: u64,
    pub context_limit: u64,
    pub fits: bool,
    pub model_binding: Option<ModelRegistryRef>,
    pub selection_revision: u64,
    pub archive_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ContextCompactionView {
    pub id: String,
    pub order_id: String,
    pub revision: u64,
    pub created_at: i64,
    pub original_sha256: String,
    pub retained_sha256: String,
    pub removed_groups: Vec<String>,
    pub summary: Option<String>,
    pub count_mode: String,
    pub input_tokens: Option<u64>,
    pub model_binding: Option<ModelRegistryRef>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ContextArchiveView {
    pub receipt: ContextCompactionView,
    pub original: Value,
    pub retained: Value,
}
