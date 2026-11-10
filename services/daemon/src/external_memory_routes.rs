use super::*;
use crate::library_config_routes::{RequestOwner, authenticate_actor};
use axum::Extension;
fn routes() -> Router<Runtime> {
    Router::new()
        .route("/external-memory/providers", get(list))
        .route(
            "/external-memory/providers/{id}",
            axum::routing::put(write).delete(remove),
        )
        .route("/external-memory/providers/{id}/retrieve", post(retrieve))
        .layer(DefaultBodyLimit::max(32 * 1024))
}
pub(super) fn owner_router(runtime: Runtime) -> Router<AppState> {
    routes()
        .layer(Extension(RequestOwner::Local))
        .with_state(runtime)
}
pub(super) fn actor_router(runtime: Runtime) -> Router {
    let store = runtime.store.clone();
    Router::new()
        .nest("/api/v1", routes())
        .layer(middleware::from_fn_with_state(store, authenticate_actor))
        .with_state(runtime)
}
async fn list(
    State(r): State<Runtime>,
    Extension(o): Extension<RequestOwner>,
) -> Answer<Vec<ExternalMemoryProvider>> {
    Ok(Json(r.store.external_memory_providers(o.owner()).await?))
}
async fn write(
    State(r): State<Runtime>,
    Extension(o): Extension<RequestOwner>,
    Path(id): Path<String>,
    body: Input<WriteExternalMemoryProvider>,
) -> Answer<ExternalMemoryProvider> {
    Ok(Json(
        r.write_external_memory_provider(o.owner(), &id, &input(body)?)
            .await?,
    ))
}
async fn remove(
    State(r): State<Runtime>,
    Extension(o): Extension<RequestOwner>,
    Path(id): Path<String>,
    body: Input<DeleteExternalMemoryProvider>,
) -> Answer<Value> {
    Ok(Json(
        r.store
            .delete_external_memory_provider(o.owner(), &id, &input(body)?)
            .await?,
    ))
}
async fn retrieve(
    State(r): State<Runtime>,
    Extension(o): Extension<RequestOwner>,
    Path(id): Path<String>,
    body: Input<RetrieveExternalMemory>,
) -> Answer<ExternalMemoryRetrieval> {
    Ok(Json(
        r.retrieve_external_memory(o.owner(), &id, &input(body)?)
            .await?,
    ))
}
