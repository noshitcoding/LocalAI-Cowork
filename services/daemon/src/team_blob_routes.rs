//! Independent persisted tenant sessions; owner/native bearer is never a Principal.
use crate::s3_store::S3Store;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, FromRequestParts, Path, Query, State},
    http::{HeaderValue, StatusCode, header, request::Parts},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use cowork_domain::{Error, hash};
use cowork_identity::{IdentityError, IdentityStore, Principal};
use cowork_storage::{
    RevisionRequest, Store, TEAM_BLOB_MAX_BYTES, TeamBlob, TeamBlobDelete, TeamBlobOperation,
    TeamBlobUpload, TeamBlobVersion,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
struct BlobState {
    store: Store,
    identity: IdentityStore,
    objects: Arc<S3Store>,
}
pub fn router(store: Store, identity: IdentityStore, objects: Arc<S3Store>) -> Router {
    Router::new()
        .route(
            "/api/v2/team-files/{team}/{project}",
            get(list).post(upload),
        )
        .route(
            "/api/v2/team-files/{team}/{project}/operations/{operation}",
            get(operation),
        )
        .route(
            "/api/v2/team-files/{team}/{project}/{blob}",
            get(download).delete(remove),
        )
        .route(
            "/api/v2/team-files/{team}/{project}/{blob}/versions",
            get(versions),
        )
        .route(
            "/api/v2/team-files/{team}/{project}/{blob}/reconcile",
            post(reconcile),
        )
        .layer(DefaultBodyLimit::max(TEAM_BLOB_MAX_BYTES as usize))
        .layer(middleware::from_fn(private_response))
        .with_state(BlobState {
            store,
            identity,
            objects,
        })
}
async fn private_response(request: axum::extract::Request, next: middleware::Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}
#[derive(Serialize)]
struct PublicFailure {
    error: &'static str,
}
fn failure(status: StatusCode, code: &'static str) -> Response {
    (
        status,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        Json(PublicFailure { error: code }),
    )
        .into_response()
}
struct Failure(Error);
impl From<Error> for Failure {
    fn from(e: Error) -> Self {
        Self(e)
    }
}
impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        match self.0 {
            Error::Validation(_) => failure(StatusCode::BAD_REQUEST, "invalid_request"),
            Error::NotFound | Error::Forbidden => failure(StatusCode::NOT_FOUND, "access_denied"),
            Error::Conflict => failure(StatusCode::CONFLICT, "revision_conflict"),
            Error::UncertainEffect => failure(StatusCode::CONFLICT, "uncertain_effect"),
            _ => failure(StatusCode::SERVICE_UNAVAILABLE, "file_storage_unavailable"),
        }
    }
}
type Answer<T> = std::result::Result<Json<T>, Failure>;
struct Session(Principal);
impl FromRequestParts<BlobState> for Session {
    type Rejection = Response;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &BlobState,
    ) -> std::result::Result<Self, Response> {
        let mut headers = parts.headers.get_all(header::AUTHORIZATION).iter();
        let bearer = headers
            .next()
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or_else(|| failure(StatusCode::UNAUTHORIZED, "authentication_required"))?;
        if headers.next().is_some() {
            return Err(failure(StatusCode::UNAUTHORIZED, "authentication_required"));
        }
        let actor = state
            .identity
            .authenticate_session(bearer)
            .await
            .map_err(|e| match e {
                IdentityError::Unauthenticated => {
                    failure(StatusCode::UNAUTHORIZED, "authentication_required")
                }
                _ => failure(StatusCode::SERVICE_UNAVAILABLE, "team_unavailable"),
            })?;
        let path = parts
            .extensions
            .get::<axum::extract::OriginalUri>()
            .map(|uri| uri.0.path())
            .unwrap_or(parts.uri.path());
        state
            .identity
            .android_management_request(&actor, parts.method.as_str(), path)
            .await
            .map_err(|_| failure(StatusCode::FORBIDDEN, "management_mfa_required"))?;
        Ok(Self(actor))
    }
}
async fn list(
    State(state): State<BlobState>,
    Session(actor): Session,
    Path((team, project)): Path<(Uuid, Uuid)>,
) -> Answer<Vec<TeamBlob>> {
    Ok(Json(
        state.store.team_blob_list(&actor, team, project).await?,
    ))
}
async fn operation(
    State(state): State<BlobState>,
    Session(actor): Session,
    Path((team, project, operation)): Path<(Uuid, Uuid, String)>,
) -> Answer<TeamBlobOperation> {
    Ok(Json(
        state
            .store
            .team_blob_operation(&actor, team, project, &operation)
            .await?,
    ))
}
async fn upload(
    State(state): State<BlobState>,
    Session(actor): Session,
    Path((team, project)): Path<(Uuid, Uuid)>,
    Query(input): Query<TeamBlobUpload>,
    body: Bytes,
) -> Answer<TeamBlob> {
    if body.len() as u64 > TEAM_BLOB_MAX_BYTES {
        return Err(Error::Validation("Team file exceeds 32 MiB".into()).into());
    }
    let plan = state
        .store
        .team_blob_prepare_upload(
            &actor,
            team,
            project,
            input,
            hash(&body),
            body.len() as u64,
            state.objects.backend_id().into(),
        )
        .await?;
    if !plan.should_send() {
        return Ok(Json(plan.metadata().clone()));
    }
    let version = plan.versions().first().ok_or(Error::Conflict)?;
    if let Err(error) = state.objects.put(version, &body).await {
        state.store.team_blob_uncertain(&plan).await?;
        return Err(error.into());
    }
    match state.store.team_blob_finish(&actor, &plan).await {
        Ok(value) => Ok(Json(value)),
        Err(error) => {
            state.store.team_blob_uncertain(&plan).await?;
            Err(error.into())
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VersionQuery {
    version: Option<i64>,
}
async fn download(
    State(state): State<BlobState>,
    Session(actor): Session,
    Path((team, project, blob)): Path<(Uuid, Uuid, Uuid)>,
    Query(query): Query<VersionQuery>,
) -> std::result::Result<Response, Failure> {
    let read = state
        .store
        .team_blob_read(&actor, team, project, blob, query.version)
        .await?;
    let body = state.objects.get(read.version()).await?;
    state.store.team_blob_read_finish(&actor, &read).await?;
    let content_type =
        HeaderValue::from_str(&read.metadata().media_type).map_err(|_| Error::Conflict)?;
    // Never interpolate a client filename into an HTTP header or local path.
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (
                header::CONTENT_DISPOSITION,
                HeaderValue::from_static("attachment; filename=team-file"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
            (
                header::ETAG,
                HeaderValue::from_str(&format!("\"{}\"", read.version().sha256))
                    .map_err(|_| Error::Conflict)?,
            ),
        ],
        body,
    )
        .into_response())
}
async fn versions(
    State(state): State<BlobState>,
    Session(actor): Session,
    Path((team, project, blob)): Path<(Uuid, Uuid, Uuid)>,
) -> Answer<Vec<TeamBlobVersion>> {
    Ok(Json(
        state
            .store
            .team_blob_versions(&actor, team, project, blob)
            .await?,
    ))
}
async fn remove(
    State(state): State<BlobState>,
    Session(actor): Session,
    Path((team, project, blob)): Path<(Uuid, Uuid, Uuid)>,
    Json(input): Json<TeamBlobDelete>,
) -> Answer<TeamBlob> {
    let plan = state
        .store
        .team_blob_prepare_delete(&actor, team, project, blob, input)
        .await?;
    if !plan.should_send() {
        return Ok(Json(plan.metadata().clone()));
    }
    for version in plan.versions() {
        if let Err(error) = state.objects.delete(version).await {
            state.store.team_blob_uncertain(&plan).await?;
            return Err(error.into());
        }
    }
    match state.store.team_blob_finish(&actor, &plan).await {
        Ok(value) => Ok(Json(value)),
        Err(error) => {
            state.store.team_blob_uncertain(&plan).await?;
            Err(error.into())
        }
    }
}
async fn reconcile(
    State(state): State<BlobState>,
    Session(actor): Session,
    Path((team, project, blob)): Path<(Uuid, Uuid, Uuid)>,
    Json(input): Json<RevisionRequest>,
) -> Answer<TeamBlob> {
    let plan = state
        .store
        .team_blob_reconciliation_plan(&actor, team, project, blob, input.expected_revision)
        .await?;
    if plan.is_upload() {
        for version in plan.versions() {
            state.objects.get(version).await?;
        }
    } else {
        for version in plan.versions() {
            if state.objects.exists(version).await? {
                return Err(Error::UncertainEffect.into());
            }
        }
    }
    Ok(Json(state.store.team_blob_finish(&actor, &plan).await?))
}
