use crate::*;
use cowork_workflow_contracts::{
    CommitExecutionPlan, PrepareImportExecutionPlanV2, PrepareOwnerExecutionPlanV2,
    PrepareRestoreExecutionPlanV2, PreparedExecutionPlanV2,
};
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/runs/{run}/workflow/input-snapshot",
            get(workflow_input_snapshot),
        )
        .route(
            "/orders/{order}/runs/{run}/working-copy/files",
            get(copy_files),
        )
        .route(
            "/orders/{order}/runs/{run}/working-copy/text",
            get(copy_text),
        )
        .route("/execution-plans/v2", post(prepare))
        .route("/execution-plans/v2/file-imports", post(prepare_import))
        .route(
            "/execution-plans/v2/file-imports/retire-request",
            post(retire_import_request),
        )
        .route(
            "/execution-plans/v2/file-imports/retire-intent",
            post(retire_import_intent),
        )
        .route(
            "/execution-plans/v2/file-restorations",
            post(prepare_restore),
        )
        .route(
            "/execution-plans/v2/file-restorations/retire-request",
            post(retire_restore_request),
        )
        .route(
            "/execution-plans/v2/file-restorations/retire-intent",
            post(retire_restore_intent),
        )
        .route("/execution-plans/v2/retire-request", post(retire_request))
        .route("/orders/{order}/execution-plans/v2/{plan}", get(inspect))
        .route(
            "/orders/{order}/execution-plans/v2/{plan}/commit",
            post(commit),
        )
        .route(
            "/orders/{order}/execution-plans/v2/{plan}/retire",
            post(retire),
        )
}
async fn workflow_input_snapshot(
    State(s): State<AppState>,
    Path(run): Path<String>,
) -> Answer<cowork_workflow_contracts::WorkflowInputSnapshot> {
    Ok(Json(s.runtime.workflow_input_snapshot(&run).await?))
}
async fn prepare_import(
    State(s): State<AppState>,
    body: Input<PrepareImportExecutionPlanV2>,
) -> Answer<PreparedExecutionPlanV2> {
    Ok(Json(
        s.runtime
            .prepare_import_execution_plan_v2(&input(body)?)
            .await?,
    ))
}
async fn retire_import_request(
    State(s): State<AppState>,
    body: Input<PrepareImportExecutionPlanV2>,
) -> Answer<cowork_workflow_contracts::RetiredExecutionRequest> {
    Ok(Json(
        s.runtime
            .retire_import_execution_request_v2(&input(body)?)
            .await?,
    ))
}
async fn retire_import_intent(
    State(s): State<AppState>,
    body: Input<CreateImportRun>,
) -> Answer<cowork_workflow_contracts::RetiredExecutionRequest> {
    Ok(Json(
        s.runtime
            .store
            .retire_import_execution_intent_v2(&input(body)?)
            .await?,
    ))
}
async fn prepare_restore(
    State(s): State<AppState>,
    body: Input<PrepareRestoreExecutionPlanV2>,
) -> Answer<PreparedExecutionPlanV2> {
    Ok(Json(
        s.runtime
            .prepare_restore_execution_plan_v2(&input(body)?)
            .await?,
    ))
}
async fn retire_restore_request(
    State(s): State<AppState>,
    body: Input<PrepareRestoreExecutionPlanV2>,
) -> Answer<cowork_workflow_contracts::RetiredExecutionRequest> {
    Ok(Json(
        s.runtime
            .retire_restore_execution_request_v2(&input(body)?)
            .await?,
    ))
}
async fn retire_restore_intent(
    State(s): State<AppState>,
    body: Input<CreateRestoreRun>,
) -> Answer<cowork_workflow_contracts::RetiredExecutionRequest> {
    Ok(Json(
        s.runtime
            .store
            .retire_restore_execution_intent_v2(&input(body)?)
            .await?,
    ))
}
async fn copy_files(
    State(s): State<AppState>,
    Path((order, run)): Path<(String, String)>,
    Query(location): Query<FileLocation>,
) -> Answer<DirectoryListing> {
    Ok(Json(
        s.runtime.run_copy_files(&order, &run, location).await?,
    ))
}
async fn copy_text(
    State(s): State<AppState>,
    Path((order, run)): Path<(String, String)>,
    Query(location): Query<FileLocation>,
) -> Answer<TextFile> {
    Ok(Json(s.runtime.run_copy_text(&order, &run, location).await?))
}
async fn retire_request(
    State(s): State<AppState>,
    body: Input<PrepareOwnerExecutionPlanV2>,
) -> Answer<cowork_workflow_contracts::RetiredExecutionRequest> {
    Ok(Json(
        s.runtime.retire_execution_request_v2(&input(body)?).await?,
    ))
}
async fn prepare(
    State(s): State<AppState>,
    body: Input<PrepareOwnerExecutionPlanV2>,
) -> Answer<PreparedExecutionPlanV2> {
    Ok(Json(
        s.runtime.prepare_execution_plan_v2(&input(body)?).await?,
    ))
}
async fn inspect(
    State(s): State<AppState>,
    Path((order, plan)): Path<(String, String)>,
) -> Answer<PreparedExecutionPlanV2> {
    Ok(Json(
        s.runtime.inspect_execution_plan_v2(&order, &plan).await?,
    ))
}
async fn commit(
    State(s): State<AppState>,
    Path((order, plan)): Path<(String, String)>,
    body: Input<CommitExecutionPlan>,
) -> Answer<RunRecord> {
    Ok(Json(
        s.runtime
            .commit_execution_plan_v2(&order, &plan, &input(body)?)
            .await?,
    ))
}
async fn retire(
    State(s): State<AppState>,
    Path((order, plan)): Path<(String, String)>,
    body: Input<CommitExecutionPlan>,
) -> Answer<PreparedExecutionPlanV2> {
    Ok(Json(
        s.runtime
            .retire_execution_plan_v2(&order, &plan, &input(body)?)
            .await?,
    ))
}
