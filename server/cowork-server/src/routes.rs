use std::{collections::BTreeMap, convert::Infallible, time::Duration};

use axum::{
    extract::{Extension, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{sse::Event, sse::KeepAlive, IntoResponse, Sse},
    Json,
};
use chrono::Utc;
use cowork_contracts::{
    ensure_compatible, AppendRunEventRequest, Capability, CapabilityDescriptor, CompleteRunRequest,
    CreateExecutorCredentialRequest, CreateRunRequest, CreateThreadMessageRequest,
    ExecutorCredentialSecret, ExecutorHeartbeat, ExecutorKind, ExecutorRegistration,
    FailRunRequest, FrozenReference, LeaseHeartbeat, ListRunsResponse, MessageRecord,
    ProjectPrivacy, ProjectRole, RunRecord, RunSpec, RunState, ThreadMessageRun, API_VERSION,
    MIN_COMPATIBLE_SCHEMA_VERSION, SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    auth::{self, ExecutorPrincipal, Principal},
    db, desktop,
    error::ApiError,
    organization, providers, AppState,
};

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    status: &'static str,
}

#[derive(Debug, Serialize)]
pub struct VersionResponse {
    api_version: &'static str,
    schema_version: u16,
    minimum_compatible_schema_version: u16,
    build_version: &'static str,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    limit: Option<i64>,
}

pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

pub async fn ready(State(state): State<AppState>) -> Result<Json<HealthResponse>, ApiError> {
    sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.pool)
        .await?;
    Ok(Json(HealthResponse { status: "ready" }))
}

pub async fn version() -> Json<VersionResponse> {
    Json(VersionResponse {
        api_version: API_VERSION,
        schema_version: SCHEMA_VERSION,
        minimum_compatible_schema_version: MIN_COMPATIBLE_SCHEMA_VERSION,
        build_version: env!("CARGO_PKG_VERSION"),
    })
}

