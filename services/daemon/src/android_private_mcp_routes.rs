//! Original actor-private V1 MCP binding routes. Decrypted configurations never cross this API.
use super::*;
#[path = "android_private_mcp_oauth_routes.rs"]
mod oauth;
pub(super) fn routes() -> Router<CompatState> {
    Router::new()
        .route("/projects/{project}/mcp-bindings", get(list))
        .route(
            "/projects/{project}/mcp-bindings/{entity}",
            axum::routing::put(set).delete(remove),
        )
        .merge(oauth::routes())
}
fn operation(headers: &HeaderMap) -> Result<Option<&str>> {
    headers
        .get("idempotency-key")
        .or_else(|| headers.get("x-operation-id"))
        .map(|v| v.to_str().map_err(|_| invalid()))
        .transpose()
}
async fn list(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(project): Path<Uuid>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .android_private_mcp_bindings(&a, project)
            .await?
    )))
}
async fn set(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((project, entity)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_private_mcp_mutate(
                &a,
                project,
                entity,
                &body,
                false,
                operation(&headers)?,
                s.keys.as_ref(),
                s.key_id,
            )
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delete {
    expected_revision: i64,
}
async fn remove(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((project, entity)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Query(q): Query<Delete>,
) -> std::result::Result<StatusCode, Failure> {
    s.service
        .store()
        .android_private_mcp_mutate(
            &a,
            project,
            entity,
            &json!({"expected_revision":q.expected_revision}),
            true,
            operation(&headers)?,
            s.keys.as_ref(),
            s.key_id,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
