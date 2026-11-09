use super::*;
use cowork_identity::audit::AuditAuthority;
pub(super) fn router(state: AppState, authority: Option<AuditAuthority>) -> Router<AppState> {
    Router::new()
        .route("/audit/v1/verify", post(verify))
        .layer(DefaultBodyLimit::max(16 * 1024))
        .layer(axum::Extension(authority))
        .layer(middleware::from_fn_with_state(
            state,
            native_folders::authorize_native,
        ))
}
async fn verify(
    State(s): State<AppState>,
    axum::Extension(authority): axum::Extension<Option<AuditAuthority>>,
    Json(input): Json<VerifyAuditV1>,
) -> Answer<AuditV1Verification> {
    Ok(Json(
        s.runtime
            .store
            .personal_audit_verify_v1(
                authority
                    .as_ref()
                    .ok_or(ApiFailure(Error::NativeKeyUnavailable))?,
                input,
            )
            .await?,
    ))
}
