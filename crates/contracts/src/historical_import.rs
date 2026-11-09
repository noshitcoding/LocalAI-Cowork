//! Canonical additive M9 product admission and immutable original receipts.
use serde::{Deserialize, Serialize};
use uuid::Uuid;
const MAX_QUARANTINE_BYTES: u64 = 12 * 1024 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HistoricalOwner {
    PersonalNative {
        installation_id: Uuid,
    },
    Organization {
        organization_id: Uuid,
        creator_id: Uuid,
        admission_project_id: Uuid,
    },
}
impl HistoricalOwner {
    pub fn locator(&self) -> String {
        match self {
            Self::PersonalNative { installation_id } => format!("native:{installation_id}"),
            Self::Organization {
                organization_id,
                creator_id,
                admission_project_id,
            } => format!("org:{organization_id}:{creator_id}:{admission_project_id}"),
        }
    }
}
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum HistoricalStream {
    QuarantineManifest,
    QuarantineDatabase,
    Mapping,
    Preview,
    ColdTargetSnapshot,
}
impl HistoricalStream {
    pub fn locator(self) -> &'static str {
        match self {
            Self::QuarantineManifest => "quarantine_manifest",
            Self::QuarantineDatabase => "quarantine_database",
            Self::Mapping => "mapping",
            Self::Preview => "preview",
            Self::ColdTargetSnapshot => "cold_target_snapshot",
        }
    }
    pub fn maximum(self) -> u64 {
        match self {
            Self::QuarantineManifest => 2 * 1024 * 1024,
            Self::QuarantineDatabase => MAX_QUARANTINE_BYTES,
            Self::Mapping => 2 * 1024 * 1024,
            Self::Preview => 256 * 1024 * 1024,
            Self::ColdTargetSnapshot => 512 * 1024 * 1024,
        }
    }
    pub fn all() -> [Self; 5] {
        [
            Self::QuarantineManifest,
            Self::QuarantineDatabase,
            Self::Mapping,
            Self::Preview,
            Self::ColdTargetSnapshot,
        ]
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalPinSet {
    pub quarantine_manifest: String,
    pub quarantine_database: String,
    pub export_manifest: String,
    pub mapping: String,
    pub preview: String,
    pub cold_target_snapshot: String,
}
impl HistoricalPinSet {
    pub fn stream(&self, s: HistoricalStream) -> &str {
        match s {
            HistoricalStream::QuarantineManifest => &self.quarantine_manifest,
            HistoricalStream::QuarantineDatabase => &self.quarantine_database,
            HistoricalStream::Mapping => &self.mapping,
            HistoricalStream::Preview => &self.preview,
            HistoricalStream::ColdTargetSnapshot => &self.cold_target_snapshot,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalFileDeclaration {
    pub stream: HistoricalStream,
    pub size: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BeginHistoricalImport {
    pub request_id: Uuid,
    pub expected_limit_revision: i64,
    pub pins: HistoricalPinSet,
    pub files: Vec<HistoricalFileDeclaration>,
    /// Absent means the original /1 intake and its original byte bounds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_format: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalImportLimit {
    pub installation_id: Uuid,
    pub revision: i64,
    pub limit_bytes: u64,
    pub reserved_bytes: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalImportRecord {
    pub id: Uuid,
    pub owner: HistoricalOwner,
    pub request_id: Uuid,
    pub request_sha256: String,
    pub state: String,
    pub revision: i64,
    pub declared_bytes: u64,
    pub received_bytes: u64,
    pub reserved_bytes: u64,
    pub files: Vec<HistoricalFileDeclaration>,
    pub pins: HistoricalPinSet,
    pub source_profile: Option<String>,
    pub source_family: Option<String>,
    pub original_rows: Option<u64>,
    pub verifier_contract: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_format: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalChunkReceipt {
    pub import_id: Uuid,
    pub stream: HistoricalStream,
    pub chunk_index: u32,
    pub size: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalContentProject {
    pub source_id: String,
    pub name: String,
    pub orders: Vec<HistoricalContentOrder>,
    pub documents: Vec<HistoricalContentDocument>,
    pub files: Vec<HistoricalContentFile>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalContentPage {
    pub stream: String,
    pub chunk_index: u32,
    pub size: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalContentCatalogEntry {
    pub source: HistoricalContentProject,
    pub pages: Vec<HistoricalContentPage>,
    pub content_bytes: u64,
    pub unsupported_reason: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalContentOrder {
    pub source_id: String,
    pub source_kind: String,
    pub title: String,
    pub draft: String,
    pub messages: Vec<HistoricalContentMessage>,
    pub artifacts: Vec<HistoricalContentArtifact>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalContentDocument {
    pub source_id: String,
    pub title: String,
    pub content: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalContentFile {
    pub source_id: String,
    pub name: String,
    pub media_type: String,
    pub size: Option<u64>,
    pub sha256: Option<String>,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HistoricalProjectTarget {
    HistoryOnly {
        reason: String,
    },
    NewPersonal,
    ExistingPersonal {
        project_id: String,
        expected_revision: i64,
    },
    ExistingOrganization {
        project_id: Uuid,
        expected_revision: i64,
        expected_profile_revision: i64,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalProjectMapping {
    pub source_project_id: String,
    pub target: HistoricalProjectTarget,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewHistoricalImport {
    pub request_id: Uuid,
    pub expected_import_revision: i64,
    pub expected_limit_revision: i64,
    pub projects: Vec<HistoricalProjectMapping>,
    /// Every missing physical source must be explicitly retained as history or supplied through native staging.
    pub history_only_file_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remainder_history_only_reason: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalActivatedObject {
    pub source_id: String,
    pub kind: String,
    pub id: String,
    pub project_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalPreparedProject {
    pub source: HistoricalContentProject,
    pub target: HistoricalProjectTarget,
    pub project_id: String,
    pub order_ids: Vec<String>,
    pub document_ids: Vec<String>,
    pub file_ids: Vec<String>,
    pub content_pages: Vec<HistoricalContentPage>,
    pub content_bytes: u64,
    pub unsupported_reason: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalImportPreview {
    pub id: Uuid,
    pub import_id: Uuid,
    pub owner: HistoricalOwner,
    pub request_id: Uuid,
    pub request_sha256: String,
    pub plan_sha256: String,
    pub source_sha256: String,
    pub source_profile: String,
    pub original_rows: u64,
    pub limit: HistoricalImportLimit,
    pub projected_bytes: u64,
    pub projects: Vec<HistoricalPreparedProject>,
    pub history_only_file_ids: Vec<String>,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remainder_history_only_reason: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActivateHistoricalImport {
    pub plan_sha256: String,
    pub expected_limit_revision: i64,
    pub explicit_activation: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalActivationReceipt {
    pub preview_id: Uuid,
    pub import_id: Uuid,
    pub plan_sha256: String,
    pub source_sha256: String,
    pub owner: HistoricalOwner,
    pub activated_at: i64,
    pub objects: Vec<HistoricalActivatedObject>,
    pub charged_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remainder_history_only_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remainder_parts: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_sha256: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalContentCatalogManifest {
    pub format: String,
    pub count: u32,
    pub sha256: String,
    pub source_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalContentCatalogItem {
    pub ordinal: u32,
    pub source_id: String,
    pub name: String,
    pub orders: u32,
    pub documents: u32,
    pub files: u32,
    pub content_bytes: u64,
    pub unsupported_reason: Option<String>,
    pub metadata_pages: Vec<HistoricalContentPage>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalContentCatalogPage {
    pub manifest: HistoricalContentCatalogManifest,
    pub offset: u32,
    pub next_offset: Option<u32>,
    pub entries: Vec<HistoricalContentCatalogItem>,
    /// Current validated ledger status, separate from immutable catalogue bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activated_parts: Option<std::collections::BTreeMap<String, String>>,
    /// A completed legacy import cannot acquire new editable mappings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editable_selection_closed: Option<bool>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalPreviewProjectSummary {
    pub ordinal: u32,
    pub source_id: String,
    pub name: String,
    pub project_id: String,
    pub target: HistoricalProjectTarget,
    pub pages: Vec<HistoricalHistoryPagePin>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalPreviewSummary {
    pub format: String,
    pub id: Uuid,
    pub import_id: Uuid,
    pub plan_sha256: String,
    pub state: String,
    pub source_profile: String,
    pub original_rows: u64,
    pub projected_bytes: u64,
    pub limit: HistoricalImportLimit,
    pub history_only_file_ids: Vec<String>,
    pub catalog_sha256: Option<String>,
    pub catalog_count: Option<u32>,
    pub remainder_history_only_reason: Option<String>,
    pub projects: Vec<HistoricalPreviewProjectSummary>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalFileStageRequest {
    pub source_id: String,
    pub backend_id: String,
    pub size: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalFileOperation {
    pub preview_id: Uuid,
    pub import_id: Uuid,
    pub plan_sha256: String,
    pub owner: HistoricalOwner,
    pub source_id: String,
    pub target_id: String,
    pub project_id: String,
    pub order_id: String,
    pub name: String,
    pub media_type: String,
    pub backend_id: String,
    pub size: u64,
    pub sha256: String,
    pub request_sha256: String,
    pub state: String,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalContentMessage {
    pub id: String,
    pub source_id: Option<String>,
    pub role: String,
    pub content: String,
    pub created_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalContentArtifact {
    pub id: String,
    pub source_id: Option<String>,
    pub name: String,
    pub media_type: String,
    pub size: u64,
    pub sha256: String,
    pub content: String,
    pub created_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigureHistoricalImportLimit {
    pub expected_revision: i64,
    pub limit_bytes: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalImportTarget {
    pub id: String,
    pub name: String,
    pub revision: i64,
    pub profile_revision: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CloseHistoricalUnsentFile {
    pub expected_request_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalUnsentFileReceipt {
    pub format: String,
    pub original_operation: HistoricalFileOperation,
    pub released_bytes: u64,
    pub retained_reserved_bytes: u64,
    pub original_charge_id: Option<Uuid>,
    pub closed_at: i64,
    pub closure_sha256: String,
}

/// Explicitly derives a read-only index; no historical authority is activated.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BuildHistoricalHistoryIndex {
    pub request_id: Uuid,
    pub expected_import_revision: i64,
    pub expected_limit_revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalHistoryPagePin {
    pub size: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalHistoryIndex {
    pub format: String,
    pub import_id: Uuid,
    pub owner: HistoricalOwner,
    pub request_id: Uuid,
    pub request_sha256: String,
    pub original_record_sha256: String,
    pub original_intake_sha256: String,
    pub pins: HistoricalPinSet,
    pub source_profile: String,
    pub source_family: String,
    pub verifier_contract: String,
    pub tables: Vec<HistoricalHistoryPagePin>,
    pub rows: u64,
    pub cells: u64,
    pub original_value_bytes: u64,
    pub index_sha256: String,
    pub reserved_bytes: u64,
    pub built_at: i64,
    pub receipt_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalHistoryTable {
    pub ordinal: u32,
    pub name: String,
    pub columns: Vec<String>,
    pub rows: u64,
    pub canonical_bytes: u64,
    pub canonical_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalHistoryRow {
    pub position: u64,
    /// Decimal text preserves every original signed 64-bit rowid in JavaScript.
    pub rowid: String,
    pub original_sha256: String,
    pub columns: u32,
    pub cell_pages: Vec<HistoricalHistoryPagePin>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HistoricalHistoryCell {
    Null,
    Integer {
        decimal: String,
    },
    RealBits {
        ieee754_binary64_be: String,
    },
    TextBytes {
        size: u64,
        sha256: String,
        pages: Vec<HistoricalHistoryPagePin>,
        preview_base64: String,
    },
    Blob {
        size: u64,
        sha256: String,
        pages: Vec<HistoricalHistoryPagePin>,
        preview_base64: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalHistoryColumn {
    pub column_index: u32,
    pub value: HistoricalHistoryCell,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalHistoryCellsPage {
    pub table_ordinal: u32,
    pub position: u64,
    pub rowid: String,
    pub page_index: u32,
    pub cells: Vec<HistoricalHistoryColumn>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalHistoryTablesPage {
    pub index_sha256: String,
    pub offset: u32,
    pub next_offset: Option<u32>,
    pub tables: Vec<HistoricalHistoryTable>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoricalHistoryRowsPage {
    pub index_sha256: String,
    pub table_ordinal: u32,
    pub offset: u64,
    pub next_offset: Option<u64>,
    pub rows: Vec<HistoricalHistoryRow>,
}
