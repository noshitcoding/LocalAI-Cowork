//! Team sessions and original project grants; no daemon-owner identity adoption.
use super::*;
pub(super) fn routes() -> Router<TeamState> {
    Router::new()
        .route("/{team}/mcp/oauth/setup", get(setup))
        .route("/{team}/mcp/oauth", get(profiles).post(configure))
        .route("/{team}/mcp/oauth/discovery", axum::routing::post(discover))
        .route(
            "/{team}/mcp/oauth/profile/{profile}/start",
            axum::routing::post(start),
        )
        .route("/{team}/mcp/oauth/flow/{flow}", get(flow))
        .route(
            "/{team}/mcp/oauth/flow/{flow}/complete",
            axum::routing::post(complete),
        )
        .route("/{team}/runs/{run}/mcp-tasks", get(tasks))
        .route("/{team}/runs/{run}/mcp-tasks/{task}", get(detail))
        .route(
            "/{team}/runs/{run}/mcp-tasks/{task}/cancel",
            axum::routing::post(cancel),
        )
        .route("/{team}/runs/{run}/mcp-interactions", get(interactions))
        .route(
            "/{team}/runs/{run}/mcp-interactions/{interaction}",
            axum::routing::post(answer),
        )
        .layer(DefaultBodyLimit::max(128 * 1024))
}
async fn setup(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath(t): StrictPath<Uuid>,
) -> Answer<serde_json::Value> {
    s.service.team(&a, t).await?;
    Ok(Json(
        serde_json::json!({"redirect_uri":crate::mcp_oauth_routes::server_redirect()?}),
    ))
}
async fn profiles(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath(t): StrictPath<Uuid>,
) -> Answer<Vec<McpOAuthProfile>> {
    Ok(Json(s.service.team_mcp_oauth_profiles(&a, t).await?))
}
async fn discover(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath(t): StrictPath<Uuid>,
    Json(input): Json<McpOAuthDiscovery>,
) -> Answer<McpOAuthMetadata> {
    Ok(Json(
        s.service.discover_team_mcp_oauth(&a, t, &input).await?,
    ))
}
async fn configure(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath(t): StrictPath<Uuid>,
    Json(input): Json<ConfigureMcpOAuth>,
) -> Answer<McpOAuthProfile> {
    Ok(Json(
        s.service
            .configure_team_mcp_oauth(&a, t, &input, &crate::mcp_oauth_routes::server_redirect()?)
            .await?,
    ))
}
async fn start(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, p)): StrictPath<(Uuid, Uuid)>,
    Json(input): Json<McpOAuthRequest>,
) -> Answer<McpOAuthFlow> {
    Ok(Json(
        s.service
            .start_team_mcp_oauth(&a, t, &p.to_string(), &input)
            .await?,
    ))
}
async fn flow(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, f)): StrictPath<(Uuid, Uuid)>,
) -> Answer<McpOAuthFlow> {
    Ok(Json(
        s.service.team_mcp_oauth_flow(&a, t, &f.to_string()).await?,
    ))
}
async fn complete(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, f)): StrictPath<(Uuid, Uuid)>,
) -> Answer<serde_json::Value> {
    Ok(Json(
        s.service
            .complete_team_mcp_oauth(&a, t, &f.to_string())
            .await?,
    ))
}
async fn tasks(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, r)): StrictPath<(Uuid, Uuid)>,
) -> Answer<Vec<McpAsyncTask>> {
    Ok(Json(s.service.team_mcp_tasks(&a, t, r).await?))
}
async fn detail(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, r, k)): StrictPath<(Uuid, Uuid, Uuid)>,
) -> Answer<McpTaskDetail> {
    Ok(Json(
        s.service
            .team_mcp_task_detail(&a, t, r, &k.to_string())
            .await?,
    ))
}
async fn cancel(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, r, k)): StrictPath<(Uuid, Uuid, Uuid)>,
    Json(input): Json<McpAsyncTaskRequest>,
) -> Answer<serde_json::Value> {
    Ok(Json(
        s.service
            .cancel_team_mcp_task(&a, t, r, &k.to_string(), &input)
            .await?,
    ))
}
async fn interactions(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, r)): StrictPath<(Uuid, Uuid)>,
) -> Answer<Vec<McpTaskInteraction>> {
    Ok(Json(s.service.team_mcp_interactions(&a, t, r).await?))
}
async fn answer(
    State(s): State<TeamState>,
    Session(a): Session,
    StrictPath((t, r, i)): StrictPath<(Uuid, Uuid, Uuid)>,
    Json(input): Json<AnswerMcpTaskInteraction>,
) -> Answer<serde_json::Value> {
    Ok(Json(
        s.service
            .answer_team_mcp_interaction(&a, t, r, &i.to_string(), &input)
            .await?,
    ))
}
