//! Explicit native file selection, then reviewable user-content-only adoption.
//! Renderer bearer access alone cannot read an arbitrary source path.
use super::*;
use axum::Extension;
use cowork_storage::import_basis::{
    V1ImportContent, V1ImportPreview, V1ImportedAttachment, V1ImportedConfiguration,
    V1MappingOptions, V1MigrationMapping, V1MigrationSecrets, V1TeamImportPreview,
    V1TeamImportedHistory,
};
use cowork_toolbox::Toolbox;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectFile {
    path: std::path::PathBuf,
    kind: String,
    #[serde(default)]
    cold_source_confirmed: bool,
    #[serde(default = "desktop_family")]
    source_family: String,
    #[serde(default)]
    source_root: Option<std::path::PathBuf>,
    #[serde(default)]
    binary_files_confirmed: bool,
}
fn desktop_family() -> String {
    "desktop".into()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Commit {
    preview_id: String,
    plan_sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamCommit {
    preview_id: String,
    plan_sha256: String,
    #[serde(default)]
    secrets: V1MigrationSecrets,
}
#[derive(Clone)]
struct ImportTeamServices(Option<cowork_storage::team_service::TeamService>);

/// Merge these relative routes below `/api/v2/native`, inside owner bearer auth.
pub(super) fn router(state: AppState, service: Arc<Toolbox>) -> Router<AppState> {
    Router::new()
        .route("/import-v1/preview", post(preview))
        .route("/import-v1/commit", post(commit))
        .route("/import-v1/receipts", get(receipts))
        .layer(Extension(service))
        .layer(middleware::from_fn_with_state(
            state,
            native_folders::authorize_native,
        ))
}

async fn preview(
    State(state): State<AppState>,
    Extension(service): Extension<Arc<Toolbox>>,
    body: Input<SelectFile>,
) -> Answer<V1ImportPreview> {
    let selected = input(body)?;
    if selected.source_root.is_some() != selected.binary_files_confirmed {
        return Err(ApiFailure(Error::Validation(
            "Explicit binary source-root consent required".into(),
        )));
    }
    let content = tokio::task::spawn_blocking(move || {
        service.capture_v1_content_with_options(
            &selected.path,
            &selected.kind,
            selected.cold_source_confirmed,
            &selected.source_family,
            selected.source_root.as_deref(),
        )
    })
    .await
    .map_err(|_| ApiFailure(Error::UncertainEffect))?
    .map_err(|error| ApiFailure(Error::Validation(error.to_string())))?;
    let content: V1ImportContent = serde_json::from_value(content).map_err(|_| {
        ApiFailure(Error::Validation(
            "Invalid selected V1 content projection".into(),
        ))
    })?;
    Ok(Json(state.runtime.store.preview_v1_import(content).await?))
}
async fn commit(State(state): State<AppState>, body: Input<Commit>) -> Answer<V1ImportPreview> {
    let selected = input(body)?;
    Ok(Json(
        state
            .runtime
            .store
            .commit_v1_import(&selected.preview_id, &selected.plan_sha256)
            .await?,
    ))
}
async fn receipts(State(state): State<AppState>) -> Answer<Vec<V1ImportPreview>> {
    Ok(Json(state.runtime.store.v1_import_receipts().await?))
}

/// Ordinary owner read authority. These routes only return committed DB bytes.
pub(super) fn content_router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/orders/{order}/imported-attachments",
            get(attachment_list),
        )
        .route(
            "/projects/{project}/orders/{order}/imported-attachments/{attachment}",
            get(attachment_download),
        )
        .route(
            "/projects/{project}/import-v1/configurations",
            get(configurations),
        )
}
async fn attachment_list(
    State(state): State<AppState>,
    Path((project, order)): Path<(String, String)>,
) -> Answer<Vec<V1ImportedAttachment>> {
    Ok(Json(
        state
            .runtime
            .store
            .v1_imported_attachments(&project, &order)
            .await?,
    ))
}
fn attachment_response(
    metadata: V1ImportedAttachment,
    bytes: Vec<u8>,
) -> std::result::Result<Response, ApiFailure> {
    let filename: String = metadata
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    Response::builder()
        .header(header::CONTENT_TYPE, metadata.media_type)
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .header("x-content-type-options", "nosniff")
        .header(header::CACHE_CONTROL, "no-store")
        .body(axum::body::Body::from(bytes))
        .map_err(|_| ApiFailure(Error::Conflict))
}
async fn attachment_download(
    State(state): State<AppState>,
    Path((project, order, attachment)): Path<(String, String, String)>,
) -> std::result::Result<Response, ApiFailure> {
    let (metadata, bytes) = state
        .runtime
        .store
        .v1_imported_attachment(&project, &order, &attachment)
        .await?;
    attachment_response(metadata, bytes)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamPreview {
    source_preview_id: String,
    source_plan_sha256: String,
    #[serde(default)]
    mapping: V1MigrationMapping,
}
/// Mount below /api/v2/teams outside owner-bearer auth. Source preview additionally
/// requires the private native header and rejects a browser Origin.
pub(super) fn team_router(
    state: AppState,
    service: Option<cowork_storage::team_service::TeamService>,
) -> Router<AppState> {
    let preview_route = Router::new()
        .route(
            "/{team}/projects/{project}/import-v1/preview",
            post(team_preview),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            native_folders::authorize_native,
        ));
    preview_route
        .merge(
            Router::new()
                .route(
                    "/{team}/projects/{project}/import-v1/commit",
                    post(team_commit),
                )
                .route(
                    "/{team}/projects/{project}/import-v1/receipts",
                    get(team_receipts),
                )
                .route(
                    "/{team}/projects/{project}/orders/{order}/imported-history",
                    get(team_history),
                )
                .route(
                    "/{team}/projects/{project}/import-v1/attachments/{attachment}",
                    get(team_attachment),
                )
                .route("/{team}/import-v1/options", get(team_options))
                .route(
                    "/{team}/projects/{project}/import-v1/configurations",
                    get(team_configurations),
                ),
        )
        .layer(DefaultBodyLimit::max(4 * 1024 * 1024))
        .layer(Extension(ImportTeamServices(service)))
        .layer(middleware::from_fn_with_state(state, team_auth))
}
async fn team_auth(State(state): State<AppState>, mut request: Request, next: Next) -> Response {
    let mut values = request.headers().get_all(header::AUTHORIZATION).iter();
    let Some(token) = values
        .next()
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        return ApiFailure(Error::Forbidden).into_response();
    };
    if values.next().is_some() {
        return ApiFailure(Error::Forbidden).into_response();
    }
    let principal = match state
        .runtime
        .store
        .identity()
        .authenticate_session(token)
        .await
    {
        Ok(value) => value,
        Err(error) => {
            return ApiFailure(cowork_storage::team_service::identity_error(error)).into_response();
        }
    };
    request.extensions_mut().insert(principal);
    next.run(request).await
}
async fn team_preview(
    State(state): State<AppState>,
    Extension(actor): Extension<cowork_identity::Principal>,
    Path((team, project)): Path<(uuid::Uuid, uuid::Uuid)>,
    body: Input<TeamPreview>,
) -> Answer<V1TeamImportPreview> {
    let selected = input(body)?;
    Ok(Json(
        state
            .runtime
            .store
            .preview_v1_team_import_mapped(
                &actor,
                team,
                project,
                &selected.source_preview_id,
                &selected.source_plan_sha256,
                selected.mapping,
            )
            .await?,
    ))
}
async fn team_commit(
    State(state): State<AppState>,
    Extension(service): Extension<ImportTeamServices>,
    Extension(actor): Extension<cowork_identity::Principal>,
    Path((team, project)): Path<(uuid::Uuid, uuid::Uuid)>,
    body: Input<TeamCommit>,
) -> Answer<V1TeamImportPreview> {
    let selected = input(body)?;
    Ok(Json(
        state
            .runtime
            .store
            .commit_v1_team_import_complete(
                &actor,
                team,
                project,
                &selected.preview_id,
                &selected.plan_sha256,
                service.0.as_ref(),
                selected.secrets,
            )
            .await?,
    ))
}
async fn configurations(
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Answer<Vec<V1ImportedConfiguration>> {
    Ok(Json(
        state
            .runtime
            .store
            .v1_imported_configurations(&project)
            .await?,
    ))
}
async fn team_configurations(
    State(state): State<AppState>,
    Extension(actor): Extension<cowork_identity::Principal>,
    Path((team, project)): Path<(uuid::Uuid, uuid::Uuid)>,
) -> Answer<Vec<V1ImportedConfiguration>> {
    Ok(Json(
        state
            .runtime
            .store
            .v1_team_imported_configurations(&actor, team, project)
            .await?,
    ))
}
async fn team_options(
    State(state): State<AppState>,
    Extension(actor): Extension<cowork_identity::Principal>,
    Path(team): Path<uuid::Uuid>,
) -> Answer<V1MappingOptions> {
    Ok(Json(
        state
            .runtime
            .store
            .v1_team_migration_options(&actor, team)
            .await?,
    ))
}
async fn team_receipts(
    State(state): State<AppState>,
    Extension(actor): Extension<cowork_identity::Principal>,
    Path((team, project)): Path<(uuid::Uuid, uuid::Uuid)>,
) -> Answer<Vec<V1TeamImportPreview>> {
    Ok(Json(
        state
            .runtime
            .store
            .v1_team_import_receipts(&actor, team, project)
            .await?,
    ))
}
async fn team_history(
    State(state): State<AppState>,
    Extension(actor): Extension<cowork_identity::Principal>,
    Path((team, project, order)): Path<(uuid::Uuid, uuid::Uuid, uuid::Uuid)>,
) -> Answer<V1TeamImportedHistory> {
    Ok(Json(
        state
            .runtime
            .store
            .v1_team_imported_history(&actor, team, project, order)
            .await?,
    ))
}
async fn team_attachment(
    State(state): State<AppState>,
    Extension(actor): Extension<cowork_identity::Principal>,
    Path((team, project, attachment)): Path<(uuid::Uuid, uuid::Uuid, String)>,
) -> std::result::Result<Response, ApiFailure> {
    let (metadata, bytes) = state
        .runtime
        .store
        .v1_team_imported_attachment(&actor, team, project, &attachment)
        .await?;
    attachment_response(metadata, bytes)
}
