//! Native private OAuth management. The browser callback is evidence-only and
//! neither accepts a daemon token in its URL nor dispatches a token POST.
use super::*;
use axum::{extract::Query, response::Html};
use cowork_storage::mcp_oauth::McpOAuthCallback;
pub(super) fn server_redirect() -> cowork_domain::Result<String> {
    let origin = std::env::var("COWORK_PUBLIC_ORIGIN").map_err(|_| {
        Error::Validation(
            "Configure the server's exact public HTTPS origin before server MCP OAuth".into(),
        )
    })?;
    crate::server_config::origins(43821, Some(&origin))
        .map_err(|_| Error::Validation("Invalid configured public OAuth origin".into()))?;
    Ok(format!("{origin}/api/v1/mcp/oauth/callback"))
}
pub(super) fn native_router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/mcp/oauth/{project}", get(profiles).post(configure))
        .route("/mcp/oauth/{project}/discovery", post(discover))
        .route("/mcp/oauth/profile/{profile}/start", post(start))
        .route("/mcp/oauth/flow/{flow}", get(flow))
        .route("/mcp/oauth/flow/{flow}/complete", post(complete))
        .route("/mcp/tasks/{run}/interactions", get(interactions))
        .route(
            "/mcp/tasks/{run}/interactions/{interaction}",
            post(answer_interaction),
        )
        .layer(DefaultBodyLimit::max(128 * 1024))
        .layer(middleware::from_fn_with_state(
            state,
            native_folders::authorize_native,
        ))
}
pub(super) fn public_router() -> Router<AppState> {
    Router::new().route("/mcp/oauth/callback", get(callback))
}
pub(super) fn owner_router() -> Router<AppState> {
    Router::new()
        .route("/runs/{run}/mcp-tasks", get(tasks))
        .route("/runs/{run}/mcp-tasks/{task}", get(task_detail))
        .route("/runs/{run}/mcp-tasks/{task}/cancel", post(cancel_task))
}
async fn interactions(
    State(s): State<AppState>,
    Path(run): Path<String>,
) -> Answer<Vec<McpTaskInteraction>> {
    Ok(Json(s.runtime.native_mcp_interactions(&run).await?))
}
async fn answer_interaction(
    State(s): State<AppState>,
    Path((run, id)): Path<(String, String)>,
    body: Input<AnswerMcpTaskInteraction>,
) -> Answer<Value> {
    Ok(Json(
        s.runtime
            .answer_native_mcp_interaction(&run, &id, &input(body)?)
            .await?,
    ))
}
async fn task_detail(
    State(s): State<AppState>,
    Path((run, task)): Path<(String, String)>,
) -> Answer<McpTaskDetail> {
    Ok(Json(s.runtime.native_mcp_task_detail(&run, &task).await?))
}
async fn profiles(
    State(s): State<AppState>,
    Path(project): Path<String>,
) -> Answer<Vec<McpOAuthProfile>> {
    Ok(Json(s.runtime.native_mcp_oauth_profiles(&project).await?))
}
async fn configure(
    State(s): State<AppState>,
    Path(project): Path<String>,
    body: Input<ConfigureMcpOAuth>,
) -> Answer<McpOAuthProfile> {
    let input = input(body)?;
    Ok(Json(
        s.runtime
            .configure_native_mcp_oauth(&project, &input)
            .await?,
    ))
}
async fn discover(
    State(s): State<AppState>,
    Path(project): Path<String>,
    body: Input<McpOAuthDiscovery>,
) -> Answer<McpOAuthMetadata> {
    let input = input(body)?;
    Ok(Json(
        s.runtime
            .discover_native_mcp_oauth(&project, &input)
            .await?,
    ))
}
async fn start(
    State(s): State<AppState>,
    Path(profile): Path<String>,
    body: Input<McpOAuthRequest>,
) -> Answer<McpOAuthFlow> {
    let input = input(body)?;
    Ok(Json(
        s.runtime.start_native_mcp_oauth(&profile, &input).await?,
    ))
}
async fn flow(State(s): State<AppState>, Path(flow): Path<String>) -> Answer<McpOAuthFlow> {
    Ok(Json(s.runtime.native_mcp_oauth_flow(&flow).await?))
}
async fn complete(State(s): State<AppState>, Path(flow): Path<String>) -> Answer<Value> {
    Ok(Json(s.runtime.complete_native_mcp_oauth(&flow).await?))
}
async fn tasks(State(s): State<AppState>, Path(run): Path<String>) -> Answer<Vec<McpAsyncTask>> {
    Ok(Json(s.runtime.store.native_mcp_tasks(&run).await?))
}
async fn cancel_task(
    State(s): State<AppState>,
    Path((run, task)): Path<(String, String)>,
    body: Input<McpAsyncTaskRequest>,
) -> Answer<Value> {
    let input = input(body)?;
    Ok(Json(
        s.runtime
            .store
            .request_native_mcp_task_cancel(&run, &task, &input)
            .await?,
    ))
}
async fn callback(
    State(s): State<AppState>,
    Query(callback): Query<McpOAuthCallback>,
) -> Result<Html<&'static str>, ApiFailure> {
    s.runtime.native_mcp_oauth_callback(&callback).await?;
    // Static response only: provider code/state never enter HTML, browser logs,
    // storage or the external-link referrer. Adoption requires private IPC.
    Ok(Html(
        "<!doctype html><html lang=en><meta charset=utf-8><meta name=referrer content=no-referrer><title>MCP authorization</title><p>Authorization response received. Return to Local AI Cowork to finish connecting.</p><p>Antwort empfangen. Kehre zu Local AI Cowork zurück, um die Verbindung abzuschließen.</p></html>",
    ))
}
