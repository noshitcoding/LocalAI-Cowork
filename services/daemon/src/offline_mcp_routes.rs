//! Local-only task management. Team sessions cannot become OfflineDevice owners.
use super::*;
use cowork_domain::{canonical_hash, storage_error};
use serde_json::Value;
use uuid::Uuid;
pub(crate) fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/offline/runs/{run}/mcp/tasks", get(tasks))
        .route("/offline/runs/{run}/mcp/tasks/{task}", get(detail))
        .route("/offline/runs/{run}/mcp/tasks/{task}/result", get(result))
        .route("/offline/runs/{run}/mcp/tasks/{task}/cancel", post(cancel))
        .route("/offline/runs/{run}/mcp/interactions", get(interactions))
        .route(
            "/offline/runs/{run}/mcp/interactions/{interaction}/receipt",
            get(answer_receipt),
        )
        .route(
            "/offline/runs/{run}/mcp/tasks/{task}/cancel-receipt",
            get(cancel_receipt),
        )
        .route(
            "/offline/runs/{run}/mcp/interactions/{interaction}/answer",
            post(answer),
        )
        .layer(DefaultBodyLimit::max(128 * 1024))
        .layer(middleware::from_fn_with_state(
            state,
            native_folders::authorize_native,
        ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    after: Option<String>,
    #[serde(default)]
    only_waiting: bool,
}
async fn tasks(State(s): State<AppState>, Path(run): Path<Uuid>) -> Answer<Vec<McpAsyncTask>> {
    Ok(Json(s.runtime.offline_mcp_tasks(run).await?))
}
async fn detail(
    State(s): State<AppState>,
    Path((run, id)): Path<(Uuid, Uuid)>,
) -> Answer<McpTaskDetail> {
    Ok(Json(
        s.runtime
            .offline_mcp_task_detail(run, &id.to_string())
            .await?,
    ))
}
async fn interactions(
    State(s): State<AppState>,
    Path(run): Path<Uuid>,
    Query(q): Query<Page>,
) -> Answer<OfflineMcpInteractionPage> {
    Ok(Json(
        s.runtime
            .offline_mcp_interactions(run, q.after.as_deref(), q.only_waiting)
            .await?,
    ))
}
async fn cancel(
    State(s): State<AppState>,
    Path((run, id)): Path<(Uuid, Uuid)>,
    body: Input<McpAsyncTaskRequest>,
) -> Answer<Value> {
    Ok(Json(
        s.runtime
            .cancel_offline_mcp_task(run, &id.to_string(), &input(body)?)
            .await?,
    ))
}
async fn answer(
    State(s): State<AppState>,
    Path((run, id)): Path<(Uuid, Uuid)>,
    body: Input<AnswerMcpTaskInteraction>,
) -> Answer<Value> {
    Ok(Json(
        s.runtime
            .answer_offline_mcp_interaction(run, &id.to_string(), &input(body)?)
            .await?,
    ))
}
async fn result(
    State(s): State<AppState>,
    Path((run, id)): Path<(Uuid, Uuid)>,
) -> std::result::Result<Response, ApiFailure> {
    let detail = s
        .runtime
        .offline_mcp_task_detail(run, &id.to_string())
        .await?;
    let value = detail.original_result.ok_or(Error::UncertainEffect)?;
    let sha = canonical_hash(&value);
    if detail.task.result_sha256.as_deref() != Some(sha.as_str()) {
        return Err(Error::Forbidden.into());
    }
    let bytes = serde_json::to_vec(&value).map_err(storage_error)?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
            (
                header::HeaderName::from_static("x-cowork-original-sha256"),
                sha.as_str(),
            ),
        ],
        bytes,
    )
        .into_response())
}
async fn answer_receipt(
    State(s): State<AppState>,
    Path((run, id)): Path<(Uuid, Uuid)>,
    Query(q): Query<McpAsyncTaskRequest>,
) -> Answer<Option<Value>> {
    Ok(Json(
        s.runtime
            .offline_mcp_original_receipt(run, &id.to_string(), true, &q)
            .await?,
    ))
}
async fn cancel_receipt(
    State(s): State<AppState>,
    Path((run, id)): Path<(Uuid, Uuid)>,
    Query(q): Query<McpAsyncTaskRequest>,
) -> Answer<Option<Value>> {
    Ok(Json(
        s.runtime
            .offline_mcp_original_receipt(run, &id.to_string(), false, &q)
            .await?,
    ))
}
