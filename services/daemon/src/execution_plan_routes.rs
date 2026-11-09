use crate::*;
use cowork_workflow_contracts::{
    CommitExecutionPlan, PreparedExecutionPlan, RetiredExecutionRequest,
};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/execution-plans", post(prepare))
        .route("/execution-plans/retire-request", post(retire_request))
        .route("/orders/{order}/execution-plans/{plan}", get(inspect))
        .route(
            "/orders/{order}/execution-plans/{plan}/commit",
            post(commit),
        )
        .route(
            "/orders/{order}/execution-plans/{plan}/retire",
            post(retire),
        )
}

async fn retire_request(
    State(s): State<AppState>,
    body: Input<RunRequest>,
) -> Answer<RetiredExecutionRequest> {
    Ok(Json(
        s.runtime
            .store
            .retire_execution_request(&input(body)?)
            .await?,
    ))
}

async fn prepare(
    State(s): State<AppState>,
    body: Input<RunRequest>,
) -> Answer<PreparedExecutionPlan> {
    Ok(Json(s.runtime.prepare_execution_plan(&input(body)?).await?))
}

async fn inspect(
    State(s): State<AppState>,
    Path((order, plan)): Path<(String, String)>,
) -> Answer<PreparedExecutionPlan> {
    Ok(Json(s.runtime.inspect_execution_plan(&order, &plan).await?))
}

async fn commit(
    State(s): State<AppState>,
    Path((order, plan)): Path<(String, String)>,
    body: Input<CommitExecutionPlan>,
) -> Answer<RunRecord> {
    Ok(Json(
        s.runtime
            .commit_execution_plan(&order, &plan, &input(body)?)
            .await?,
    ))
}

async fn retire(
    State(s): State<AppState>,
    Path((order, plan)): Path<(String, String)>,
    body: Input<CommitExecutionPlan>,
) -> Answer<PreparedExecutionPlan> {
    Ok(Json(
        s.runtime
            .retire_execution_plan(&order, &plan, &input(body)?)
            .await?,
    ))
}
