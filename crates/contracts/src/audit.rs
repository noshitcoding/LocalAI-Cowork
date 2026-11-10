//! Redacted audit data. Imported history is inert and never grants authority.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuditOrigin {
    NativeV2,
    LegacyUnsigned,
    PreservedArchive,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuditRecord {
    pub sequence: i64,
    pub event_id: String,
    pub action: String,
    pub actor_id: Option<String>,
    pub target_id: Option<String>,
    pub observed_at: i64,
    pub origin: AuditOrigin,
    pub source_sha256: Option<String>,
    pub previous_sha256: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditPage {
    pub format: String,
    pub scope: String,
    pub chain_id: Uuid,
    pub after_sequence: i64,
    pub previous_sha256: String,
    pub through_sequence: i64,
    pub through_sha256: String,
    pub events: Vec<AuditRecord>,
    pub next_sequence: i64,
    pub complete: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SignedAuditArchive {
    pub format: String,
    pub request_id: Uuid,
    pub captured_at: i64,
    pub public_key: String,
    pub public_key_sha256: String,
    pub page: AuditPage,
    pub signature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditArchiveBundle {
    pub format: String,
    pub pages: Vec<SignedAuditArchive>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CaptureAuditArchive {
    pub request_id: Uuid,
    #[serde(default)]
    pub after_sequence: i64,
    pub through_sequence: Option<i64>,
    pub limit: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreserveAuditHistory {
    pub request_id: Uuid,
    pub bytes_base64: String,
    /// Required for a V2 archive: obtain this pin independently from its origin.
    pub expected_public_key_sha256: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreservedAuditHistory {
    pub request_id: Uuid,
    pub content_sha256: String,
    pub bytes: i64,
    pub trust: String,
    pub original_chain_id: Option<Uuid>,
    pub public_key_sha256: Option<String>,
    pub anchor_sequence: i64,
    pub anchor_chain_id: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditVerification {
    pub scope: String,
    pub chain_id: Uuid,
    pub valid: bool,
    pub complete: bool,
    pub checked_events: i64,
    pub head_sequence: i64,
    pub head_sha256: String,
    pub first_problem_sequence: Option<i64>,
    pub legacy_unsigned_events: i64,
    pub preserved_archives: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditCount {
    pub category: String,
    pub count: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditInsights {
    pub scope: String,
    pub total_events: i64,
    pub projects: Option<i64>,
    pub orders: Option<i64>,
    pub runs: Option<i64>,
    pub messages: Option<i64>,
    pub knowledge_documents: Option<i64>,
    pub skills: Option<i64>,
    pub categories: Vec<AuditCount>,
    pub run_states: Vec<AuditCount>,
    pub recent_events: Vec<AuditRecord>,
    pub verification: AuditVerification,
    pub public_key_sha256: Option<String>,
    pub execution: Option<AuditExecutionInsights>,
    #[serde(default)]
    pub retired_chains: Vec<AuditRecoveryReceipt>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditInsightsQuery {
    pub from_ms: Option<i64>,
    pub until_ms: Option<i64>,
    #[serde(default)]
    pub after_sequence: i64,
    pub snapshot_sequence: Option<i64>,
    #[serde(default = "insights_limit")]
    pub limit: i64,
}
fn insights_limit() -> i64 {
    100
}
impl Default for AuditInsightsQuery {
    fn default() -> Self {
        Self {
            from_ms: None,
            until_ms: None,
            after_sequence: 0,
            snapshot_sequence: None,
            limit: 100,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuditSkillUse {
    pub id: String,
    pub revision: i64,
    pub instructions_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuditCost {
    /// Integer amount in 10^-scale units. Never a binary float or an invoice claim.
    pub units: String,
    pub scale: u8,
    pub currency: Option<String>,
    /// provider_reported or price_calculated, never an estimated reservation.
    pub basis: String,
    pub price_source_sha256: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuditMetricFact {
    pub sequence: i64,
    pub id: String,
    pub scope: String,
    pub run_id: String,
    pub kind: String,
    pub observed_at: i64,
    pub time_basis: String,
    pub source_kind: String,
    pub source_sha256: String,
    pub request_sha256: Option<String>,
    pub input_tokens: Option<String>,
    pub output_tokens: Option<String>,
    pub usage_complete: bool,
    pub duration_ms: Option<String>,
    pub costs: Vec<AuditCost>,
    pub skills: Vec<AuditSkillUse>,
    pub skill_provenance_complete: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditExecutionTotals {
    pub model_receipts: String,
    pub input_tokens: String,
    pub output_tokens: String,
    pub incomplete_usage_receipts: String,
    pub run_receipts: String,
    pub duration_ms: String,
    pub unknown_duration_runs: String,
    pub skill_context_uses: String,
    pub unknown_skill_model_receipts: String,
    pub model_duration_ms: String,
    pub unknown_model_duration_receipts: String,
    pub non_execution_timestamp_receipts: String,
    pub costs: Vec<AuditCost>,
    pub missing_cost_model_receipts: String,
    pub facts: String,
    pub complete: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditExecutionInsights {
    pub from_ms: i64,
    pub until_ms: i64,
    pub comparison_from_ms: i64,
    pub snapshot_sequence: i64,
    pub current: AuditExecutionTotals,
    pub previous: AuditExecutionTotals,
    pub facts: Vec<AuditMetricFact>,
    pub next_sequence: i64,
    pub has_more: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditDiagnostics {
    pub format: String,
    pub captured_at: i64,
    pub insights: AuditInsights,
    pub redaction: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditOriginalReference {
    pub request_id: Uuid,
    pub content_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VerifyAuditV1 {
    pub request_id: Uuid,
    pub archives: Vec<AuditOriginalReference>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditV1Assessment {
    pub valid: bool,
    pub signed_events: i64,
    pub legacy_events: i64,
    pub first_sequence: Option<String>,
    pub last_sequence: Option<String>,
    pub chain_complete: bool,
    pub first_problem_line: Option<i64>,
    pub error_code: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditV1Verification {
    pub format: String,
    pub request_id: Uuid,
    pub scope: String,
    pub captured_at: i64,
    pub archives: Vec<AuditOriginalReference>,
    pub assessment: AuditV1Assessment,
    pub key_source: String,
    pub public_key: String,
    pub public_key_sha256: String,
    pub signature: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditRecoveryPlan {
    pub scope: String,
    pub snapshot_sha256: String,
    pub prior_chain_id: Option<Uuid>,
    pub prior_sequence: String,
    pub records: i64,
    pub bytes: i64,
    pub corrupt: bool,
    pub verification_complete: bool,
    pub public_key_sha256: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfirmAuditRecovery {
    pub request_id: Uuid,
    pub snapshot_sha256: String,
    pub preserve_original_and_start_new_chain: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditRecoveryReceipt {
    pub format: String,
    pub request_id: Uuid,
    pub scope: String,
    pub captured_at: i64,
    pub original: AuditRecoveryPlan,
    pub new_chain_id: Uuid,
    pub actor_id: Option<Uuid>,
    pub public_key: String,
    pub public_key_sha256: String,
    pub signature: String,
}

/// Explicitly confirmed independent trust. The public pin never comes from a
/// checkpoint file itself. A replacement requires an original signed handover.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigureAuditWitness {
    pub request_id: Uuid,
    pub origin_id: Uuid,
    pub origin_public_key_sha256: String,
    pub witness_public_key_sha256: String,
    pub independently_confirmed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditWitnessBinding {
    pub format: String,
    pub binding_id: Uuid,
    pub origin_id: Uuid,
    pub scope: String,
    pub origin_public_key_sha256: String,
    pub witness_public_key_sha256: String,
    pub configured_at: i64,
    pub actor_id: Option<Uuid>,
    pub public_key: String,
    pub signature: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CaptureAuditCheckpoint {
    pub request_id: Uuid,
    pub binding_id: Uuid,
    pub previous_checkpoint_sha256: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditCheckpointRequest {
    pub format: String,
    pub request_id: Uuid,
    pub binding: AuditWitnessBinding,
    pub captured_at: i64,
    pub previous_checkpoint_sha256: Option<String>,
    /// A complete original metadata proof from sequence zero to the frozen head.
    /// It contains no prompts, credentials, paths, or model responses.
    pub archive: AuditArchiveBundle,
    pub public_key: String,
    pub public_key_sha256: String,
    pub signature: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SignedAuditCheckpoint {
    pub format: String,
    pub request_id: Uuid,
    pub binding_id: Uuid,
    pub origin_id: Uuid,
    pub scope: String,
    pub candidate_sha256: String,
    pub origin_public_key_sha256: String,
    pub chain_id: Uuid,
    pub head_sequence: i64,
    pub head_sha256: String,
    pub witnessed_at: i64,
    pub witness_sequence: i64,
    pub previous_checkpoint_sha256: Option<String>,
    /// True only after an explicit independent operator permits this new chain.
    pub operator_chain_transition: bool,
    pub public_key: String,
    pub public_key_sha256: String,
    pub signature: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportAuditCheckpoint {
    pub request_id: Uuid,
    pub candidate_request_id: Uuid,
    pub checkpoint_bytes_base64: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditCheckpointStatus {
    pub binding: Option<AuditWitnessBinding>,
    pub latest: Option<SignedAuditCheckpoint>,
    pub latest_sha256: Option<String>,
    pub pending_request_id: Option<Uuid>,
    pub current_chain_matches: bool,
    pub current_prefix_matches: bool,
    #[serde(default)]
    pub trust_transitions: Vec<SignedAuditTrustTransition>,
    pub pending_transition_request_id: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrepareAuditTrustTransition {
    pub request_id: Uuid,
    pub binding_id: Uuid,
    pub previous_checkpoint_sha256: String,
    pub new_witness_public_key_sha256: String,
    pub independently_confirmed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditTrustTransitionProposal {
    pub format: String,
    pub request_id: Uuid,
    pub previous_binding: AuditWitnessBinding,
    pub next_binding: AuditWitnessBinding,
    pub previous_checkpoint: SignedAuditCheckpoint,
    pub prepared_at: i64,
    pub previous_origin_signature: String,
    pub next_origin_signature: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SignedAuditTrustTransition {
    pub format: String,
    pub proposal: AuditTrustTransitionProposal,
    pub accepted_at: i64,
    pub previous_witness_public_key: String,
    pub next_witness_public_key: String,
    pub previous_witness_signature: String,
    pub next_witness_signature: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportAuditTrustTransition {
    pub request_id: Uuid,
    pub proposal_request_id: Uuid,
    pub original_bytes_base64: String,
    pub independently_confirmed: bool,
}

/// Public original operator plan; no KMS token, wrapping plaintext or key seed.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditOriginRotationPlan {
    pub format: String,
    pub request_id: Uuid,
    pub instance_id: Uuid,
    pub original_registry_sha256: String,
    pub original_audit_key: Uuid,
    pub next_audit_key: Uuid,
    pub original_epoch: i64,
    pub next_epoch: i64,
    pub prepared_at: i64,
    pub proposals: Vec<AuditTrustTransitionProposal>,
    pub previous_public_key: String,
    pub previous_public_key_sha256: String,
    pub next_public_key: String,
    pub next_public_key_sha256: String,
    pub previous_origin_signature: String,
    pub next_origin_signature: String,
}
