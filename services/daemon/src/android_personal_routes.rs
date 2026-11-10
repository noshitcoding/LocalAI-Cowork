//! Existing Android wire operations over identity-owned content and ACL-bound team tasks.
use super::*;
pub(super) fn routes() -> Router<CompatState> {
    Router::new()
        .route("/tasks", get(tasks).post(create_task))
        .route("/tasks/{task}/versions", post(task_version))
        .route("/tasks/{task}/release", post(release_task))
        .route("/tasks/{task}", axum::routing::delete(delete_task))
}
pub(super) fn checked_target(value: &Value) -> Result<()> {
    if value.is_null() {
        return Ok(());
    }
    let object = value.as_object().ok_or_else(invalid)?;
    match value["kind"].as_str() {
        Some("personal_device")
            if object.len() == 2
                && value["device_id"]
                    .as_str()
                    .is_some_and(|s| Uuid::parse_str(s).is_ok()) =>
        {
            Ok(())
        }
        Some("server_linux")
            if object.len() <= 2 && value.get("pool_id").is_none_or(Value::is_null) =>
        {
            Ok(())
        }
        Some("managed_windows_pool")
            if object.len() == 2
                && value["pool_id"]
                    .as_str()
                    .is_some_and(|s| Uuid::parse_str(s).is_ok()) =>
        {
            Ok(())
        }
        _ => Err(invalid()),
    }
}
pub(super) async fn freeze_project_files(
    s: &CompatState,
    a: &Principal,
    v: &mut RunInput,
) -> Result<()> {
    if let Some(input) = v.input.as_object_mut() {
        input.remove("frozen_mcp_bindings");
        input.remove("frozen_private_runner");
        input.remove("frozen_github_binding");
    }
    if v.executor_target["kind"] == "server_linux" {
        if let Some(binding) = s.devices.private_runner_binding() {
            v.input
                .as_object_mut()
                .ok_or_else(invalid)?
                .insert("frozen_private_runner".into(), binding);
        }
        if let Some(id) = v.input.get("github_binding_id").filter(|v| !v.is_null()) {
            let id = Uuid::parse_str(id.as_str().ok_or_else(invalid)?).map_err(|_| invalid())?;
            let frozen = s
                .service
                .store()
                .android_private_github_freeze(a, v.project_id, id)
                .await?;
            v.input
                .as_object_mut()
                .ok_or_else(invalid)?
                .insert("frozen_github_binding".into(), frozen);
        }
        let selected = ids(&v.input, "mcp_metadata_ids")?;
        if !selected.is_empty() {
            let frozen = s
                .service
                .store()
                .android_private_mcp_freeze(a, v.project_id, &selected)
                .await?;
            if let Some(expected) = v.input.get("expected_mcp_revisions") {
                let expected = expected
                    .as_object()
                    .filter(|m| m.len() <= 16)
                    .ok_or_else(invalid)?;
                for (id, revision) in expected {
                    let revision = revision.as_i64().filter(|v| *v > 0).ok_or_else(invalid)?;
                    if !frozen.iter().any(|binding| {
                        binding["entity_id"].as_str() == Some(id)
                            && binding["revision"].as_i64() == Some(revision)
                    }) {
                        return Err(Error::Conflict);
                    }
                }
            }
            v.input
                .as_object_mut()
                .ok_or_else(invalid)?
                .insert("frozen_mcp_bindings".into(), json!(frozen));
        } else if v
            .input
            .get("expected_mcp_revisions")
            .is_some_and(|v| v.as_object().is_none_or(|m| !m.is_empty()))
        {
            return Err(Error::Conflict);
        }
    }
    let files = ids(&v.input, "project_file_ids")?;
    if files.is_empty() {
        if v.input
            .get("project_file_versions")
            .is_some_and(|value| value.as_object().is_none_or(|map| !map.is_empty()))
        {
            return Err(Error::Conflict);
        }
        return Ok(());
    }
    if v.snapshot_id.is_some() {
        return Err(invalid());
    }
    let (snapshot, bindings) = s
        .service
        .store()
        .android_private_files_snapshot_selected(
            a,
            v.project_id,
            &files,
            &v.idempotency_key,
            s.keys.as_ref(),
            s.key_id,
            v.input.get("project_file_versions"),
        )
        .await?;
    v.snapshot_id = Some(snapshot);
    v.input
        .as_object_mut()
        .ok_or_else(invalid)?
        .insert("frozen_project_files".into(), json!(bindings));
    Ok(())
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UpdateProject {
    expected_revision: i64,
    name: String,
    description: String,
    preferred_executor_target: Option<Value>,
    policy: Value,
}
pub(super) async fn update_project(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(p): Path<Uuid>,
    Json(v): Json<UpdateProject>,
) -> Answer {
    if let Some(t) = &v.preferred_executor_target {
        checked_target(t)?;
    }
    if s.service
        .store()
        .android_personal_project(&a, p)
        .await?
        .is_some()
    {
        return Ok(Json(
            s.service
                .store()
                .android_personal_update_project(
                    &a,
                    p,
                    serde_json::to_value(v).map_err(|_| invalid())?,
                    false,
                )
                .await?,
        ));
    }
    if let Some(t) = v.preferred_executor_target {
        target(&t)?;
    }
    let team = s.service.store().android_project_team(&a, p).await?;
    let old = s.service.project(&a, team, p).await?;
    if !v.description.is_empty()
        || !v.policy.is_null()
            && !v.policy.as_object().is_some_and(|o| o.is_empty())
            && v.policy != json!({"adapter":"team_litellm","archived":old.archived})
    {
        return Err(invalid().into());
    }
    let project = s
        .service
        .update_project(
            &a,
            team,
            p,
            UpdateTeamProject {
                name: v.name,
                expected_revision: v.expected_revision,
                archived: old.archived,
            },
        )
        .await?;
    Ok(Json(project_wire(&s, &a, &project).await?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExpectedRevision {
    expected_revision: i64,
}
pub(super) async fn delete_project(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(p): Path<Uuid>,
    Query(q): Query<ExpectedRevision>,
) -> std::result::Result<StatusCode, Failure> {
    if s.service
        .store()
        .android_personal_project(&a, p)
        .await?
        .is_some()
    {
        s.service
            .store()
            .android_personal_update_project(
                &a,
                p,
                json!({"expected_revision":q.expected_revision}),
                true,
            )
            .await?;
    } else {
        let team = s.service.store().android_project_team(&a, p).await?;
        let old = s.service.project(&a, team, p).await?;
        s.service
            .update_project(
                &a,
                team,
                p,
                UpdateTeamProject {
                    name: old.name,
                    expected_revision: q.expected_revision,
                    archived: true,
                },
            )
            .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UpdateThread {
    expected_revision: i64,
    title: String,
}
pub(super) async fn update_thread(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    Json(v): Json<UpdateThread>,
) -> Answer {
    if s.service
        .store()
        .android_personal_thread(&a, id)
        .await?
        .is_some()
    {
        return Ok(Json(
            s.service
                .store()
                .android_personal_update_thread(
                    &a,
                    id,
                    serde_json::to_value(v).map_err(|_| invalid())?,
                    false,
                )
                .await?,
        ));
    }
    let old = s.service.store().android_order(&a, id).await?;
    let o = s
        .service
        .update_order(
            &a,
            old.team_id,
            id,
            UpdateTeamOrder {
                title: v.title,
                prompt: old.prompt,
                expected_revision: v.expected_revision,
                archived: old.archived,
            },
        )
        .await?;
    Ok(Json(thread_wire(&o)))
}
pub(super) async fn delete_thread(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    Query(q): Query<ExpectedRevision>,
) -> std::result::Result<StatusCode, Failure> {
    if s.service
        .store()
        .android_personal_thread(&a, id)
        .await?
        .is_some()
    {
        s.service
            .store()
            .android_personal_update_thread(
                &a,
                id,
                json!({"expected_revision":q.expected_revision}),
                true,
            )
            .await?;
    } else {
        let old = s.service.store().android_order(&a, id).await?;
        s.service
            .update_order(
                &a,
                old.team_id,
                id,
                UpdateTeamOrder {
                    title: old.title,
                    prompt: old.prompt,
                    expected_revision: q.expected_revision,
                    archived: true,
                },
            )
            .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskQuery {
    project_id: Uuid,
}
async fn tasks(
    State(s): State<CompatState>,
    Session(a): Session,
    Query(q): Query<TaskQuery>,
) -> Answer {
    Ok(Json(json!(
        s.service.store().android_tasks(&a, q.project_id).await?
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
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TaskVersion {
    base_revision: i64,
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
    Json(v): Json<CreateTask>,
) -> Answer {
    if let Some(t) = &v.default_target {
        checked_target(t)?;
    }
    checked_task_config(&v.config)?;
    Ok(Json(
        s.service
            .store()
            .android_task_write(
                &a,
                None,
                serde_json::to_value(v).map_err(|_| invalid())?,
                "create",
            )
            .await?,
    ))
}
async fn task_version(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    Json(v): Json<TaskVersion>,
) -> Answer {
    if let Some(t) = &v.default_target {
        checked_target(t)?;
    }
    checked_task_config(&v.config)?;
    Ok(Json(
        s.service
            .store()
            .android_task_write(
                &a,
                Some(id),
                serde_json::to_value(v).map_err(|_| invalid())?,
                "version",
            )
            .await?,
    ))
}
fn checked_task_config(config: &Value) -> Result<()> {
    let runner = config
        .get("runner")
        .or_else(|| config.get("sync_metadata").and_then(|m| m.get("runner")));
    if runner.is_some_and(|v| !matches!(v.as_str(), Some("model" | "crew" | "workflow"))) {
        return Err(invalid());
    }
    Ok(())
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReleaseTask {
    revision: i64,
}
async fn release_task(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    Json(v): Json<ReleaseTask>,
) -> Answer {
    Ok(Json(
        s.service
            .store()
            .android_task_write(
                &a,
                Some(id),
                serde_json::to_value(v).map_err(|_| invalid())?,
                "release",
            )
            .await?,
    ))
}
async fn delete_task(
    State(s): State<CompatState>,
    Session(a): Session,
    Path(id): Path<Uuid>,
    Query(q): Query<ExpectedRevision>,
) -> std::result::Result<StatusCode, Failure> {
    s.service
        .store()
        .android_task_write(
            &a,
            Some(id),
            json!({"expected_revision":q.expected_revision}),
            "delete",
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn freeze_task(s: &CompatState, a: &Principal, v: &mut RunInput) -> Result<()> {
    let Some(reference) = &v.task else {
        return Ok(());
    };
    let task = s
        .service
        .store()
        .android_task_frozen(a, v.project_id, reference)
        .await?;
    checked_task_config(&task["config"])?;
    let instructions = task["instructions"].as_str().ok_or_else(invalid)?;
    let original = prompt(&v.input)?;
    let mut input = v.input.as_object().cloned().unwrap_or_default();
    input.insert("prompt".into(), json!(original));
    input.insert("task_instructions".into(), json!(instructions));
    input.insert("task_config".into(), task["config"].clone());
    let runner = task["config"]
        .get("runner")
        .or_else(|| {
            task["config"]
                .get("sync_metadata")
                .and_then(|m| m.get("runner"))
        })
        .and_then(Value::as_str)
        .unwrap_or("model");
    input.remove("private_workflow");
    input.remove("device_crew");
    input.insert("task_runner".into(), json!(runner));
    if runner != "model" {
        if v.project_privacy != "private_local" {
            return Err(Error::Forbidden);
        }
        match v.executor_target["kind"].as_str() {
            Some("server_linux") => {
                let stamp = s
                    .devices
                    .private_workflow_admission(a, v.project_id, runner)
                    .await?;
                let frozen = s
                    .service
                    .store()
                    .android_workflow_freeze_task(
                        a,
                        v.project_id,
                        &task["config"],
                        v.model_profile_id.ok_or_else(invalid)?,
                        s.keys.as_ref(),
                        &stamp,
                    )
                    .await?
                    .ok_or(Error::Forbidden)?;
                input.insert("private_workflow".into(), frozen);
                if let Some(binding) = s.devices.private_runner_binding() {
                    input.insert("frozen_private_runner".into(), binding);
                }
                let capability = if runner == "crew" {
                    "crew.python"
                } else {
                    "workflow.pure"
                };
                if !v.required_capabilities.iter().any(|c| c == capability) {
                    v.required_capabilities.push(capability.into());
                }
            }
            Some("personal_device") if runner == "crew" => {
                let definition = s
                    .service
                    .store()
                    .android_workflow_device_task(a, v.project_id, &task["config"], s.keys.as_ref())
                    .await?;
                input.insert("crew_definition".into(), definition);
                if !v.required_capabilities.iter().any(|c| c == "crew.python") {
                    v.required_capabilities.push("crew.python".into());
                }
            }
            _ => {
                return Err(Error::Validation(
                    "private_workflow_target_unavailable".into(),
                ));
            }
        }
    }
    v.input = Value::Object(input);
    for cap in task["required_capabilities"]
        .as_array()
        .ok_or_else(invalid)?
    {
        let cap = cap.as_str().ok_or_else(invalid)?.to_owned();
        if !v.required_capabilities.contains(&cap) {
            v.required_capabilities.push(cap);
        }
    }
    Ok(())
}
pub(super) fn metadata_ids(input: &Value, key: &str) -> Result<Vec<Uuid>> {
    let value = input.get(key).or_else(|| {
        input.get("task_config").and_then(|v| {
            v.get(key)
                .or_else(|| v.get("sync_metadata").and_then(|m| m.get(key)))
        })
    });
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let mut result = Vec::new();
    for id in ids(&json!({key:value}), key)? {
        if !result.contains(&id) {
            result.push(id);
        }
    }
    if result.len() > 32 {
        return Err(invalid());
    }
    Ok(result)
}
