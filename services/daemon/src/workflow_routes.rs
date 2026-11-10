//! Project-scoped workflow metadata routes. Starts use the shared run endpoints.
use super::*;
use axum::{body::Bytes, extract::rejection::BytesRejection};
use cowork_workflow_contracts::{
    CreateWorkflow, PrepareWorkflowInputAnswer, PublishWorkflow, RevokeWorkflow,
    SaveWorkflowInputDraft, WorkflowInputAnswerIntent, WorkflowInputDraftLookup,
    WorkflowInputDraftSave, WorkflowInputDraftSnapshot, WorkflowLibraryEntry, WorkflowPage,
    WorkflowPublication, WorkflowRunView, WorkflowVersion,
};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/workflows", get(list).post(create))
        .route("/projects/{project}/workflows/{workflow}", get(read))
        .route(
            "/projects/{project}/workflows/{workflow}/versions/{version}",
            get(version),
        )
        .route(
            "/projects/{project}/workflows/{workflow}/versions",
            post(publish),
        )
        .route(
            "/projects/{project}/workflows/{workflow}/revoke",
            post(revoke),
        )
        .route("/runs/{run}/workflow", get(run_view))
        .route(
            "/runs/{run}/workflow/reconcile-effects",
            post(reconcile_effects),
        )
        .route(
            "/runs/{run}/workflow/inputs/{request}/draft",
            get(input_draft).put(save_input_draft),
        )
        .route(
            "/runs/{run}/workflow/inputs/{request}/draft/snapshots/{snapshot}",
            get(input_snapshot),
        )
        .route(
            "/runs/{run}/workflow/inputs/{request}/answer-intents",
            post(prepare_input),
        )
        .route(
            "/runs/{run}/workflow/input-answer-intents/{intent}",
            get(read_input_intent),
        )
        .route(
            "/runs/{run}/workflow/input-answer-intents/{intent}/commit",
            post(commit_input),
        )
}

