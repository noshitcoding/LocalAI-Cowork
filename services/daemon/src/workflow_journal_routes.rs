//! Owner-authenticated durable editor and command metadata. No browser grants.
use super::*;
use axum::{body::Bytes, extract::rejection::BytesRejection};
use cowork_workflow_contracts::{
    PrepareWorkflowOperation, SaveWorkflowDraft, WorkflowDraft, WorkflowDraftPage,
    WorkflowDraftSave, WorkflowDraftSnapshot, WorkflowDraftSnapshotPage, WorkflowOperation,
    WorkflowOperationPage,
};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/workflow-journal/archive",
            post(archive),
        )
        .route(
            "/projects/{project}/workflow-drafts",
            get(drafts).put(save_draft),
        )
        .route("/projects/{project}/workflow-drafts/{draft}", get(draft))
        .route(
            "/projects/{project}/workflow-drafts/{draft}/snapshots",
            get(snapshots),
        )
        .route(
            "/projects/{project}/workflow-drafts/{draft}/snapshots/{snapshot}",
            get(snapshot),
        )
        .route(
            "/projects/{project}/workflow-operations",
            get(operations).post(prepare),
        )
        .route(
            "/projects/{project}/workflow-operations/{operation}",
            get(operation),
        )
        .route(
            "/projects/{project}/workflow-operations/{operation}/commit",
            post(commit),
        )
        .route(
            "/projects/{project}/workflow-operations/{operation}/discard",
            post(discard),
        )
        .route(
            "/projects/{project}/workflow-operations/{operation}/acknowledge",
            post(acknowledge),
        )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchiveJournal {
    before_ms: i64,
    keep_snapshots: u32,
}
async fn archive(
    State(s): State<AppState>,
    Path(project): Path<String>,
    body: Input<ArchiveJournal>,
) -> Answer<Value> {
    let selected = input(body)?;
    Ok(Json(
        s.runtime
            .store
            .archive_workflow_journal(&project, selected.before_ms, selected.keep_snapshots)
            .await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PageQuery {
    #[serde(default)]
    offset: u32,
    #[serde(default = "default_limit")]
    limit: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OperationPageQuery {
    #[serde(default)]
    offset: u32,
    #[serde(default = "default_limit")]
    limit: u32,
    #[serde(default)]
    include_acknowledged: bool,
}
fn default_limit() -> u32 {
    30
}
fn check_ids(values: &[&str]) -> std::result::Result<(), ApiFailure> {
    for value in values {
        cowork_domain::valid_text(value, 128)?;
    }
    Ok(())
}
type ActionBody = std::result::Result<Bytes, BytesRejection>;
fn empty_action(body: ActionBody) -> std::result::Result<(), ApiFailure> {
    let bytes =
        body.map_err(|_| ApiFailure(Error::Validation("Invalid journal action body".into())))?;
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(());
    }
    let object = serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&bytes)
        .map_err(|_| {
            ApiFailure(Error::Validation(
                "Journal actions accept only the operation identifier".into(),
            ))
        })?;
    if !object.is_empty() {
        return Err(ApiFailure(Error::Validation(
            "Journal actions accept only the operation identifier".into(),
        )));
    }
    Ok(())
}
async fn drafts(
    State(s): State<AppState>,
    Path(project): Path<String>,
    Query(page): Query<PageQuery>,
) -> Answer<WorkflowDraftPage> {
    check_ids(&[&project])?;
    Ok(Json(
        s.runtime
            .store
            .list_workflow_drafts(&project, page.offset, page.limit)
            .await?,
    ))
}
async fn save_draft(
    State(s): State<AppState>,
    Path(project): Path<String>,
    body: Input<SaveWorkflowDraft>,
) -> std::result::Result<Response, ApiFailure> {
    check_ids(&[&project])?;
    let result = s
        .runtime
        .store
        .save_workflow_draft(&project, &input(body)?)
        .await?;
    let status = match &result {
        WorkflowDraftSave::Saved { .. } => StatusCode::OK,
        WorkflowDraftSave::Conflict { .. } => StatusCode::CONFLICT,
    };
    // This conflict is a confirmed saved conflict snapshot, not an ApiError.
    Ok((status, Json(result)).into_response())
}
async fn draft(
    State(s): State<AppState>,
    Path((project, draft)): Path<(String, String)>,
) -> Answer<WorkflowDraft> {
    check_ids(&[&project, &draft])?;
    Ok(Json(
        s.runtime.store.workflow_draft(&project, &draft).await?,
    ))
}
async fn snapshots(
    State(s): State<AppState>,
    Path((project, draft)): Path<(String, String)>,
    Query(page): Query<PageQuery>,
) -> Answer<WorkflowDraftSnapshotPage> {
    check_ids(&[&project, &draft])?;
    Ok(Json(
        s.runtime
            .store
            .list_workflow_draft_snapshots(&project, &draft, page.offset, page.limit)
            .await?,
    ))
}
async fn snapshot(
    State(s): State<AppState>,
    Path((project, draft, snapshot)): Path<(String, String, String)>,
) -> Answer<WorkflowDraftSnapshot> {
    check_ids(&[&project, &draft, &snapshot])?;
    Ok(Json(
        s.runtime
            .store
            .workflow_draft_snapshot(&project, &draft, &snapshot)
            .await?,
    ))
}
async fn operations(
    State(s): State<AppState>,
    Path(project): Path<String>,
    Query(page): Query<OperationPageQuery>,
) -> Answer<WorkflowOperationPage> {
    check_ids(&[&project])?;
    Ok(Json(
        s.runtime
            .store
            .list_workflow_operations(&project, page.offset, page.limit, page.include_acknowledged)
            .await?,
    ))
}
async fn prepare(
    State(s): State<AppState>,
    Path(project): Path<String>,
    body: Input<PrepareWorkflowOperation>,
) -> Answer<WorkflowOperation> {
    check_ids(&[&project])?;
    Ok(Json(
        s.runtime
            .store
            .prepare_workflow_operation(&project, &input(body)?)
            .await?,
    ))
}
async fn operation(
    State(s): State<AppState>,
    Path((project, operation)): Path<(String, String)>,
) -> Answer<WorkflowOperation> {
    check_ids(&[&project, &operation])?;
    Ok(Json(
        s.runtime
            .store
            .workflow_operation(&project, &operation)
            .await?,
    ))
}
async fn commit(
    State(s): State<AppState>,
    Path((project, operation)): Path<(String, String)>,
    body: ActionBody,
) -> Answer<WorkflowOperation> {
    check_ids(&[&project, &operation])?;
    empty_action(body)?;
    Ok(Json(
        s.runtime
            .commit_workflow_operation(&project, &operation)
            .await?,
    ))
}
async fn discard(
    State(s): State<AppState>,
    Path((project, operation)): Path<(String, String)>,
    body: ActionBody,
) -> Answer<WorkflowOperation> {
    check_ids(&[&project, &operation])?;
    empty_action(body)?;
    Ok(Json(
        s.runtime
            .store
            .discard_workflow_operation(&project, &operation)
            .await?,
    ))
}
async fn acknowledge(
    State(s): State<AppState>,
    Path((project, operation)): Path<(String, String)>,
    body: ActionBody,
) -> Answer<WorkflowOperation> {
    check_ids(&[&project, &operation])?;
    empty_action(body)?;
    Ok(Json(
        s.runtime
            .store
            .acknowledge_workflow_operation(&project, &operation)
            .await?,
    ))
}
