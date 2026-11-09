//! Order workspace transfers use persisted team sessions and current ACLs.
use crate::team_toolbox::TeamToolbox;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, FromRequestParts, Path, Query, State},
    http::{StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::get,
};
use cowork_domain::Error;
use cowork_identity::{ContentAction, IdentityStore, Principal};
use cowork_storage::team_service::TeamService;
use cowork_toolbox::Toolbox;
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
struct FileState {
    service: TeamService,
    identity: IdentityStore,
    toolbox: Arc<Toolbox>,
    tools: Arc<TeamToolbox>,
}
pub(crate) fn router(
    service: TeamService,
    identity: IdentityStore,
    toolbox: Arc<Toolbox>,
    tools: Arc<TeamToolbox>,
) -> Router {
    Router::new()
        .route(
            "/api/v2/team-tool-files/{team}/{project}/{order}",
            get(download).put(upload),
        )
        .layer(DefaultBodyLimit::max(16 * 1024 * 1024))
        .with_state(FileState {
            service,
            identity,
            toolbox,
            tools,
        })
}
fn failure(status: StatusCode, code: &'static str) -> Response {
    (
        status,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        Json(serde_json::json!({"error":code})),
    )
        .into_response()
}
struct Failure(Error);
impl From<Error> for Failure {
    fn from(value: Error) -> Self {
        Self(value)
    }
}
impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        match self.0 {
            Error::NotFound | Error::Forbidden => failure(StatusCode::NOT_FOUND, "access_denied"),
            Error::Validation(_) => failure(StatusCode::BAD_REQUEST, "invalid_file_request"),
            Error::Conflict | Error::UncertainEffect => {
                failure(StatusCode::CONFLICT, "workspace_busy_or_uncertain")
            }
            _ => failure(StatusCode::SERVICE_UNAVAILABLE, "workspace_unavailable"),
        }
    }
}
struct Session(Principal);
impl FromRequestParts<FileState> for Session {
    type Rejection = Response;
    async fn from_request_parts(parts: &mut Parts, state: &FileState) -> Result<Self, Response> {
        let mut values = parts.headers.get_all(header::AUTHORIZATION).iter();
        let bearer = values
            .next()
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .filter(|value| !value.is_empty())
            .ok_or_else(|| failure(StatusCode::UNAUTHORIZED, "authentication_required"))?;
        if values.next().is_some() {
            return Err(failure(StatusCode::UNAUTHORIZED, "authentication_required"));
        }
        state
            .identity
            .authenticate_session(bearer)
            .await
            .map(Self)
            .map_err(|_| failure(StatusCode::UNAUTHORIZED, "authentication_required"))
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileQuery {
    path: String,
}
fn io_error(_: cowork_toolbox::Error) -> Error {
    Error::Conflict
}
async fn download(
    State(state): State<FileState>,
    Session(actor): Session,
    Path((team, project, order)): Path<(Uuid, Uuid, Uuid)>,
    Query(query): Query<FileQuery>,
) -> Result<Response, Failure> {
    state
        .service
        .project_authorize(&actor, team, project, ContentAction::Read)
        .await?;
    let lease = state.tools.file_workspace_lease(order)?;
    let bytes = state
        .service
        .order_file_io(&actor, team, project, order, ContentAction::Read, || {
            state
                .toolbox
                .read_output_file_with_lease(&order.to_string(), &query.path, lease)
                .map_err(io_error)
        })
        .await?;
    state
        .service
        .project_authorize(&actor, team, project, ContentAction::Read)
        .await?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CONTENT_DISPOSITION, "attachment"),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        bytes,
    )
        .into_response())
}
async fn upload(
    State(state): State<FileState>,
    Session(actor): Session,
    Path((team, project, order)): Path<(Uuid, Uuid, Uuid)>,
    Query(query): Query<FileQuery>,
    body: Bytes,
) -> Result<Json<serde_json::Value>, Failure> {
    state
        .service
        .project_authorize(&actor, team, project, ContentAction::Write)
        .await?;
    let lease = state.tools.file_workspace_lease(order)?;
    state
        .service
        .order_file_io(&actor, team, project, order, ContentAction::Write, || {
            state
                .toolbox
                .write_input_file_with_lease(&order.to_string(), &query.path, &body, lease)
                .map_err(io_error)
        })
        .await?;
    Ok(Json(
        serde_json::json!({"path":query.path,"bytes":body.len()}),
    ))
}
