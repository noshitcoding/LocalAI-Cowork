//! These routes are merged under the personal-owner bearer/origin middleware.
//! Team sessions must not use this module as a substitute for tenant ACL checks.
use super::*;
use axum::{body::Bytes, http::HeaderMap, routing::delete};
use cowork_storage::*;
use serde_json::{Value, json};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/skills", get(skills).post(write_skill))
        .route("/projects/{project}/skills/{object}", delete(delete_skill))
        .route(
            "/projects/{project}/skills/{object}/revisions",
            get(skill_revisions),
        )
        .route(
            "/projects/{project}/skills/{object}/learning",
            get(skill_learning),
        )
        .route(
            "/projects/{project}/knowledge",
            get(knowledge).post(write_knowledge),
        )
        .route(
            "/projects/{project}/knowledge/{object}",
            delete(delete_knowledge),
        )
        .route(
            "/projects/{project}/knowledge/{object}/revisions",
            get(knowledge_revisions),
        )
        .route(
            "/projects/{project}/schedules",
            get(schedules).post(write_schedule),
        )
        .route(
            "/projects/{project}/schedules/{schedule}",
            delete(delete_schedule),
        )
        .route(
            "/projects/{project}/schedules/{schedule}/dispatch",
            post(dispatch_schedule),
        )
        .route(
            "/projects/{project}/schedules/{schedule}/history",
            get(schedule_history),
        )
        .route(
            "/projects/{project}/deleted-schedules",
            get(deleted_schedules),
        )
        .route(
            "/projects/{project}/schedules/{schedule}/abandon-uncertain",
            post(abandon_uncertain),
        )
        .route("/projects/{project}/sync", get(sync))
        .route("/projects/{project}/export", get(export_project))
        .route(
            "/projects/{project}/orders/{order}/imported-history",
            get(imported_history),
        )
        .route("/workspace/export", get(export_workspace))
        .route("/workspace/import", post(import_json))
        .route("/workspace/import-zip", post(import_zip))
        .route("/devices/protocol", get(protocol))
        .route("/devices", get(devices).post(register_device))
        .route("/devices/{device}/revoke", post(revoke_device))
        .route(
            "/devices/{device}/queue",
            get(offline_queue).post(enqueue_offline),
        )
        .route(
            "/devices/{device}/queue/{operation}/commit",
            post(commit_offline),
        )
        .route(
            "/devices/{device}/queue/{operation}",
            delete(discard_offline),
        )
        .layer(DefaultBodyLimit::max(PRODUCTIVITY_ARCHIVE_MAX_BYTES))
}
async fn skills(State(state): State<AppState>, Path(project): Path<String>) -> Answer<Vec<Skill>> {
    Ok(Json(
        state.runtime.store.productivity_skills(&project).await?,
    ))
}
async fn write_skill(
    State(state): State<AppState>,
    Path(project): Path<String>,
    body: Input<WriteSkill>,
) -> Answer<Skill> {
    Ok(Json(
        state
            .runtime
            .store
            .productivity_write_skill(&project, input(body)?)
            .await?,
    ))
}
async fn delete_skill(
    State(state): State<AppState>,
    Path((project, object)): Path<(String, String)>,
    body: Input<RevisionRequest>,
) -> Answer<Value> {
    state
        .runtime
        .store
        .productivity_delete_object(&project, &object, "skill", input(body)?.expected_revision)
        .await?;
    Ok(Json(json!({"deleted":true})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KnowledgeQuery {
    #[serde(default)]
    query: String,
    kind: Option<KnowledgeKind>,
    limit: Option<usize>,
}
async fn knowledge(
    State(state): State<AppState>,
    Path(project): Path<String>,
    Query(query): Query<KnowledgeQuery>,
) -> Answer<Vec<KnowledgeHit>> {
    Ok(Json(
        state
            .runtime
            .store
            .productivity_knowledge(
                &project,
                &query.query,
                query.kind,
                query.limit.unwrap_or(50),
            )
            .await?,
    ))
}
async fn write_knowledge(
    State(state): State<AppState>,
    Path(project): Path<String>,
    body: Input<WriteKnowledge>,
) -> Answer<KnowledgeDocument> {
    Ok(Json(
        state
            .runtime
            .store
            .productivity_write_knowledge(&project, input(body)?)
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KnowledgeRevisionQuery {
    #[serde(default)]
    after: i64,
    limit: Option<i64>,
}
async fn knowledge_revisions(
    State(state): State<AppState>,
    Path((project, object)): Path<(String, String)>,
    Query(query): Query<KnowledgeRevisionQuery>,
) -> Answer<Vec<KnowledgeRevision>> {
    Ok(Json(
        state
            .runtime
            .store
            .knowledge_revisions(&project, &object, query.after, query.limit.unwrap_or(100))
            .await?,
    ))
}
async fn delete_knowledge(
    State(state): State<AppState>,
    Path((project, object)): Path<(String, String)>,
    body: Input<RevisionRequest>,
) -> Answer<Value> {
    state
        .runtime
        .store
        .productivity_delete_object(
            &project,
            &object,
            "knowledge",
            input(body)?.expected_revision,
        )
        .await?;
    Ok(Json(json!({"deleted":true})))
}
async fn schedules(
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Answer<Vec<ProductSchedule>> {
    Ok(Json(
        state.runtime.store.productivity_schedules(&project).await?,
    ))
}
async fn write_schedule(
    State(state): State<AppState>,
    Path(project): Path<String>,
    headers: HeaderMap,
    body: Input<WriteSchedule>,
) -> Answer<ProductSchedule> {
    let request = input(body)?;
    if let Some(key) = headers.get("Idempotency-Key")
        && let Some(original) = state
            .runtime
            .store
            .native_schedule_original(
                &project,
                &request,
                key.to_str()
                    .map_err(|_| Error::Validation("Invalid original key".into()))?,
            )
            .await?
    {
        return Ok(Json(original));
    }
    if !request.paused {
        require_schedule_provider(&state.runtime, &request.template).await?;
    }
    let saved = if let Some(key) = headers.get("Idempotency-Key") {
        state
            .runtime
            .store
            .productivity_write_schedule_idempotent(
                &project,
                request,
                key.to_str()
                    .map_err(|_| Error::Validation("Invalid original key".into()))?,
            )
            .await?
    } else {
        state
            .runtime
            .store
            .productivity_write_schedule(&project, request)
            .await?
    };
    Ok(Json(saved))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeScheduleHistoryQuery {
    before: Option<i64>,
    limit: Option<i64>,
}
async fn skill_revisions(
    State(state): State<AppState>,
    Path((project, skill)): Path<(String, String)>,
    Query(query): Query<NativeScheduleHistoryQuery>,
) -> Answer<NativeSkillHistory> {
    Ok(Json(
        state
            .runtime
            .store
            .native_skill_history(&project, &skill, query.before, query.limit.unwrap_or(50))
            .await?,
    ))
}
async fn schedule_history(
    State(state): State<AppState>,
    Path((project, schedule)): Path<(String, String)>,
    Query(query): Query<NativeScheduleHistoryQuery>,
) -> Answer<NativeScheduleHistory> {
    Ok(Json(
        state
            .runtime
            .store
            .native_schedule_history(&project, &schedule, query.before, query.limit.unwrap_or(50))
            .await?,
    ))
}
async fn deleted_schedules(
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Answer<Vec<NativeDeletedSchedule>> {
    Ok(Json(
        state
            .runtime
            .store
            .native_deleted_schedules(&project)
            .await?,
    ))
}
async fn require_schedule_provider(
    runtime: &Runtime,
    template: &ScheduledRunTemplate,
) -> std::result::Result<(), Error> {
    if let Some(request) = &template.workflow {
        if request.order_id != template.order_id {
            return Err(Error::Forbidden);
        }
        let mut request = request.clone();
        request.base_revision = runtime.store.order(&template.order_id).await?.revision;
        request.prompt = template.prompt.clone();
        if template.agent_config.is_some() {
            request.agent_config = template.agent_config.clone();
        }
        runtime.preflight_workflow(&request).await?;
        return Ok(());
    }
    let profiles = runtime.profiles().await;
    let profile = profiles
        .iter()
        .find(|p| p.id == template.provider_id)
        .ok_or_else(|| Error::Provider("profile_missing".into()))?;
    if profile.revision != template.provider_revision {
        return Err(Error::Conflict);
    }
    if !profile.credential_ready {
        return Err(Error::Provider("credential_missing".into()));
    }
    Ok(())
}
async fn delete_schedule(
    State(state): State<AppState>,
    Path((project, key)): Path<(String, String)>,
    body: Input<RevisionRequest>,
) -> Answer<Value> {
    state
        .runtime
        .store
        .productivity_delete_schedule(&project, &key, input(body)?.expected_revision)
        .await?;
    Ok(Json(json!({"deleted":true})))
}
async fn dispatch_claim(
    runtime: &Runtime,
    schedule: ProductSchedule,
) -> std::result::Result<ProductSchedule, Error> {
    let active = schedule.active_occurrence.as_ref().ok_or(Error::Conflict)?;
    let result = if let Some(request) = &active.workflow_request {
        runtime.create_product_workflow_run(request).await
    } else {
        runtime.create_run(&active.request).await
    };
    match result {
        Ok(run) => {
            runtime
                .store
                .productivity_schedule_dispatch_result(
                    &schedule.project_id,
                    &schedule.id,
                    &active.id,
                    Some(&run),
                )
                .await
        }
        Err(error) => {
            // Even an apparent local error may follow a lost commit ACK. Preserve the
            // immutable request and reconcile it by reading storage, never retry here.
            runtime
                .store
                .productivity_schedule_dispatch_result(
                    &schedule.project_id,
                    &schedule.id,
                    &active.id,
                    None,
                )
                .await?;
            runtime.store.productivity_reconcile_schedules().await?;
            Err(error)
        }
    }
}
async fn dispatch_schedule(
    State(state): State<AppState>,
    Path((project, key)): Path<(String, String)>,
    body: Input<RevisionRequest>,
) -> Answer<ProductSchedule> {
    let revision = input(body)?.expected_revision;
    let claim = state
        .runtime
        .store
        .productivity_claim_schedule(Some(&project), Some(&key), true, Some(revision))
        .await?
        .ok_or(Error::Conflict)?;
    Ok(Json(dispatch_claim(&state.runtime, claim).await?))
}
async fn abandon_uncertain(
    State(state): State<AppState>,
    Path((project, key)): Path<(String, String)>,
    body: Input<RevisionRequest>,
) -> Answer<ProductSchedule> {
    Ok(Json(
        state
            .runtime
            .store
            .productivity_abandon_uncertain_schedule(&project, &key, input(body)?.expected_revision)
            .await?,
    ))
}
/// Call from the daemon's existing shutdown-aware periodic worker. One occurrence
/// is durably claimed before any Runtime call, with no overlapping active run.
pub async fn schedule_tick(runtime: &Runtime) -> std::result::Result<usize, Error> {
    runtime.store.productivity_reconcile_schedules().await?;
    let mut dispatched = 0;
    for _ in 0..8 {
        let Some(claim) = runtime
            .store
            .productivity_claim_schedule(None, None, false, None)
            .await?
        else {
            break;
        };
        match dispatch_claim(runtime, claim).await {
            Ok(_) => dispatched += 1,
            Err(error) => tracing::warn!(
                code = error.code(),
                "Scheduled occurrence paused for review"
            ),
        }
    }
    Ok(dispatched)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SyncQuery {
    after: Option<i64>,
    limit: Option<i64>,
}
async fn sync(
    State(state): State<AppState>,
    Path(project): Path<String>,
    Query(query): Query<SyncQuery>,
) -> Answer<ProductivitySyncPage> {
    Ok(Json(
        state
            .runtime
            .store
            .productivity_sync(
                &project,
                query.after.unwrap_or(0),
                query.limit.unwrap_or(100),
            )
            .await?,
    ))
}
fn zip_response(archive: ProductivityArchive) -> std::result::Result<Response, ApiFailure> {
    let bytes = archive.to_zip()?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/zip"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=localai-cowork-projects.zip",
            ),
        ],
        bytes,
    )
        .into_response())
}
async fn export_project(
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> std::result::Result<Response, ApiFailure> {
    zip_response(
        state
            .runtime
            .store
            .productivity_export(Some(&project))
            .await?,
    )
}
async fn export_workspace(
    State(state): State<AppState>,
) -> std::result::Result<Response, ApiFailure> {
    zip_response(state.runtime.store.productivity_export(None).await?)
}
async fn import_json(
    State(state): State<AppState>,
    body: Input<ProductivityImportRequest>,
) -> Answer<ProductivityImportResult> {
    Ok(Json(
        state
            .runtime
            .store
            .productivity_import(input(body)?)
            .await?,
    ))
}
async fn imported_history(
    State(state): State<AppState>,
    Path((project, order)): Path<(String, String)>,
) -> Answer<Option<ImportedProductHistory>> {
    Ok(Json(
        state
            .runtime
            .store
            .productivity_imported_history(&project, &order)
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ZipImportQuery {
    idempotency_key: String,
    policy: ProductivityImportPolicy,
}
async fn import_zip(
    State(state): State<AppState>,
    Query(query): Query<ZipImportQuery>,
    body: Bytes,
) -> Answer<ProductivityImportResult> {
    if query.policy == ProductivityImportPolicy::Replace {
        return Err(Error::Validation(
            "Replacing projects requires explicit JSON revision expectations".into(),
        )
        .into());
    }
    let archive = ProductivityArchive::from_zip(&body)?;
    Ok(Json(
        state
            .runtime
            .store
            .productivity_import(ProductivityImportRequest {
                idempotency_key: query.idempotency_key,
                policy: query.policy,
                expected_project_revisions: Default::default(),
                expected_order_revisions: Default::default(),
                archive,
            })
            .await?,
    ))
}

async fn protocol() -> Json<ProductivityProtocol> {
    Json(productivity_protocol())
}
async fn devices(State(state): State<AppState>) -> Answer<Vec<ProductivityDevice>> {
    Ok(Json(state.runtime.store.productivity_devices().await?))
}
async fn register_device(
    State(state): State<AppState>,
    body: Input<RegisterProductivityDevice>,
) -> Answer<ProductivityDevice> {
    Ok(Json(
        state
            .runtime
            .store
            .productivity_register_device(input(body)?)
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyRequest {}
async fn revoke_device(
    State(state): State<AppState>,
    Path(device): Path<String>,
    body: Input<EmptyRequest>,
) -> Answer<ProductivityDevice> {
    input(body)?;
    Ok(Json(
        state
            .runtime
            .store
            .productivity_revoke_device(&device)
            .await?,
    ))
}
async fn offline_queue(
    State(state): State<AppState>,
    Path(device): Path<String>,
) -> Answer<Vec<ProductivityOfflineOperation>> {
    Ok(Json(
        state
            .runtime
            .store
            .productivity_offline_queue(&device)
            .await?,
    ))
}
async fn enqueue_offline(
    State(state): State<AppState>,
    Path(device): Path<String>,
    body: Input<WriteProductivityOfflineOperation>,
) -> Answer<ProductivityOfflineOperation> {
    Ok(Json(
        state
            .runtime
            .store
            .productivity_enqueue_offline(&device, input(body)?)
            .await?,
    ))
}
async fn commit_offline(
    State(state): State<AppState>,
    Path((device, operation)): Path<(String, String)>,
    body: Input<RevisionRequest>,
) -> Answer<ProductivityOfflineOperation> {
    Ok(Json(
        state
            .runtime
            .store
            .productivity_commit_offline(&device, &operation, input(body)?.expected_revision)
            .await?,
    ))
}
async fn discard_offline(
    State(state): State<AppState>,
    Path((device, operation)): Path<(String, String)>,
    body: Input<RevisionRequest>,
) -> Answer<Value> {
    state
        .runtime
        .store
        .productivity_discard_offline(&device, &operation, input(body)?.expected_revision)
        .await?;
    Ok(Json(json!({"discarded":true})))
}
async fn skill_learning(
    State(state): State<AppState>,
    Path((project, skill)): Path<(String, String)>,
) -> Answer<SkillLearningHistory> {
    Ok(Json(
        state
            .runtime
            .store
            .skill_learning_history(&project, &skill)
            .await?,
    ))
}
