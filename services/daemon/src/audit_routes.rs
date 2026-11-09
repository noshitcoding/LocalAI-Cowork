use super::*;
use cowork_identity::audit::AuditAuthority;

pub(super) fn router(authority: Option<AuditAuthority>) -> Router<AppState> {
    Router::new()
        .route("/audit", get(page))
        .route("/audit/archives", post(capture))
        .route("/audit/history", get(history).post(preserve))
        .route("/audit/history/{request}", get(original))
        .route("/audit/verification", get(verification))
        .route("/insights", get(insights))
        .route("/audit/diagnostics", get(diagnostics))
        .route("/audit/recovery", get(recovery_plan).post(retire))
        .route("/audit/retirements", get(retirements))
        .route(
            "/audit/retirements/{request}/original",
            get(retired_original),
        )
        .route(
            "/audit/checkpoints",
            get(checkpoint_status).post(checkpoint_capture),
        )
        .route("/audit/checkpoints/witness", post(configure_witness))
        .route("/audit/checkpoints/transitions", post(trust_prepare))
        .route("/audit/checkpoints/transitions/import", post(trust_import))
        .route(
            "/audit/checkpoints/transitions/{request}",
            get(trust_proposal),
        )
        .route(
            "/audit/checkpoints/transitions/{request}/original",
            get(trust_original),
        )
        .route("/audit/checkpoints/import", post(checkpoint_import))
        .route("/audit/checkpoints/history", get(checkpoint_history))
        .route(
            "/audit/checkpoints/requests/{request}",
            get(checkpoint_request),
        )
        .route(
            "/audit/checkpoints/originals/{request}",
            get(checkpoint_original),
        )
        .layer(DefaultBodyLimit::max(24 * 1024 * 1024))
        .layer(axum::Extension(authority))
}
async fn trust_prepare(
    State(s): State<AppState>,
    axum::Extension(a): axum::Extension<Option<AuditAuthority>>,
    Json(input): Json<PrepareAuditTrustTransition>,
) -> Answer<AuditTrustTransitionProposal> {
    Ok(Json(
        s.runtime
            .store
            .personal_audit_trust_prepare(
                a.as_ref().ok_or(ApiFailure(Error::NativeKeyUnavailable))?,
                input,
            )
            .await?,
    ))
}
async fn trust_import(
    State(s): State<AppState>,
    Json(input): Json<ImportAuditTrustTransition>,
) -> Answer<SignedAuditTrustTransition> {
    Ok(Json(
        s.runtime.store.personal_audit_trust_import(input).await?,
    ))
}
async fn trust_proposal(
    State(s): State<AppState>,
    Path(id): Path<uuid::Uuid>,
) -> Answer<AuditTrustTransitionProposal> {
    Ok(Json(
        s.runtime.store.personal_audit_trust_proposal(id).await?,
    ))
}
async fn trust_original(State(s): State<AppState>, Path(id): Path<uuid::Uuid>) -> Answer<String> {
    Ok(Json(
        s.runtime.store.personal_audit_trust_original(id).await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PageQuery {
    #[serde(default)]
    after_sequence: i64,
    through_sequence: Option<i64>,
    #[serde(default = "page_limit")]
    limit: i64,
}
fn page_limit() -> i64 {
    100
}
async fn page(State(s): State<AppState>, Query(q): Query<PageQuery>) -> Answer<AuditPage> {
    Ok(Json(
        s.runtime
            .store
            .personal_audit_page(q.after_sequence, q.through_sequence, q.limit)
            .await?,
    ))
}
async fn capture(
    State(s): State<AppState>,
    axum::Extension(authority): axum::Extension<Option<AuditAuthority>>,
    Json(input): Json<CaptureAuditArchive>,
) -> Answer<SignedAuditArchive> {
    let authority = authority
        .as_ref()
        .ok_or(ApiFailure(Error::NativeKeyUnavailable))?;
    Ok(Json(
        s.runtime
            .store
            .personal_audit_capture(authority, input)
            .await?,
    ))
}
async fn insights(
    State(s): State<AppState>,
    axum::Extension(authority): axum::Extension<Option<AuditAuthority>>,
    Query(query): Query<AuditInsightsQuery>,
) -> Answer<AuditInsights> {
    Ok(Json(
        s.runtime
            .store
            .personal_audit_insights_query(authority.as_ref(), query)
            .await?,
    ))
}
async fn verification(
    State(s): State<AppState>,
    axum::Extension(authority): axum::Extension<Option<AuditAuthority>>,
) -> Answer<AuditVerification> {
    Ok(Json(
        s.runtime
            .store
            .personal_audit_insights(authority.as_ref())
            .await?
            .verification,
    ))
}
async fn diagnostics(
    State(s): State<AppState>,
    axum::Extension(authority): axum::Extension<Option<AuditAuthority>>,
) -> Answer<AuditDiagnostics> {
    Ok(Json(AuditDiagnostics {
        format: "cowork.audit.diagnostics.v1".into(),
        captured_at: cowork_storage::now(),
        insights: s
            .runtime
            .store
            .personal_audit_insights(authority.as_ref())
            .await?,
        redaction: "metadata_only_no_prompts_credentials_paths_arguments_or_model_bodies".into(),
    }))
}
async fn preserve(
    State(s): State<AppState>,
    Json(input): Json<PreserveAuditHistory>,
) -> Answer<PreservedAuditHistory> {
    Ok(Json(s.runtime.store.personal_audit_preserve(input).await?))
}
async fn history(State(s): State<AppState>) -> Answer<Vec<PreservedAuditHistory>> {
    Ok(Json(s.runtime.store.personal_audit_history().await?))
}
async fn original(State(s): State<AppState>, Path(request): Path<uuid::Uuid>) -> Answer<String> {
    Ok(Json(
        s.runtime.store.personal_audit_original(request).await?,
    ))
}
async fn recovery_plan(
    State(s): State<AppState>,
    axum::Extension(authority): axum::Extension<Option<AuditAuthority>>,
) -> Answer<AuditRecoveryPlan> {
    let mut v = s.runtime.store.personal_audit_recovery_plan().await?;
    v.public_key_sha256 = authority
        .as_ref()
        .map(AuditAuthority::public_key_sha256)
        .transpose()
        .map_err(|_| ApiFailure(Error::NativeKeyUnavailable))?;
    Ok(Json(v))
}
async fn retire(
    State(s): State<AppState>,
    axum::Extension(authority): axum::Extension<Option<AuditAuthority>>,
    Json(input): Json<ConfirmAuditRecovery>,
) -> Answer<AuditRecoveryReceipt> {
    Ok(Json(
        s.runtime
            .store
            .personal_audit_retire(
                authority
                    .as_ref()
                    .ok_or(ApiFailure(Error::NativeKeyUnavailable))?,
                input,
            )
            .await?,
    ))
}
async fn retirements(State(s): State<AppState>) -> Answer<Vec<AuditRecoveryReceipt>> {
    Ok(Json(s.runtime.store.personal_audit_retirements().await?))
}
async fn retired_original(
    State(s): State<AppState>,
    Path(request): Path<uuid::Uuid>,
) -> Answer<String> {
    Ok(Json(
        s.runtime
            .store
            .personal_audit_retired_original(request)
            .await?,
    ))
}
async fn configure_witness(
    State(s): State<AppState>,
    axum::Extension(a): axum::Extension<Option<AuditAuthority>>,
    Json(input): Json<ConfigureAuditWitness>,
) -> Answer<AuditWitnessBinding> {
    Ok(Json(
        s.runtime
            .store
            .personal_audit_witness(
                a.as_ref().ok_or(ApiFailure(Error::NativeKeyUnavailable))?,
                input,
            )
            .await?,
    ))
}
async fn checkpoint_capture(
    State(s): State<AppState>,
    axum::Extension(a): axum::Extension<Option<AuditAuthority>>,
    Json(input): Json<CaptureAuditCheckpoint>,
) -> Answer<AuditCheckpointRequest> {
    Ok(Json(
        s.runtime
            .store
            .personal_audit_checkpoint_capture(
                a.as_ref().ok_or(ApiFailure(Error::NativeKeyUnavailable))?,
                input,
            )
            .await?,
    ))
}
async fn checkpoint_status(State(s): State<AppState>) -> Answer<AuditCheckpointStatus> {
    Ok(Json(
        s.runtime.store.personal_audit_checkpoint_status().await?,
    ))
}
async fn checkpoint_request(
    State(s): State<AppState>,
    Path(id): Path<uuid::Uuid>,
) -> Answer<AuditCheckpointRequest> {
    Ok(Json(
        s.runtime
            .store
            .personal_audit_checkpoint_request(id)
            .await?,
    ))
}
async fn checkpoint_import(
    State(s): State<AppState>,
    Json(input): Json<ImportAuditCheckpoint>,
) -> Answer<SignedAuditCheckpoint> {
    Ok(Json(
        s.runtime
            .store
            .personal_audit_checkpoint_import(input)
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckpointQuery {
    #[serde(default)]
    after_sequence: i64,
    #[serde(default = "checkpoint_limit")]
    limit: i64,
}
fn checkpoint_limit() -> i64 {
    100
}
async fn checkpoint_history(
    State(s): State<AppState>,
    Query(q): Query<CheckpointQuery>,
) -> Answer<Vec<SignedAuditCheckpoint>> {
    Ok(Json(
        s.runtime
            .store
            .personal_audit_checkpoint_history(q.after_sequence, q.limit)
            .await?,
    ))
}
async fn checkpoint_original(
    State(s): State<AppState>,
    Path(id): Path<uuid::Uuid>,
) -> Answer<String> {
    Ok(Json(
        s.runtime
            .store
            .personal_audit_checkpoint_original(id)
            .await?,
    ))
}
