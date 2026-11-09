//! Actual tenant authority; the controller witness is only evidence, never a bearer.
use super::*;
use serde_json::Value;
pub(super) fn routes() -> Router<TeamState> {
    Router::new()
        .route("/{team}/runs/{run}/lost-host-recovery", get(plans))
        .route(
            "/{team}/runs/{run}/lost-host-recovery/plans/{nonce}",
            get(plan),
        )
        .route(
            "/{team}/runs/{run}/lost-host-recovery/prepare",
            axum::routing::post(prepare),
        )
        .route(
            "/{team}/runs/{run}/lost-host-recovery/receive",
            axum::routing::post(receive),
        )
        .route(
            "/{team}/runs/{run}/lost-host-recovery/operations/{operation}",
            get(operation),
        )
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024 + 8192))
}
async fn plans(
    State(s): State<TeamState>,
    Session(actor): Session,
    StrictPath((team, run)): StrictPath<(Uuid, Uuid)>,
) -> Answer<Vec<TeamLostHostPlan>> {
    Ok(Json(s.service.lost_host_plans(&actor, team, run).await?))
}
async fn prepare(
    State(s): State<TeamState>,
    Session(actor): Session,
    StrictPath((team, run)): StrictPath<(Uuid, Uuid)>,
    StrictJson(input): StrictJson<PrepareTeamLostHostRetirement>,
) -> Answer<TeamLostHostPlan> {
    Ok(Json(
        s.service
            .prepare_lost_host(&actor, team, run, input)
            .await?,
    ))
}
async fn receive(
    State(s): State<TeamState>,
    Session(actor): Session,
    StrictPath((team, run)): StrictPath<(Uuid, Uuid)>,
    StrictJson(input): StrictJson<ReceiveTeamLostHostRetirement>,
) -> Answer<TeamLostHostRetirementReceipt> {
    Ok(Json(
        s.service
            .receive_lost_host(&actor, team, run, input)
            .await?,
    ))
}
async fn operation(
    State(s): State<TeamState>,
    Session(actor): Session,
    StrictPath((team, run, id)): StrictPath<(Uuid, Uuid, Uuid)>,
) -> Answer<Option<Value>> {
    Ok(Json(
        s.service.lost_host_operation(&actor, team, run, id).await?,
    ))
}

async fn plan(
    State(s): State<TeamState>,
    Session(actor): Session,
    StrictPath((team, run, nonce)): StrictPath<(Uuid, Uuid, Uuid)>,
) -> Answer<TeamLostHostPlan> {
    Ok(Json(
        s.service.lost_host_plan(&actor, team, run, nonce).await?,
    ))
}
