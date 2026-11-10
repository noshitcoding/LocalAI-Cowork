//! Owner-only productivity routes; never mounted as an actor-session fallback.
use super::*;
use cowork_storage::{KnowledgeImportRequest, KnowledgeImportResult};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/knowledge/import", post(import))
        .layer(DefaultBodyLimit::max(4_194_304))
}
pub(super) fn extract_router(service: Arc<cowork_toolbox::Toolbox>) -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/knowledge/extract", post(extract))
        .layer(DefaultBodyLimit::max(16 * 1024 * 1024))
        .layer(axum::Extension(service))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExtractQuery {
    operation_id: String,
    file_name: String,
}
async fn extract(
    State(state): State<AppState>,
    axum::Extension(service): axum::Extension<Arc<cowork_toolbox::Toolbox>>,
    Path(project): Path<String>,
    Query(query): Query<ExtractQuery>,
    body: axum::body::Bytes,
) -> Answer<cowork_toolbox::KnowledgeExtraction> {
    state.runtime.store.project_settings(&project).await?;
    let value = tokio::task::spawn_blocking(move || {
        service.extract_knowledge(&project, &query.operation_id, &query.file_name, &body)
    })
    .await
    .map_err(|_| Error::UncertainEffect)?
    .map_err(|error| match error {
        cowork_toolbox::Error::Conflict => Error::Conflict,
        cowork_toolbox::Error::Invalid(message) => Error::Validation(message),
        _ => Error::UncertainEffect,
    })?;
    Ok(Json(value))
}
async fn import(
    State(state): State<AppState>,
    Path(project): Path<String>,
    body: Input<KnowledgeImportRequest>,
) -> Answer<KnowledgeImportResult> {
    Ok(Json(
        state
            .runtime
            .store
            .import_knowledge(&project, input(body)?)
            .await?,
    ))
}
