use super::*;
pub(super) fn routes() -> Router<TeamState> {
    Router::new()
        .route("/{team}/runs/{run}/container-recovery", get(plans))
        .route(
            "/{team}/runs/{run}/container-recovery/retire",
            axum::routing::post(retire),
        )
        .route(
            "/{team}/runs/{run}/container-recovery/reject",
            axum::routing::post(reject),
        )
        .route(
            "/{team}/runs/{run}/container-recovery/operations/{operation}",
            get(receipt),
        )
        .layer(DefaultBodyLimit::max(8192))
}
async fn plans(
    State(s): State<TeamState>,
    Session(actor): Session,
    StrictPath((team, run)): StrictPath<(Uuid, Uuid)>,
) -> Answer<Vec<TeamContainerRecoveryPlan>> {
    Ok(Json(
        s.service
            .container_recovery_plans(&actor, team, run)
            .await?,
    ))
}
async fn retire(
    State(s): State<TeamState>,
    Session(actor): Session,
    StrictPath((team, run)): StrictPath<(Uuid, Uuid)>,
    StrictJson(input): StrictJson<RetireTeamContainer>,
) -> Answer<TeamContainerRecoveryReceipt> {
    Ok(Json(
        s.service.retire_container(&actor, team, run, input).await?,
    ))
}
async fn receipt(
    State(s): State<TeamState>,
    Session(actor): Session,
    StrictPath((team, run, operation)): StrictPath<(Uuid, Uuid, Uuid)>,
) -> Answer<Option<TeamContainerRecoveryReceipt>> {
    Ok(Json(
        s.service
            .container_recovery_receipt(&actor, team, run, operation)
            .await?,
    ))
}
async fn reject(
    State(s): State<TeamState>,
    Session(actor): Session,
    StrictPath((team, run)): StrictPath<(Uuid, Uuid)>,
    StrictJson(input): StrictJson<RetireTeamContainer>,
) -> Answer<serde_json::Value> {
    Ok(Json(
        s.service
            .reject_container_recovery(&actor, team, run, input)
            .await?,
    ))
}
