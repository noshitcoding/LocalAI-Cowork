//! Exact V1 schedule bodies over actor-private grants, never native/owner bearer.
use super::*;
pub(super) fn routes() -> Router<CompatState> {
    Router::new()
        .route("/schedules", get(list).post(create))
        .route("/thread-schedules", post(create_thread))
        .route("/schedules/preview", get(preview))
        .route("/schedules/tasks", post(create_task))
        .route("/schedules/operations/{key}", get(original))
        .route(
            "/schedules/{schedule}",
            get(read).put(update).delete(remove),
        )
        .route("/schedules/{schedule}/history", get(history))
        .route(
            "/schedules/{schedule}/occurrences/{occurrence}/resolution",
            get(resolution).post(finalize),
        )
}
fn key(headers: &HeaderMap, fallback: Option<&str>) -> Result<String> {
    let mut keys = headers.get_all("idempotency-key").iter();
    let supplied = keys
        .next()
        .map(|v| v.to_str().map(str::to_owned))
        .transpose()
        .map_err(|_| invalid())?;
    if keys.next().is_some()
        || supplied
            .as_deref()
            .zip(fallback)
            .is_some_and(|(a, b)| a != b)
    {
        return Err(invalid());
    }
    let key = supplied
        .or_else(|| fallback.map(str::to_owned))
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    if key.is_empty() || key.len() > 200 || key.chars().any(char::is_control) {
        return Err(invalid());
    }
    Ok(key)
}
#[derive(Deserialize)]
struct Project {
    project_id: Uuid,
}
async fn list(
    State(s): State<CompatState>,
    Session(a): Session,
    Query(v): Query<Project>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .android_schedules(&a, v.project_id)
            .await?
    )))
}
async fn read(State(s): State<CompatState>, Session(a): Session, Path(id): Path<Uuid>) -> Answer {
    Ok(Json(json!(
        s.service.store().android_schedule(&a, id).await?
    )))
}
#[expect(
    clippy::too_many_arguments,
    reason = "Schedule preparation binds the authenticated principal, project/thread/task, target inputs, provider profile and idempotency key to the current device service."
)]
async fn prepare(
    s: &CompatState,
    a: &Principal,
    project: Uuid,
    thread: Option<Uuid>,
    task: Uuid,
    target: &Value,
    input: &Value,
    profile: Option<Uuid>,
    key: &str,
) -> Result<cowork_storage::android_private_schedules::PreparedSchedule> {
    let runner = s.devices.private_runner_binding();
    s.service
        .store()
        .android_schedule_prepare(
            a,
            project,
            thread,
            task,
            target,
            input,
            profile,
            key,
            runner.as_ref(),
            s.devices.private_server_available(),
            s.keys.as_ref(),
            s.key_id,
        )
        .await
}
async fn create(
    State(s): State<CompatState>,
    Session(a): Session,
    headers: HeaderMap,
    Json(v): Json<AndroidCreateSchedule>,
) -> std::result::Result<(StatusCode, Json<Value>), Failure> {
    let key = key(&headers, None)?;
    let request = serde_json::to_value(&v).map_err(|_| invalid())?;
    if let Some(v) = s
        .service
        .store()
        .android_schedule_original(&a, &key, "create", &request)
        .await?
    {
        return Ok((StatusCode::CREATED, Json(v)));
    }
    let prepared = if v.enabled {
        Some(
            prepare(
                &s,
                &a,
                v.project_id,
                Some(v.thread_id),
                v.task_id,
                &v.executor_target,
                &v.input,
                v.model_profile_id,
                &key,
            )
            .await?,
        )
    } else {
        None
    };
    Ok((
        StatusCode::CREATED,
        Json(
            s.service
                .store()
                .android_schedule_write(&a, None, &key, &request, prepared, None)
                .await?,
        ),
    ))
}
async fn create_thread(
    State(s): State<CompatState>,
    Session(a): Session,
    headers: HeaderMap,
    Json(v): Json<AndroidCreateThreadSchedule>,
) -> std::result::Result<(StatusCode, Json<Value>), Failure> {
    let key = key(&headers, Some(&v.idempotency_key))?;
    let request = serde_json::to_value(&v).map_err(|_| invalid())?;
    if let Some(v) = s
        .service
        .store()
        .android_schedule_original(&a, &key, "thread_create", &request)
        .await?
    {
        return Ok((StatusCode::CREATED, Json(v)));
    }
    let prepared = if v.enabled {
        Some(
            prepare(
                &s,
                &a,
                v.project_id,
                None,
                v.task_id,
                &v.executor_target,
                &v.input,
                v.model_profile_id,
                &key,
            )
            .await?,
        )
    } else {
        None
    };
    Ok((
        StatusCode::CREATED,
        Json(
            s.service
                .store()
                .android_schedule_write(&a, None, &key, &request, prepared, Some(&v.title))
                .await?,
        ),
    ))
}
async fn update(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(v): Json<AndroidUpdateSchedule>,
) -> Answer {
    let key = key(&headers, None)?;
    let request = json!({"id":id,"body":v});
    if let Some(v) = s
        .service
        .store()
        .android_schedule_original(&a, &key, "update", &request)
        .await?
    {
        return Ok(Json(v));
    }
    let old = s.service.store().android_schedule(&a, id).await?;
    if old.revision != v.expected_revision {
        return Err(Error::Conflict.into());
    }
    let prepared = if v.enabled {
        Some(
            prepare(
                &s,
                &a,
                old.project_id,
                Some(old.thread_id),
                old.task_id,
                &v.executor_target,
                &v.input,
                v.model_profile_id,
                &key,
            )
            .await?,
        )
    } else {
        None
    };
    Ok(Json(
        s.service
            .store()
            .android_schedule_write(&a, Some(id), &key, &request, prepared, None)
            .await?,
    ))
}
#[derive(Deserialize)]
struct Delete {
    expected_revision: Option<i64>,
}
async fn remove(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Query(v): Query<Delete>,
) -> std::result::Result<StatusCode, Failure> {
    s.service
        .store()
        .android_schedule_delete(&a, id, &key(&headers, None)?, v.expected_revision)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
struct History {
    #[serde(default)]
    after: i64,
    limit: Option<i64>,
}
async fn history(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    Query(v): Query<History>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .android_schedule_history(&a, id, v.after, v.limit.unwrap_or(50))
            .await?
    )))
}
#[derive(Deserialize)]
struct Preview {
    cron: String,
    timezone: String,
}
async fn preview(Session(_): Session, Query(v): Query<Preview>) -> Answer {
    Ok(Json(json!(cowork_storage::android_schedule_cron::preview(
        &v.cron,
        &v.timezone,
        now()
    )?)))
}
#[derive(Deserialize)]
struct Original {
    request_sha256: String,
}
async fn original(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(key): Path<String>,
    Query(v): Query<Original>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_schedule_receipt(&a, &key, &v.request_sha256)
            .await?,
    ))
}
async fn resolution(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((schedule, occurrence)): Path<(Uuid, Uuid)>,
) -> Answer {
    Ok(Json(json!(
        s.service
            .store()
            .android_schedule_resolution(&a, schedule, occurrence, s.keys.as_ref())
            .await?
    )))
}
async fn finalize(
    State(s): State<CompatState>,
    Session(a): Session,
    Path((schedule, occurrence)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(v): Json<AndroidScheduleResolutionRequest>,
) -> Answer {
    // This additive irreversible operation requires an original caller key even for old APKs.
    if !headers.contains_key("idempotency-key") {
        return Err(invalid().into());
    }
    Ok(Json(json!(
        s.service
            .store()
            .android_schedule_finalize(
                &a,
                schedule,
                occurrence,
                &key(&headers, None)?,
                &v,
                s.keys.as_ref()
            )
            .await?
    )))
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateTask {
    project_id: Uuid,
    name: String,
    instructions: String,
    required_capabilities: Vec<String>,
    default_target: Option<Value>,
    config: Value,
    release: bool,
}
async fn create_task(
    State(s): State<CompatState>,
    Session(a): Session,
    headers: HeaderMap,
    Json(v): Json<CreateTask>,
) -> std::result::Result<(StatusCode, Json<Value>), Failure> {
    let request = serde_json::to_value(v).map_err(|_| invalid())?;
    Ok((
        StatusCode::CREATED,
        Json(
            s.service
                .store()
                .android_schedule_create_task(&a, &key(&headers, None)?, &request)
                .await?,
        ),
    ))
}
