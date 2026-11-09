use super::*;
use crate::audit::AuditAuthority;
use cowork_contracts::{
    AuditDiagnostics, AuditInsights, AuditPage, AuditVerification, CaptureAuditArchive,
    PreserveAuditHistory, PreservedAuditHistory, SignedAuditArchive,
};

pub(super) fn router() -> Router<HttpState> {
    let scoped = Router::new()
        .route("/verification", get(verification))
        .route("/archives", post(capture))
        .route("/history", get(history).post(preserve))
        .route("/history/{request}", get(original))
        .route("/insights", get(insights))
        .route("/diagnostics", get(diagnostics))
        .route("/recovery", get(recovery_plan).post(retire))
        .route("/retirements", get(retirements))
        .route("/retirements/{request}/original", get(retired_original))
        .route(
            "/checkpoints",
            get(checkpoint_status).post(checkpoint_capture),
        )
        .route("/checkpoints/witness", post(configure_witness))
        .route("/checkpoints/transitions", post(trust_prepare))
        .route("/checkpoints/transitions/import", post(trust_import))
        .route("/checkpoints/transitions/{request}", get(trust_proposal))
        .route(
            "/checkpoints/transitions/{request}/original",
            get(trust_original),
        )
        .route("/checkpoints/import", post(checkpoint_import))
        .route("/checkpoints/history", get(checkpoint_history))
        .route("/checkpoints/requests/{request}", get(checkpoint_request))
        .route("/checkpoints/originals/{request}", get(checkpoint_original))
        .layer(DefaultBodyLimit::max(24 * 1024 * 1024));
    Router::new()
        .route("/me/audit-v2", get(page))
        .route("/organizations/{org}/audit-v2", get(page))
        .nest("/me/audit-v2", scoped.clone())
        .nest("/organizations/{org}/audit-v2", scoped)
}
async fn trust_prepare(
    State(s): State<HttpState>,
    Session(actor): Session,
    AuditScope(org): AuditScope,
    StrictJson(input): StrictJson<cowork_contracts::PrepareAuditTrustTransition>,
) -> ApiResult<cowork_contracts::AuditTrustTransitionProposal> {
    Ok(Json(
        s.store
            .audit_trust_prepare(
                &actor,
                org,
                s.audit_authority
                    .as_ref()
                    .ok_or(ApiError(IdentityError::Corrupt))?,
                input,
            )
            .await?,
    ))
}
async fn trust_import(
    State(s): State<HttpState>,
    Session(actor): Session,
    AuditScope(org): AuditScope,
    StrictJson(input): StrictJson<cowork_contracts::ImportAuditTrustTransition>,
) -> ApiResult<cowork_contracts::SignedAuditTrustTransition> {
    Ok(Json(s.store.audit_trust_import(&actor, org, input).await?))
}
async fn trust_proposal(
    State(s): State<HttpState>,
    Session(actor): Session,
    AuditScope(org): AuditScope,
    Path(values): Path<std::collections::HashMap<String, String>>,
) -> ApiResult<cowork_contracts::AuditTrustTransitionProposal> {
    Ok(Json(
        s.store
            .audit_trust_proposal(&actor, org, checkpoint_id(&values)?)
            .await?,
    ))
}
async fn trust_original(
    State(s): State<HttpState>,
    Session(actor): Session,
    AuditScope(org): AuditScope,
    Path(values): Path<std::collections::HashMap<String, String>>,
) -> ApiResult<String> {
    Ok(Json(
        s.store
            .audit_trust_original(&actor, org, checkpoint_id(&values)?)
            .await?,
    ))
}
struct AuditScope(Option<Uuid>);
impl FromRequestParts<HttpState> for AuditScope {
    type Rejection = ApiError;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &HttpState,
    ) -> std::result::Result<Self, Self::Rejection> {
        let values =
            Path::<std::collections::HashMap<String, String>>::from_request_parts(parts, state)
                .await;
        let org = match values {
            Ok(Path(values)) => values
                .get("org")
                .map(|value| {
                    Uuid::parse_str(value).map_err(|_| ApiError(IdentityError::Validation))
                })
                .transpose()?,
            Err(_) => None,
        };
        Ok(Self(org))
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PageQuery {
    #[serde(default)]
    after_sequence: i64,
    through_sequence: Option<i64>,
    #[serde(default = "default_limit")]
    limit: i64,
}
async fn page(
    State(s): State<HttpState>,
    Session(actor): Session,
    AuditScope(org): AuditScope,
    StrictQuery(q): StrictQuery<PageQuery>,
) -> ApiResult<AuditPage> {
    Ok(Json(
        s.store
            .audit_page(&actor, org, q.after_sequence, q.through_sequence, q.limit)
            .await?,
    ))
}
async fn capture(
    State(s): State<HttpState>,
    Session(actor): Session,
    AuditScope(org): AuditScope,
    StrictJson(input): StrictJson<CaptureAuditArchive>,
) -> ApiResult<SignedAuditArchive> {
    let authority = s
        .audit_authority
        .as_ref()
        .ok_or(ApiError(IdentityError::Corrupt))?;
    Ok(Json(
        s.store.audit_capture(&actor, org, authority, input).await?,
    ))
}
async fn verification(
    State(s): State<HttpState>,
    Session(actor): Session,
    AuditScope(org): AuditScope,
) -> ApiResult<AuditVerification> {
    Ok(Json(s.store.audit_verification(&actor, org).await?))
}
async fn insights(
    State(s): State<HttpState>,
    Session(actor): Session,
    AuditScope(org): AuditScope,
    StrictQuery(query): StrictQuery<cowork_contracts::AuditInsightsQuery>,
) -> ApiResult<AuditInsights> {
    Ok(Json(
        s.store
            .audit_insights_query(&actor, org, s.audit_authority.as_ref(), query)
            .await?,
    ))
}
async fn diagnostics(
    State(s): State<HttpState>,
    Session(actor): Session,
    AuditScope(org): AuditScope,
) -> ApiResult<AuditDiagnostics> {
    let insights = s
        .store
        .audit_insights(&actor, org, s.audit_authority.as_ref())
        .await?;
    Ok(Json(AuditDiagnostics {
        format: "cowork.audit.diagnostics.v1".into(),
        captured_at: chrono::Utc::now().timestamp_millis(),
        insights,
        redaction: "metadata_only_no_prompts_credentials_paths_arguments_or_model_bodies".into(),
    }))
}
async fn preserve(
    State(s): State<HttpState>,
    Session(actor): Session,
    AuditScope(org): AuditScope,
    StrictJson(input): StrictJson<PreserveAuditHistory>,
) -> ApiResult<PreservedAuditHistory> {
    Ok(Json(s.store.audit_preserve(&actor, org, input).await?))
}
async fn history(
    State(s): State<HttpState>,
    Session(actor): Session,
    AuditScope(org): AuditScope,
) -> ApiResult<Vec<PreservedAuditHistory>> {
    Ok(Json(s.store.audit_preserved_history(&actor, org).await?))
}
async fn original(
    State(s): State<HttpState>,
    Session(actor): Session,
    AuditScope(org): AuditScope,
    Path(values): Path<std::collections::HashMap<String, String>>,
) -> ApiResult<String> {
    let request = values
        .get("request")
        .and_then(|v| Uuid::parse_str(v).ok())
        .ok_or(ApiError(IdentityError::Validation))?;
    Ok(Json(
        s.store.audit_history_bytes(&actor, org, request).await?,
    ))
}
async fn recovery_plan(
    State(s): State<HttpState>,
    Session(actor): Session,
    AuditScope(org): AuditScope,
) -> ApiResult<cowork_contracts::AuditRecoveryPlan> {
    let mut v = s.store.audit_recovery_plan(&actor, org).await?;
    v.public_key_sha256 = s
        .audit_authority
        .as_ref()
        .map(AuditAuthority::public_key_sha256)
        .transpose()?;
    Ok(Json(v))
}
async fn retire(
    State(s): State<HttpState>,
    Session(actor): Session,
    AuditScope(org): AuditScope,
    StrictJson(input): StrictJson<cowork_contracts::ConfirmAuditRecovery>,
) -> ApiResult<cowork_contracts::AuditRecoveryReceipt> {
    let authority = s
        .audit_authority
        .as_ref()
        .ok_or(ApiError(IdentityError::Corrupt))?;
    Ok(Json(
        s.store.audit_retire(&actor, org, authority, input).await?,
    ))
}
async fn retirements(
    State(s): State<HttpState>,
    Session(actor): Session,
    AuditScope(org): AuditScope,
) -> ApiResult<Vec<cowork_contracts::AuditRecoveryReceipt>> {
    Ok(Json(s.store.audit_retirements(&actor, org).await?))
}
async fn retired_original(
    State(s): State<HttpState>,
    Session(actor): Session,
    AuditScope(org): AuditScope,
    Path(values): Path<std::collections::HashMap<String, String>>,
) -> ApiResult<String> {
    let id = values
        .get("request")
        .and_then(|v| Uuid::parse_str(v).ok())
        .ok_or(ApiError(IdentityError::Validation))?;
    Ok(Json(s.store.audit_retired_original(&actor, org, id).await?))
}
async fn configure_witness(
    State(s): State<HttpState>,
    Session(actor): Session,
    AuditScope(org): AuditScope,
    StrictJson(input): StrictJson<cowork_contracts::ConfigureAuditWitness>,
) -> ApiResult<cowork_contracts::AuditWitnessBinding> {
    Ok(Json(
        s.store
            .audit_witness(
                &actor,
                org,
                s.audit_authority
                    .as_ref()
                    .ok_or(ApiError(IdentityError::Corrupt))?,
                input,
            )
            .await?,
    ))
}
async fn checkpoint_capture(
    State(s): State<HttpState>,
    Session(actor): Session,
    AuditScope(org): AuditScope,
    StrictJson(input): StrictJson<cowork_contracts::CaptureAuditCheckpoint>,
) -> ApiResult<cowork_contracts::AuditCheckpointRequest> {
    Ok(Json(
        s.store
            .audit_checkpoint_capture(
                &actor,
                org,
                s.audit_authority
                    .as_ref()
                    .ok_or(ApiError(IdentityError::Corrupt))?,
                input,
            )
            .await?,
    ))
}
async fn checkpoint_status(
    State(s): State<HttpState>,
    Session(actor): Session,
    AuditScope(org): AuditScope,
) -> ApiResult<cowork_contracts::AuditCheckpointStatus> {
    Ok(Json(s.store.audit_checkpoint_status(&actor, org).await?))
}
fn checkpoint_id(
    values: &std::collections::HashMap<String, String>,
) -> std::result::Result<Uuid, ApiError> {
    values
        .get("request")
        .and_then(|v| Uuid::parse_str(v).ok())
        .ok_or(ApiError(IdentityError::Validation))
}
async fn checkpoint_request(
    State(s): State<HttpState>,
    Session(actor): Session,
    AuditScope(org): AuditScope,
    Path(values): Path<std::collections::HashMap<String, String>>,
) -> ApiResult<cowork_contracts::AuditCheckpointRequest> {
    Ok(Json(
        s.store
            .audit_checkpoint_request(&actor, org, checkpoint_id(&values)?)
            .await?,
    ))
}
async fn checkpoint_import(
    State(s): State<HttpState>,
    Session(actor): Session,
    AuditScope(org): AuditScope,
    StrictJson(input): StrictJson<cowork_contracts::ImportAuditCheckpoint>,
) -> ApiResult<cowork_contracts::SignedAuditCheckpoint> {
    Ok(Json(
        s.store.audit_checkpoint_import(&actor, org, input).await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckpointQuery {
    #[serde(default)]
    after_sequence: i64,
    #[serde(default = "default_limit")]
    limit: i64,
}
async fn checkpoint_history(
    State(s): State<HttpState>,
    Session(actor): Session,
    AuditScope(org): AuditScope,
    StrictQuery(q): StrictQuery<CheckpointQuery>,
) -> ApiResult<Vec<cowork_contracts::SignedAuditCheckpoint>> {
    Ok(Json(
        s.store
            .audit_checkpoint_history(&actor, org, q.after_sequence, q.limit)
            .await?,
    ))
}
async fn checkpoint_original(
    State(s): State<HttpState>,
    Session(actor): Session,
    AuditScope(org): AuditScope,
    Path(values): Path<std::collections::HashMap<String, String>>,
) -> ApiResult<String> {
    Ok(Json(
        s.store
            .audit_checkpoint_original(&actor, org, checkpoint_id(&values)?)
            .await?,
    ))
}
