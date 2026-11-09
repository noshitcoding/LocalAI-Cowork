use super::*;
pub(super) fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/model-registry", get(registry))
        .route("/model-context/preview", post(preview))
        .route("/model-context/compact", post(compact))
        .route("/orders/{order}/context-archives", get(archives))
        .route("/orders/{order}/context-archives/{archive}", get(archive))
        .merge(
            Router::new()
                .route("/native-model-registry/import", post(import))
                .route("/native-model-registry/revoke", post(revoke))
                .layer(middleware::from_fn_with_state(
                    state,
                    native_folders::authorize_native,
                )),
        )
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
}
async fn registry(State(s): State<AppState>) -> Answer<Vec<ModelRegistryView>> {
    Ok(Json(s.runtime.store.model_registry().await?))
}
async fn import(
    State(s): State<AppState>,
    body: Input<ImportModelQualification>,
) -> Answer<ModelRegistryView> {
    Ok(Json(
        s.runtime
            .store
            .import_model_qualification(&input(body)?)
            .await?,
    ))
}
async fn revoke(State(s): State<AppState>, body: Input<ModelRegistryRef>) -> Answer<Value> {
    s.runtime
        .store
        .revoke_model_qualification(&input(body)?)
        .await?;
    Ok(Json(json!({"revoked":true})))
}
async fn preview(
    State(s): State<AppState>,
    body: Input<PreviewModelContext>,
) -> Answer<ModelContextView> {
    Ok(Json(s.runtime.preview_model_context(&input(body)?).await?))
}
async fn compact(
    State(s): State<AppState>,
    body: Input<CompactModelContext>,
) -> Answer<ContextCompactionView> {
    Ok(Json(s.runtime.compact_model_context(&input(body)?).await?))
}
#[derive(Default, Deserialize)]
struct Page {
    #[serde(default)]
    after: u64,
}
async fn archives(
    State(s): State<AppState>,
    Path(order): Path<String>,
    Query(page): Query<Page>,
) -> Answer<Vec<ContextCompactionView>> {
    Ok(Json(
        s.runtime.store.context_archives(&order, page.after).await?,
    ))
}
async fn archive(
    State(s): State<AppState>,
    Path((order, archive)): Path<(String, String)>,
) -> Answer<ContextArchiveView> {
    Ok(Json(
        s.runtime.store.context_archive(&order, &archive).await?,
    ))
}
