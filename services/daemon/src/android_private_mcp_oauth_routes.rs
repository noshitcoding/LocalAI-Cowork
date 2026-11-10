use super::*;
use cowork_contracts::{
    AnswerMcpTaskInteraction, ConfigureMcpOAuth, McpAsyncTaskRequest, McpOAuthDiscovery,
    McpOAuthRequest,
};
use cowork_storage::mcp_oauth::{McpOAuthCallback, McpOAuthOwner};
pub(super) fn routes() -> Router<CompatState> {
    Router::new()
        .route(
            "/projects/{project}/mcp-metadata/{entity}",
            axum::routing::put(metadata),
        )
        .route("/projects/{project}/mcp-bindings/endpoints", get(endpoints))
        .route("/projects/{project}/mcp/oauth/setup", get(setup))
        .route(
            "/projects/{project}/mcp/oauth",
            get(profiles).post(configure),
        )
        .route("/projects/{project}/mcp/oauth/discovery", post(discover))
        .route("/mcp/oauth/profile/{profile}/start", post(start))
        .route("/mcp/oauth/flow/{flow}", get(flow))
        .route("/mcp/oauth/flow/{flow}/complete", post(complete))
        .route("/runs/{run}/mcp-tasks", get(tasks))
        .route("/runs/{run}/mcp-tasks/{task}", get(detail))
        .route("/runs/{run}/mcp-tasks/{task}/cancel", post(cancel))
        .route("/runs/{run}/mcp-interactions", get(interactions))
        .route("/runs/{run}/mcp-interactions/{interaction}", post(answer))
        .route("/mcp/oauth/callback", get(callback))
        .layer(axum::extract::DefaultBodyLimit::max(128 * 1024))
}
async fn metadata(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((p, e)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Answer {
    let operation = super::operation(&headers)?.ok_or_else(invalid)?;
    Ok(Json(
        s.service
            .store()
            .android_private_mcp_metadata(&a, p, e, &body, operation, s.keys.as_ref(), s.key_id)
            .await?,
    ))
}
async fn endpoints(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(p): Path<Uuid>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .android_private_mcp_public_endpoints(&a, p, s.keys.as_ref())
            .await?
    )))
}
async fn setup(State(s): State<CompatState>, Session(a): Session, Path(p): Path<Uuid>) -> Answer {
    s.service.store().android_personal_project(&a, p).await?;
    Ok(Json(
        json!({"redirect_uri":crate::mcp_oauth_routes::server_redirect()?}),
    ))
}
async fn profiles(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(p): Path<Uuid>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .mcp_oauth_profiles(
                &McpOAuthOwner::Personal {
                    actor: &a,
                    key: s.key_id
                },
                &p.to_string(),
                s.keys.as_ref()
            )
            .await?
    )))
}
async fn discover(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(p): Path<Uuid>,
    Json(input): Json<McpOAuthDiscovery>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .discover_scoped_mcp_oauth(
                &McpOAuthOwner::Personal {
                    actor: &a,
                    key: s.key_id
                },
                &p.to_string(),
                &input,
                s.keys.as_ref()
            )
            .await?
    )))
}
async fn configure(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(p): Path<Uuid>,
    Json(input): Json<ConfigureMcpOAuth>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .configure_scoped_mcp_oauth(
                &McpOAuthOwner::Personal {
                    actor: &a,
                    key: s.key_id
                },
                &p.to_string(),
                &input,
                &crate::mcp_oauth_routes::server_redirect()?,
                s.keys.as_ref()
            )
            .await?
    )))
}
async fn start(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(p): Path<Uuid>,
    Json(input): Json<McpOAuthRequest>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .start_mcp_oauth(
                &McpOAuthOwner::Personal {
                    actor: &a,
                    key: s.key_id
                },
                &p.to_string(),
                &input,
                s.keys.as_ref()
            )
            .await?
    )))
}
async fn flow(State(s): State<CompatState>, Session(a): Session, Path(f): Path<Uuid>) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .mcp_oauth_flow(
                &McpOAuthOwner::Personal {
                    actor: &a,
                    key: s.key_id
                },
                &f.to_string(),
                s.keys.as_ref()
            )
            .await?
            .view
    )))
}
async fn complete(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(f): Path<Uuid>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .complete_mcp_oauth_code(
                &McpOAuthOwner::Personal {
                    actor: &a,
                    key: s.key_id,
                },
                &f.to_string(),
                s.keys.as_ref(),
            )
            .await?,
    ))
}
async fn tasks(State(s): State<CompatState>, Session(a): Session, Path(r): Path<Uuid>) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .scoped_mcp_tasks(
                &McpOAuthOwner::Personal {
                    actor: &a,
                    key: s.key_id
                },
                &r.to_string(),
                s.keys.as_ref()
            )
            .await?
    )))
}
async fn detail(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((r, t)): Path<(Uuid, Uuid)>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .scoped_mcp_task_detail(
                &McpOAuthOwner::Personal {
                    actor: &a,
                    key: s.key_id
                },
                &r.to_string(),
                &t.to_string(),
                s.keys.as_ref()
            )
            .await?
    )))
}
async fn cancel(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((r, t)): Path<(Uuid, Uuid)>,
    Json(input): Json<McpAsyncTaskRequest>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .request_scoped_mcp_task_cancel(
                &McpOAuthOwner::Personal {
                    actor: &a,
                    key: s.key_id,
                },
                &r.to_string(),
                &t.to_string(),
                &input,
                s.keys.as_ref(),
            )
            .await?,
    ))
}
async fn interactions(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(r): Path<Uuid>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .mcp_task_interactions(
                &McpOAuthOwner::Personal {
                    actor: &a,
                    key: s.key_id
                },
                &r.to_string(),
                s.keys.as_ref()
            )
            .await?
    )))
}
async fn answer(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((r, i)): Path<(Uuid, Uuid)>,
    Json(input): Json<AnswerMcpTaskInteraction>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .answer_mcp_task_interaction(
                &McpOAuthOwner::Personal {
                    actor: &a,
                    key: s.key_id,
                },
                &r.to_string(),
                &i.to_string(),
                &input,
                s.keys.as_ref(),
            )
            .await?,
    ))
}
async fn callback(
    State(s): State<CompatState>,
    Query(callback): Query<McpOAuthCallback>,
) -> std::result::Result<axum::response::Html<&'static str>, Failure> {
    s.service
        .store()
        .capture_server_mcp_oauth_callback(
            &callback,
            &crate::mcp_oauth_routes::server_redirect()?,
            s.keys.as_ref(),
            s.key_id,
        )
        .await?;
    Ok(axum::response::Html(
        "<!doctype html><html lang=en><meta charset=utf-8><meta name=referrer content=no-referrer><title>MCP authorization</title><p>Return to Local AI Cowork to finish this original sign-in.</p><p>Kehre zu Local AI Cowork zurück, um diese ursprüngliche Anmeldung abzuschließen.</p></html>",
    ))
}
