//! Tenant-scoped immutable receipt reconciliation; no endpoint repeats IO.
use super::*;
use cowork_storage::team_service::{
    ResolveTeamEffect, ResumeTeamRun, TeamEffectDetail, TeamRecoveryView,
};
use serde_json::Value;

pub(super) fn routes() -> Router<TeamState> {
    Router::new()
        .route("/{team}/runs/{run}/recovery", get(view))
        .route("/{team}/runs/{run}/recovery/effects/{effect}", get(detail))
        .route(
            "/{team}/runs/{run}/recovery/operations/{operation}",
            get(receipt),
        )
        .route(
            "/{team}/runs/{run}/recovery/resolve",
            axum::routing::post(resolve),
        )
        .route(
            "/{team}/runs/{run}/recovery/resume",
            axum::routing::post(resume),
        )
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024 + 8192))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    #[serde(default)]
    after: i64,
}
async fn view(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, r)): StrictPath<(Uuid, Uuid)>,
    Query(cursor): Query<Cursor>,
) -> Answer<TeamRecoveryView> {
    Ok(Json(s.service.recovery(&a, t, r, cursor.after).await?))
}
async fn detail(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, r, e)): StrictPath<(Uuid, Uuid, Uuid)>,
) -> Answer<TeamEffectDetail> {
    Ok(Json(s.service.effect_detail(&a, t, r, e).await?))
}
async fn receipt(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, r, key)): StrictPath<(Uuid, Uuid, Uuid)>,
) -> Answer<Value> {
    Ok(Json(s.service.recovery_receipt(&a, t, r, key).await?))
}
async fn resolve(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, r)): StrictPath<(Uuid, Uuid)>,
    StrictJson(v): StrictJson<ResolveTeamEffect>,
) -> Answer<Value> {
    Ok(Json(s.service.resolve_effect(&a, t, r, v).await?))
}
async fn resume(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, r)): StrictPath<(Uuid, Uuid)>,
    StrictJson(v): StrictJson<ResumeTeamRun>,
) -> Answer<Value> {
    Ok(Json(s.service.resume_recovered(&a, t, r, v).await?))
}