pub async fn capabilities(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Value>, ApiError> {
    let executors = db::list_executors_for_user(&state.pool, principal.user_id).await?;
    Ok(Json(json!({
        "schema_version": SCHEMA_VERSION,
        "server_linux": server_capability_descriptors(&state.server_capabilities),
        "executors": executors,
    })))
}

pub async fn create_run(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<CreateRunRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let (spec, initial_state) = prepare_run(&state, &principal, request).await?;
    let run = db::create_run(&state.pool, &spec, initial_state).await?;
    Ok((StatusCode::CREATED, Json(run)))
}

pub async fn create_thread_message(
    State(state): State<AppState>,
    Path(thread_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<CreateThreadMessageRequest>,
) -> Result<(StatusCode, Json<ThreadMessageRun>), ApiError> {
    validate_message_content(&request.content)?;
    if request.run.thread_id != thread_id {
        return Err(ApiError::Unprocessable(
            "run.thread_id must match the thread in the request path".to_owned(),
        ));
    }
    let (spec, initial_state) = prepare_run(&state, &principal, request.run).await?;
    let (message, run) =
        db::create_thread_message_run(&state.pool, &spec, initial_state, request.content).await?;
    Ok((
        StatusCode::CREATED,
        Json(ThreadMessageRun {
            schema_version: SCHEMA_VERSION,
            message,
            run,
        }),
    ))
}

pub async fn list_thread_messages(
    State(state): State<AppState>,
    Path(thread_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<MessageRecord>>, ApiError> {
    let project_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT project_id FROM threads WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(thread_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("thread {thread_id} was not found")))?;
    organization::ensure_thread_role(
        &state.pool,
        principal.user_id,
        project_id,
        thread_id,
        ProjectRole::Viewer,
    )
    .await?;
    Ok(Json(
        db::list_thread_messages(&state.pool, thread_id, query.limit.unwrap_or(100)).await?,
    ))
}

async fn prepare_run(
    state: &AppState,
    principal: &Principal,
    mut request: CreateRunRequest,
) -> Result<(RunSpec, RunState), ApiError> {
    validate_create_run(&request)?;
    organization::ensure_run_context(
        &state.pool,
        principal.user_id,
        request.project_id,
        request.thread_id,
        request.project_revision,
        request.project_privacy,
        &request.executor_target,
    )
    .await?;
    if let Some(instructions) =
        ensure_released_task_reference(&state.pool, request.project_id, request.task.as_ref())
            .await?
    {
        request.input = freeze_task_instructions(request.input, instructions);
    }
    providers::ensure_profile_for_target(
        &state.pool,
        principal.user_id,
        request.project_id,
        request.model_profile_id,
        &request.executor_target,
    )
    .await?;
    let now = Utc::now();
    let run_id = Uuid::new_v4();
    let initial_state = initial_run_state(state, &request).await?;
    let spec = RunSpec {
        schema_version: SCHEMA_VERSION,
        id: run_id,
        thread_id: request.thread_id,
        project_id: request.project_id,
        project: FrozenReference {
            id: request.project_id,
            revision: request.project_revision,
        },
        project_privacy: request.project_privacy,
        task: request.task,
        creator_user_id: principal.user_id,
        executor_target: request.executor_target,
        required_capabilities: request.required_capabilities,
        input: request.input,
        model_profile_id: request.model_profile_id,
        snapshot_id: request.snapshot_id,
        idempotency_key: request.idempotency_key,
        created_at: now,
    };
    Ok((spec, initial_state))
}

async fn ensure_released_task_reference(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    task: Option<&FrozenReference>,
) -> Result<Option<String>, ApiError> {
    let Some(task) = task else {
        return Ok(None);
    };
    let instructions = sqlx::query_scalar::<_, String>(
        r#"
        SELECT instructions FROM task_definitions
        WHERE id = $1 AND revision = $2 AND project_id = $3
          AND released AND deleted_at IS NULL
        "#,
    )
    .bind(task.id)
    .bind(task.revision)
    .bind(project_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| {
        ApiError::Unprocessable(format!(
            "task {} revision {} is missing, belongs to another project, or is not released",
            task.id, task.revision
        ))
    })?;
    Ok(Some(instructions))
}

fn freeze_task_instructions(input: Value, instructions: String) -> Value {
    let mut object = match input {
        Value::Object(object) => object,
        Value::Null => serde_json::Map::new(),
        other => serde_json::Map::from_iter([("input".to_owned(), other)]),
    };
    object.insert("task_instructions".to_owned(), Value::String(instructions));
    Value::Object(object)
}

pub async fn list_runs(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<ListQuery>,
) -> Result<Json<ListRunsResponse>, ApiError> {
    let items =
        db::list_runs_for_user(&state.pool, principal.user_id, query.limit.unwrap_or(100)).await?;
    for run in &items {
        organization::ensure_thread_role(
            &state.pool,
            principal.user_id,
            run.spec.project_id,
            run.spec.thread_id,
            ProjectRole::Viewer,
        )
        .await?;
    }
    Ok(Json(ListRunsResponse {
        items,
        next_cursor: None,
    }))
}

pub async fn get_run(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<RunRecord>, ApiError> {
    let run = db::get_run(&state.pool, run_id).await?;
    organization::ensure_thread_role(
        &state.pool,
        principal.user_id,
        run.spec.project_id,
        run.spec.thread_id,
        ProjectRole::Viewer,
    )
    .await?;
    Ok(Json(run))
}

pub async fn cancel_run(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<RunRecord>, ApiError> {
    let run = db::get_run(&state.pool, run_id).await?;
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        run.spec.project_id,
        ProjectRole::Runner,
    )
    .await?;
    desktop::end_sessions(&state.pool, state.runner.as_deref(), run_id)
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(db::cancel_run(&state.pool, run_id).await?))
}

pub async fn run_events(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let run = db::get_run(&state.pool, run_id).await?;
    organization::ensure_thread_role(
        &state.pool,
        principal.user_id,
        run.spec.project_id,
        run.spec.thread_id,
        ProjectRole::Viewer,
    )
    .await?;
    let mut cursor = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    let pool = state.pool.clone();
    let stream = async_stream::stream! {
        loop {
            match db::get_run_events(&pool, run_id, cursor, 250).await {
                Ok(events) => {
                    for run_event in events {
                        cursor = run_event.sequence;
                        let event_name = serde_json::to_value(run_event.kind)
                            .ok()
                            .and_then(|value| value.as_str().map(ToOwned::to_owned))
                            .unwrap_or_else(|| "event".to_owned());
                        match serde_json::to_string(&run_event) {
                            Ok(data) => yield Ok(Event::default()
                                .id(run_event.sequence.to_string())
                                .event(event_name)
                                .data(data)),
                            Err(error) => {
                                tracing::error!(?error, %run_id, "failed to encode SSE event");
                                break;
                            }
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(?error, %run_id, "event stream database read failed");
                }
            }
            match db::get_run(&pool, run_id).await {
                Ok(run) if run.state.is_terminal()
                    && db::get_run_events(&pool, run_id, cursor, 1)
                        .await
                        .map(|events| events.is_empty())
                        .unwrap_or(false) => break,
                Err(_) => break,
                _ => {}
            }
            tokio::time::sleep(Duration::from_millis(750)).await;
        }
    };
    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

pub async fn register_executor(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(mut registration): Json<ExecutorRegistration>,
) -> Result<impl IntoResponse, ApiError> {
    if registration.kind == cowork_contracts::ExecutorKind::PersonalDevice {
        if registration.pool_id.is_some() {
            return Err(ApiError::Unprocessable(
                "personal devices cannot belong to an executor pool".to_owned(),
            ));
        }
        match registration.owner_user_id {
            Some(owner) if owner != principal.user_id => {
                return Err(ApiError::Unauthorized(
                    "a personal device can only be registered for the current user".to_owned(),
                ));
            }
            None => registration.owner_user_id = Some(principal.user_id),
            _ => {}
        }
        registration
            .personal_device_remote_control
            .get_or_insert_default();
    } else if !db::user_is_platform_admin(&state.pool, principal.user_id).await? {
        return Err(ApiError::Unauthorized(
            "only platform administrators can register managed executors".to_owned(),
        ));
    } else {
        if registration.personal_device_remote_control.is_some() {
            return Err(ApiError::Unprocessable(
                "personal_device_remote_control is only valid for personal devices".to_owned(),
            ));
        }
        let pool_id = registration.pool_id.ok_or_else(|| {
            ApiError::Unprocessable("managed executors require pool_id".to_owned())
        })?;
        let expected_kind = match registration.kind {
            ExecutorKind::ManagedWindows => "managed_windows",
            ExecutorKind::ServerLinux => "server_linux",
            ExecutorKind::PersonalDevice => unreachable!(),
        };
        let valid_pool = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM executor_pools WHERE id = $1 AND kind = $2 AND deleted_at IS NULL)",
        )
        .bind(pool_id)
        .bind(expected_kind)
        .fetch_one(&state.pool)
        .await?;
        if !valid_pool {
            return Err(ApiError::Unprocessable(
                "executor pool is missing or has the wrong kind".to_owned(),
            ));
        }
        if registration.kind == ExecutorKind::ManagedWindows
            && registration.max_concurrent_runs != 1
        {
            return Err(ApiError::Unprocessable(
                "managed Windows executors support exactly one interactive run".to_owned(),
            ));
        }
    }
    let record = db::register_executor(&state.pool, &registration).await?;
    Ok((axum::http::StatusCode::CREATED, Json(record)))
}

pub async fn register_executor_agent(
    State(state): State<AppState>,
    Path(executor_id): Path<Uuid>,
    Extension(principal): Extension<ExecutorPrincipal>,
    Json(registration): Json<ExecutorRegistration>,
) -> Result<Json<cowork_contracts::ExecutorRecord>, ApiError> {
    ensure_executor_identity(executor_id, &principal)?;
    Ok(Json(
        db::refresh_executor_registration(&state.pool, executor_id, &registration).await?,
    ))
}

pub async fn create_executor_credential(
    State(state): State<AppState>,
    Path(executor_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<CreateExecutorCredentialRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if !db::user_can_manage_executor(&state.pool, principal.user_id, executor_id).await? {
        return Err(ApiError::Unauthorized(
            "the current user cannot manage this executor".to_owned(),
        ));
    }
    let label = request.label.trim();
    if label.is_empty() || label.len() > 200 {
        return Err(ApiError::Unprocessable(
            "credential label must contain 1 to 200 characters".to_owned(),
        ));
    }
    let now = Utc::now();
    let expires_at = request
        .expires_at
        .unwrap_or_else(|| now + chrono::Duration::days(90));
    if expires_at <= now + chrono::Duration::hours(1)
        || expires_at > now + chrono::Duration::days(365)
    {
        return Err(ApiError::Unprocessable(
            "executor credentials must expire between one hour and 365 days from now".to_owned(),
        ));
    }
    let credential_id = Uuid::new_v4();
    let token = auth::random_token()?;
    let digest = auth::opaque_token_hash(&token);
    sqlx::query(
        r#"
        INSERT INTO executor_credentials (
            id, executor_id, token_hash, label, created_by_user_id, expires_at
        ) VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(credential_id)
    .bind(executor_id)
    .bind(digest.as_slice())
    .bind(label)
    .bind(principal.user_id)
    .bind(expires_at)
    .execute(&state.pool)
    .await?;
    sqlx::query(
        "INSERT INTO audit_events (id, actor_user_id, action, target_type, target_id, metadata) VALUES ($1, $2, 'executor.credential.create', 'executor', $3, $4)",
    )
    .bind(Uuid::new_v4())
    .bind(principal.user_id)
    .bind(executor_id)
    .bind(json!({"credential_id": credential_id, "label": label, "expires_at": expires_at}))
    .execute(&state.pool)
    .await?;
    Ok((
        axum::http::StatusCode::CREATED,
        Json(ExecutorCredentialSecret {
            schema_version: SCHEMA_VERSION,
            credential_id,
            executor_id,
            token,
            expires_at: Some(expires_at),
        }),
    ))
}

pub async fn revoke_executor_credential(
    State(state): State<AppState>,
    Path((executor_id, credential_id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<Principal>,
) -> Result<axum::http::StatusCode, ApiError> {
    if !db::user_can_manage_executor(&state.pool, principal.user_id, executor_id).await? {
        return Err(ApiError::Unauthorized(
            "the current user cannot manage this executor".to_owned(),
        ));
    }
    let affected = sqlx::query(
        "UPDATE executor_credentials SET revoked_at = now(), revoke_reason = 'administrator_revoked' WHERE id = $1 AND executor_id = $2 AND revoked_at IS NULL",
    )
    .bind(credential_id)
    .bind(executor_id)
    .execute(&state.pool)
    .await?
    .rows_affected();
    if affected == 0 {
        return Err(ApiError::NotFound(format!(
            "executor credential {credential_id} was not found"
        )));
    }
    Ok(axum::http::StatusCode::NO_CONTENT)
}

pub async fn heartbeat_executor(
    State(state): State<AppState>,
    Path(executor_id): Path<Uuid>,
    Extension(principal): Extension<ExecutorPrincipal>,
    Json(heartbeat): Json<ExecutorHeartbeat>,
) -> Result<Json<cowork_contracts::ExecutorRecord>, ApiError> {
    ensure_executor_identity(executor_id, &principal)?;
    ensure_compatible(heartbeat.protocol_version)
        .map_err(|error| ApiError::Unprocessable(error.to_string()))?;
    Ok(Json(
        db::heartbeat_executor(&state.pool, executor_id, heartbeat.active_run_ids.len()).await?,
    ))
}

pub async fn list_executors(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<cowork_contracts::ExecutorRecord>>, ApiError> {
    Ok(Json(
        db::list_executors_for_user(&state.pool, principal.user_id).await?,
    ))
}

pub async fn claim_executor_run(
    State(state): State<AppState>,
    Path(executor_id): Path<Uuid>,
    Extension(principal): Extension<ExecutorPrincipal>,
) -> Result<Json<Option<cowork_contracts::RunLease>>, ApiError> {
    ensure_executor_identity(executor_id, &principal)?;
    Ok(Json(
        db::claim_external_run(&state.pool, executor_id, state.lease_seconds).await?,
    ))
}

pub async fn renew_executor_lease(
    State(state): State<AppState>,
    Path((executor_id, run_id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<ExecutorPrincipal>,
    Json(heartbeat): Json<LeaseHeartbeat>,
) -> Result<Json<RunRecord>, ApiError> {
    ensure_executor_identity(executor_id, &principal)?;
    Ok(Json(
        db::renew_lease(
            &state.pool,
            run_id,
            executor_id,
            heartbeat.lease_token,
            state.lease_seconds,
        )
        .await?,
    ))
}

pub async fn append_executor_event(
    State(state): State<AppState>,
    Path((executor_id, run_id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<ExecutorPrincipal>,
    Json(request): Json<AppendRunEventRequest>,
) -> Result<Json<cowork_contracts::RunEvent>, ApiError> {
    ensure_executor_identity(executor_id, &principal)?;
    Ok(Json(
        db::append_leased_event(
            &state.pool,
            run_id,
            executor_id,
            request.lease_token,
            request.source_event_id,
            request.kind,
            request.payload,
        )
        .await?,
    ))
}

pub async fn complete_executor_run(
    State(state): State<AppState>,
    Path((executor_id, run_id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<ExecutorPrincipal>,
    Json(request): Json<CompleteRunRequest>,
) -> Result<Json<RunRecord>, ApiError> {
    ensure_executor_identity(executor_id, &principal)?;
    let record = db::complete_leased_run(
        &state.pool,
        run_id,
        executor_id,
        request.lease_token,
        request.result,
        request.result_snapshot_manifest_id,
        request.result_diff_summary,
    )
    .await?;
    if let Err(error) = desktop::end_external_sessions(&state.pool, run_id).await {
        tracing::warn!(?error, %run_id, "failed to close completed executor desktop sessions");
    }
    Ok(Json(record))
}

pub async fn fail_executor_run(
    State(state): State<AppState>,
    Path((executor_id, run_id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<ExecutorPrincipal>,
    Json(request): Json<FailRunRequest>,
) -> Result<Json<RunRecord>, ApiError> {
    ensure_executor_identity(executor_id, &principal)?;
    let record = db::fail_leased_run(
        &state.pool,
        run_id,
        executor_id,
        request.lease_token,
        request.error,
    )
    .await?;
    if let Err(error) = desktop::end_external_sessions(&state.pool, run_id).await {
        tracing::warn!(?error, %run_id, "failed to close failed executor desktop sessions");
    }
    Ok(Json(record))
}

fn ensure_executor_identity(
    executor_id: Uuid,
    principal: &ExecutorPrincipal,
) -> Result<(), ApiError> {
    if principal.executor_id == executor_id {
        Ok(())
    } else {
        Err(ApiError::Unauthorized(
            "executor credential does not match the requested executor".to_owned(),
        ))
    }
}

async fn initial_run_state(
    state: &AppState,
    request: &CreateRunRequest,
) -> Result<RunState, ApiError> {
    if let Some(snapshot_id) = request.snapshot_id {
        let snapshot_status = sqlx::query_scalar::<_, String>(
            "SELECT status FROM snapshot_manifests WHERE id = $1 AND project_id = $2",
        )
        .bind(snapshot_id)
        .bind(request.project_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| {
            ApiError::Unprocessable(
                "snapshot_id must identify a snapshot from the selected project".to_owned(),
            )
        })?;
        if snapshot_status != "ready" {
            return Ok(RunState::WaitingForSnapshot);
        }
    }
    let remote_target = !matches!(
        request.executor_target,
        cowork_contracts::ExecutorTarget::PersonalDevice { .. }
    );
    if request.project_privacy == ProjectPrivacy::PrivateLocal
        && remote_target
        && request.snapshot_id.is_none()
    {
        return Ok(RunState::WaitingForSnapshot);
    }

    if let cowork_contracts::ExecutorTarget::ServerLinux { .. } = request.executor_target {
        let available: std::collections::HashSet<&str> = state
            .server_capabilities
            .iter()
            .map(|capability| capability.0.as_str())
            .collect();
        return Ok(
            if request
                .required_capabilities
                .iter()
                .all(|capability| available.contains(capability.0.as_str()))
            {
                RunState::Queued
            } else {
                RunState::WaitingForExecutor
            },
        );
    }

    if db::target_has_executor(
        &state.pool,
        &request.executor_target,
        &request.required_capabilities,
    )
    .await?
    {
        Ok(RunState::Queued)
    } else {
        Ok(RunState::WaitingForExecutor)
    }
}

fn validate_create_run(request: &CreateRunRequest) -> Result<(), ApiError> {
    if request.project_revision < 1 {
        return Err(ApiError::Unprocessable(
            "project_revision must be at least one".to_owned(),
        ));
    }
    if request.idempotency_key.trim().is_empty() || request.idempotency_key.len() > 200 {
        return Err(ApiError::Unprocessable(
            "idempotency_key must contain 1 to 200 characters".to_owned(),
        ));
    }
    if request
        .required_capabilities
        .iter()
        .any(|capability| capability.0.trim().is_empty() || capability.0.len() > 100)
    {
        return Err(ApiError::Unprocessable(
            "capability names must contain 1 to 100 characters".to_owned(),
        ));
    }
    Ok(())
}

fn validate_message_content(content: &Value) -> Result<(), ApiError> {
    const MAX_MESSAGE_BYTES: usize = 256 * 1024;
    if content.is_null() {
        return Err(ApiError::Unprocessable(
            "message content must not be null".to_owned(),
        ));
    }
    if serde_json::to_vec(content)?.len() > MAX_MESSAGE_BYTES {
        return Err(ApiError::Unprocessable(format!(
            "message content must not exceed {MAX_MESSAGE_BYTES} bytes"
        )));
    }
    Ok(())
}

pub fn server_capability_descriptors(capabilities: &[Capability]) -> Vec<CapabilityDescriptor> {
    capabilities
        .iter()
        .cloned()
        .map(|name| CapabilityDescriptor {
            schema_version: SCHEMA_VERSION,
            name,
            version: env!("CARGO_PKG_VERSION").to_owned(),
            attributes: BTreeMap::new(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cowork_contracts::ExecutorTarget;

    #[test]
    fn rejects_empty_idempotency_keys() {
        let request = CreateRunRequest {
            thread_id: Uuid::nil(),
            project_id: Uuid::nil(),
            project_revision: 1,
            project_privacy: ProjectPrivacy::TeamManaged,
            task: None,
            executor_target: ExecutorTarget::ServerLinux { pool_id: None },
            required_capabilities: vec![],
            input: Value::Null,
            model_profile_id: None,
            snapshot_id: None,
            idempotency_key: "".to_owned(),
        };
        assert!(validate_create_run(&request).is_err());
    }

    #[test]
    fn rejects_null_and_oversized_message_content() {
        assert!(validate_message_content(&Value::Null).is_err());
        assert!(validate_message_content(&json!({"text": "a".repeat(256 * 1024)})).is_err());
        assert!(validate_message_content(&json!({"text": "hello"})).is_ok());
    }

    #[test]
    fn freezes_authoritative_task_instructions_without_losing_run_input() {
        assert_eq!(
            freeze_task_instructions(
                json!({"prompt": "customer-specific input"}),
                "released task instructions".to_owned(),
            ),
            json!({
                "prompt": "customer-specific input",
                "task_instructions": "released task instructions"
            })
        );
        assert_eq!(
            freeze_task_instructions(json!("legacy input"), "instructions".to_owned()),
            json!({"input": "legacy input", "task_instructions": "instructions"})
        );
    }
}