#[derive(Deserialize)]
struct PageQuery {
    #[serde(default)]
    offset: u32,
    #[serde(default = "default_limit")]
    limit: u32,
}
fn default_limit() -> u32 {
    30
}
async fn list(
    State(s): State<AppState>,
    Path(project): Path<String>,
    Query(page): Query<PageQuery>,
) -> Answer<WorkflowPage> {
    Ok(Json(
        s.runtime
            .store
            .list_workflows(&project, page.offset, page.limit)
            .await?,
    ))
}
async fn create(
    State(s): State<AppState>,
    Path(project): Path<String>,
    body: Input<CreateWorkflow>,
) -> Answer<WorkflowLibraryEntry> {
    let request = input(body)?;
    Ok(Json(
        s.runtime
            .store
            .create_workflow(&project, &request.idempotency_key, &request.definition)
            .await?,
    ))
}
async fn read(
    State(s): State<AppState>,
    Path((project, workflow)): Path<(String, String)>,
) -> Answer<WorkflowLibraryEntry> {
    Ok(Json(s.runtime.store.workflow(&project, &workflow).await?))
}
async fn version(
    State(s): State<AppState>,
    Path((project, workflow, version)): Path<(String, String, String)>,
) -> Answer<WorkflowVersion> {
    Ok(Json(
        s.runtime
            .store
            .workflow_version(&project, &workflow, &version)
            .await?,
    ))
}
async fn publish(
    State(s): State<AppState>,
    Path((project, workflow)): Path<(String, String)>,
    body: Input<PublishWorkflow>,
) -> Answer<WorkflowPublication> {
    let request = input(body)?;
    Ok(Json(
        s.runtime
            .store
            .publish_workflow(
                &project,
                &workflow,
                request.base_revision,
                &request.idempotency_key,
                &request.definition,
            )
            .await?,
    ))
}
async fn revoke(
    State(s): State<AppState>,
    Path((project, workflow)): Path<(String, String)>,
    body: Input<RevokeWorkflow>,
) -> Answer<WorkflowPublication> {
    let request = input(body)?;
    Ok(Json(
        s.runtime
            .store
            .revoke_workflow(
                &project,
                &workflow,
                request.base_revision,
                &request.idempotency_key,
            )
            .await?,
    ))
}
async fn run_view(State(s): State<AppState>, Path(run): Path<String>) -> Answer<WorkflowRunView> {
    let parent = s.runtime.store.stored_run(&run).await?;
    Ok(Json(
        s.runtime
            .store
            .workflow_run_view(&parent.record.spec.project_id, &run)
            .await?,
    ))
}
async fn reconcile_effects(
    State(s): State<AppState>,
    Path(run): Path<String>,
) -> Answer<cowork_workflow_contracts::WorkflowEffectResolutionView> {
    let parent = s.runtime.store.stored_run(&run).await?;
    let result = s
        .runtime
        .reconcile_workflow_effects(&parent.record.spec.project_id, &run)
        .await?;
    Ok(Json(
        serde_json::from_value(result).map_err(cowork_domain::storage_error)?,
    ))
}
fn input_ids(ids: &[&str]) -> Result<(), ApiFailure> {
    for id in ids {
        cowork_domain::valid_text(id, 128)?;
    }
    Ok(())
}
async fn input_draft(
    State(s): State<AppState>,
    Path((run, request)): Path<(String, String)>,
) -> Answer<WorkflowInputDraftLookup> {
    input_ids(&[&run, &request])?;
    Ok(Json(
        s.runtime.store.workflow_input_draft(&run, &request).await?,
    ))
}
async fn save_input_draft(
    State(s): State<AppState>,
    Path((run, request)): Path<(String, String)>,
    body: Input<SaveWorkflowInputDraft>,
) -> Result<Response, ApiFailure> {
    input_ids(&[&run, &request])?;
    let result = s
        .runtime
        .store
        .save_workflow_input_draft(&run, &request, &input(body)?)
        .await?;
    let status = if matches!(&result, WorkflowInputDraftSave::Conflict { .. }) {
        StatusCode::CONFLICT
    } else {
        StatusCode::OK
    };
    Ok((status, Json(result)).into_response())
}
async fn input_snapshot(
    State(s): State<AppState>,
    Path((run, request, snapshot)): Path<(String, String, String)>,
) -> Answer<WorkflowInputDraftSnapshot> {
    input_ids(&[&run, &request, &snapshot])?;
    Ok(Json(
        s.runtime
            .store
            .workflow_input_draft_snapshot(&run, &request, &snapshot)
            .await?,
    ))
}
async fn prepare_input(
    State(s): State<AppState>,
    Path((run, request)): Path<(String, String)>,
    body: Input<PrepareWorkflowInputAnswer>,
) -> Answer<WorkflowInputAnswerIntent> {
    input_ids(&[&run, &request])?;
    Ok(Json(
        s.runtime
            .store
            .prepare_workflow_input_answer(&run, &request, &input(body)?)
            .await?,
    ))
}
async fn read_input_intent(
    State(s): State<AppState>,
    Path((run, intent)): Path<(String, String)>,
) -> Answer<WorkflowInputAnswerIntent> {
    input_ids(&[&run, &intent])?;
    Ok(Json(
        s.runtime
            .store
            .workflow_input_answer_intent(&run, &intent)
            .await?,
    ))
}
async fn commit_input(
    State(s): State<AppState>,
    Path((run, intent)): Path<(String, String)>,
    body: std::result::Result<Bytes, BytesRejection>,
) -> Answer<WorkflowInputAnswerIntent> {
    input_ids(&[&run, &intent])?;
    let bytes = body.map_err(|_| {
        ApiFailure(cowork_domain::Error::Validation(
            "Unreadable action body".into(),
        ))
    })?;
    if !bytes.is_empty() {
        return Err(cowork_domain::Error::Validation(
            "Answer commit accepts only the stored intent ID and no body".into(),
        )
        .into());
    }
    Ok(Json(
        s.runtime
            .store
            .commit_workflow_input_answer_intent(&run, &intent)
            .await?,
    ))
}
