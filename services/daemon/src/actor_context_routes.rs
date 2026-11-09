use super::*;
pub(super) fn routes() -> Router<CompatState> {
    Router::new()
        .route("/context/skills/{id}/learning", get(learning))
        .route("/context/{kind}", get(list))
        .route(
            "/context/{kind}/{id}",
            axum::routing::put(write).delete(remove),
        )
}
#[derive(Deserialize)]
struct LearningScope {
    project_id: Uuid,
}
async fn learning(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    Query(q): Query<LearningScope>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .actor_skill_learning(&a, q.project_id, id, s.keys.as_ref())
            .await?
    )))
}
#[derive(Deserialize)]
struct Scope {
    project_id: Option<Uuid>,
}
async fn list(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(kind): Path<String>,
    Query(q): Query<Scope>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .actor_context(&a, &kind, q.project_id)
            .await?
    )))
}
async fn write(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((kind, id)): Path<(String, Uuid)>,
    Json(body): Json<Value>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .actor_context_mutate(&a, &kind, id, &body, false)
            .await?
    )))
}
async fn remove(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((kind, id)): Path<(String, Uuid)>,
    Json(body): Json<Value>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .actor_context_mutate(&a, &kind, id, &body, true)
            .await?
    )))
}
