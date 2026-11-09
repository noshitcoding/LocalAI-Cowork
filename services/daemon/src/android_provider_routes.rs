//! V1 provider mutations use authenticated identities and never the daemon owner's provider inventory.
use super::*;
pub(super) fn routes() -> Router<CompatState> {
    Router::new()
        .route("/provider-profiles", get(providers).post(create))
        .route(
            "/provider-profiles/{profile}",
            get(profile).put(update).delete(remove),
        )
        .route(
            "/provider-profiles/{profile}/secret",
            axum::routing::put(secret),
        )
}
fn operation(headers: &HeaderMap) -> Result<Option<&str>> {
    let value = headers
        .get("idempotency-key")
        .or_else(|| headers.get("x-operation-id"));
    value.map(|h| h.to_str().map_err(|_| invalid())).transpose()
}
async fn create(
    State(s): State<CompatState>,
    Session(a): Session,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Answer {
    let value = s
        .service
        .store()
        .android_personal_provider_mutate(
            &a,
            "create",
            None,
            body,
            operation(&headers)?,
            s.keys.as_ref(),
            s.key_id,
        )
        .await?;
    Ok(Json(value))
}
async fn profile(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
) -> Answer {
    Ok(Json(
        s.service.store().android_personal_provider(&a, id).await?,
    ))
}
async fn update(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_personal_provider_mutate(
                &a,
                "update",
                Some(id),
                body,
                operation(&headers)?,
                s.keys.as_ref(),
                s.key_id,
            )
            .await?,
    ))
}
async fn secret(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_personal_provider_mutate(
                &a,
                "secret",
                Some(id),
                body,
                operation(&headers)?,
                s.keys.as_ref(),
                s.key_id,
            )
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteQuery {
    expected_revision: i64,
}
async fn remove(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    Query(q): Query<DeleteQuery>,
    headers: HeaderMap,
) -> std::result::Result<StatusCode, Failure> {
    s.service
        .store()
        .android_personal_provider_mutate(
            &a,
            "delete",
            Some(id),
            json!({"expected_revision":q.expected_revision}),
            operation(&headers)?,
            s.keys.as_ref(),
            s.key_id,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
