use super::*;
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/providers/discover-models", post(discover))
        .route("/providers/{provider}", put(update).delete(remove))
        .route("/providers/{provider}/settings", get(settings))
        .route("/providers/{provider}/models", get(models))
        .route("/providers/{provider}/test", post(test))
        .layer(DefaultBodyLimit::max(32 * 1024))
}
async fn discover(State(s): State<AppState>, body: Input<DiscoverProviderModels>) -> Answer<Value> {
    Ok(Json(
        json!({"models":s.runtime.discover_provider_models(&input(body)?).await?}),
    ))
}
async fn settings(State(s): State<AppState>, Path(id): Path<String>) -> Answer<ProviderSettings> {
    Ok(Json(s.runtime.provider_settings(&id).await?))
}
async fn update(
    State(s): State<AppState>,
    Path(id): Path<String>,
    body: Input<UpdateProviderSettings>,
) -> Answer<ProviderSettings> {
    Ok(Json(
        s.runtime
            .update_provider_settings(&id, &input(body)?)
            .await?,
    ))
}
async fn remove(
    State(s): State<AppState>,
    Path(id): Path<String>,
    body: Input<DeleteProviderSettings>,
) -> Answer<Value> {
    s.runtime
        .delete_provider_settings(&id, &input(body)?)
        .await?;
    Ok(Json(json!({"deleted":true})))
}
async fn models(State(s): State<AppState>, Path(id): Path<String>) -> Answer<Value> {
    Ok(Json(
        json!({"models":s.runtime.provider_model_catalog(&id).await?}),
    ))
}
async fn test(State(s): State<AppState>, Path(id): Path<String>) -> Answer<Value> {
    s.runtime.test_provider_settings(&id).await?;
    Ok(Json(json!({"ready":true})))
}
