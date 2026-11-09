//! Current Actor-private library; no owner bearer, native Runtime or synthetic team.
use super::*;
pub(super) fn routes() -> Router<CompatState> {
    Router::new()
        .route("/private-workflows", get(list).post(create))
        .route("/private-workflows/operations/{request}", get(operation))
        .route(
            "/private-workflows/{workflow}/versions",
            get(versions).post(version),
        )
        .route(
            "/private-workflows/{workflow}/versions/{version}",
            get(read_version),
        )
        .route("/private-workflows/{workflow}/publication", post(change))
        .route("/runs/{run}/workflow", get(run_view))
        .route("/runs/{run}/workflow/preflight", get(preflight))
        .route("/runs/{run}/workflow/results/{version}", get(result_bytes))
        .layer(DefaultBodyLimit::max(3 * 1024 * 1024))
}
#[derive(Deserialize)]
struct Page {
    project_id: Uuid,
    after: Option<Uuid>,
    #[serde(default = "page_limit")]
    limit: u16,
}
fn page_limit() -> u16 {
    25
}
#[derive(Deserialize)]
struct Project {
    project_id: Uuid,
}
#[derive(Deserialize)]
struct Versions {
    project_id: Uuid,
    #[serde(default)]
    after_revision: i64,
    #[serde(default = "page_limit")]
    limit: u16,
}
#[derive(Deserialize)]
struct Nodes {
    after: Option<String>,
    #[serde(default = "page_limit")]
    limit: u16,
}
async fn list(State(s): State<CompatState>, Session(a): Session, Query(q): Query<Page>) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .android_workflows(&a, q.project_id, q.after, q.limit)
            .await?
    )))
}
async fn create(
    State(s): State<CompatState>,
    Session(a): Session,
    Json(q): Json<AndroidWorkflowWrite>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .android_workflow_write(&a, None, &q, s.keys.as_ref(), s.key_id)
            .await?
    )))
}
async fn version(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    Json(q): Json<AndroidWorkflowWrite>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .android_workflow_write(&a, Some(id), &q, s.keys.as_ref(), s.key_id)
            .await?
    )))
}
async fn read_version(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((id, version)): Path<(Uuid, Uuid)>,
    Query(q): Query<Project>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .android_workflow_version(&a, q.project_id, id, version, s.keys.as_ref())
            .await?
    )))
}
async fn versions(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    Query(q): Query<Versions>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .android_workflow_versions(&a, q.project_id, id, q.after_revision, q.limit)
            .await?
    )))
}
async fn operation(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(request): Path<Uuid>,
    Query(q): Query<Project>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .android_workflow_operation(&a, q.project_id, request, s.keys.as_ref())
            .await?
    )))
}
async fn change(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    Query(q): Query<Project>,
    Json(body): Json<AndroidWorkflowChange>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .android_workflow_change(&a, q.project_id, id, &body, s.keys.as_ref(), s.key_id)
            .await?
    )))
}
async fn run_view(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(run): Path<Uuid>,
    Query(q): Query<Nodes>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .android_private_workflow_view(&a, run, q.after.as_deref(), q.limit, s.keys.as_ref())
            .await?
    )))
}
async fn preflight(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(run): Path<Uuid>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .android_private_workflow_preflight(&a, run, s.keys.as_ref())
            .await?
    )))
}
async fn result_bytes(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((run, version)): Path<(Uuid, Uuid)>,
) -> std::result::Result<Response, Failure> {
    let bytes = s
        .service
        .store()
        .android_private_workflow_result_bytes(&a, run, version, s.keys.as_ref())
        .await?;
    let sha = cowork_domain::hash(&bytes);
    let size = bytes.len();
    let mut response = Response::new(axum::body::Body::from(bytes.to_vec()));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        "application/json; charset=utf-8"
            .parse()
            .expect("static header"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "no-store".parse().expect("static header"),
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        size.to_string().parse().map_err(|_| invalid())?,
    );
    response.headers_mut().insert(
        "x-cowork-original-sha256",
        sha.parse().map_err(|_| invalid())?,
    );
    Ok(response)
}
