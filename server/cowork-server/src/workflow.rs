use std::{str::FromStr, time::Duration as StdDuration};

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use cowork_contracts::{
    ApprovalDecision, ApprovalRequest, ApprovalState, Capability, CreateApprovalRequest,
    CreateCheckpointRequest, CreateInputRequest, CreateScheduleRequest,
    CreateTaskDefinitionRequest, CreateTaskVersionRequest, ExecutorKind, ExecutorTarget,
    FrozenReference, InputRequestState, ProjectPrivacy, ProjectRole, ReleaseTaskVersionRequest,
    ResolveApprovalRequest, RunCheckpoint, RunEventKind, RunInputRequest, RunSpec, RunState,
    ScheduleRecord, SubmitInputResponseRequest, TaskDefinition, UpdateScheduleRequest,
    SCHEMA_VERSION,
};
use cron::Schedule;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::{postgres::PgRow, PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{
    auth::{ExecutorPrincipal, Principal},
    db,
    error::ApiError,
    organization, providers, sync, AppState,
};

const DEFAULT_WAIT_DAYS: i64 = 7;
const MAX_SCHEDULE_CATCH_UPS: usize = 10_000;

#[derive(Debug, Deserialize)]
pub struct ProjectQuery {
    project_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct TaskVersionQuery {
    revision: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteTaskQuery {
    expected_revision: i64,
}

pub async fn create_task(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<CreateTaskDefinitionRequest>,
) -> Result<(StatusCode, Json<TaskDefinition>), ApiError> {
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        request.project_id,
        ProjectRole::Editor,
    )
    .await?;
    validate_task_fields(&request.name, &request.instructions)?;
    if let Some(target) = &request.default_target {
        validate_target_for_project(&state.pool, principal.user_id, request.project_id, target)
            .await?;
    }
    let id = Uuid::new_v4();
    let revision = 1_i64;
    let etag = version_etag(id, revision);
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        r#"
        INSERT INTO task_definitions (
            id, revision, etag, project_id, name, instructions,
            required_capabilities, default_executor_target, config,
            released, created_by
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(revision)
    .bind(&etag)
    .bind(request.project_id)
    .bind(request.name.trim())
    .bind(request.instructions)
    .bind(serde_json::to_value(request.required_capabilities)?)
    .bind(
        request
            .default_target
            .map(serde_json::to_value)
            .transpose()?,
    )
    .bind(request.config)
    .bind(request.release)
    .bind(principal.user_id)
    .fetch_one(&mut *tx)
    .await?;
    let task = row_to_task(&row)?;
    sync::publish_canonical_task_tx(&mut tx, id).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(task)))
}

pub async fn list_tasks(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<Vec<TaskDefinition>>, ApiError> {
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        query.project_id,
        ProjectRole::Viewer,
    )
    .await?;
    let rows = sqlx::query(
        r#"
        SELECT DISTINCT ON (id) * FROM task_definitions
        WHERE project_id = $1 AND deleted_at IS NULL
        ORDER BY id, revision DESC
        "#,
    )
    .bind(query.project_id)
    .fetch_all(&state.pool)
    .await?;
    rows.iter()
        .map(row_to_task)
        .collect::<Result<_, _>>()
        .map(Json)
}

pub async fn get_task(
    State(state): State<AppState>,
    Path(task_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<TaskVersionQuery>,
) -> Result<Json<TaskDefinition>, ApiError> {
    let row = if let Some(revision) = query.revision {
        sqlx::query("SELECT * FROM task_definitions WHERE id = $1 AND revision = $2 AND deleted_at IS NULL")
            .bind(task_id)
            .bind(revision)
            .fetch_optional(&state.pool)
            .await?
    } else {
        sqlx::query("SELECT * FROM task_definitions WHERE id = $1 AND deleted_at IS NULL ORDER BY revision DESC LIMIT 1")
            .bind(task_id)
            .fetch_optional(&state.pool)
            .await?
    }
    .ok_or_else(|| ApiError::NotFound(format!("task {task_id} was not found")))?;
    let task = row_to_task(&row)?;
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        task.project_id,
        ProjectRole::Viewer,
    )
    .await?;
    Ok(Json(task))
}

pub async fn create_task_version(
    State(state): State<AppState>,
    Path(task_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<CreateTaskVersionRequest>,
) -> Result<(StatusCode, Json<TaskDefinition>), ApiError> {
    validate_task_fields(&request.name, &request.instructions)?;
    let mut tx = state.pool.begin().await?;
    let current = sqlx::query(
        "SELECT * FROM task_definitions WHERE id = $1 AND deleted_at IS NULL ORDER BY revision DESC LIMIT 1 FOR UPDATE",
    )
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("task {task_id} was not found")))?;
    let project_id: Uuid = current.try_get("project_id")?;
    let current_revision: i64 = current.try_get("revision")?;
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Editor,
    )
    .await?;
    if current_revision != request.base_revision {
        return Err(ApiError::Conflict(format!(
            "task revision changed from {} to {current_revision}",
            request.base_revision
        )));
    }
    if let Some(target) = &request.default_target {
        validate_target_for_project(&state.pool, principal.user_id, project_id, target).await?;
    }
    if request.release {
        sqlx::query("UPDATE task_definitions SET released = FALSE WHERE id = $1 AND released")
            .bind(task_id)
            .execute(&mut *tx)
            .await?;
    }
    let revision = current_revision + 1;
    let row = sqlx::query(
        r#"
        INSERT INTO task_definitions (
            id, revision, etag, project_id, name, instructions,
            required_capabilities, default_executor_target, config,
            released, created_by
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        RETURNING *
        "#,
    )
    .bind(task_id)
    .bind(revision)
    .bind(version_etag(task_id, revision))
    .bind(project_id)
    .bind(request.name.trim())
    .bind(request.instructions)
    .bind(serde_json::to_value(request.required_capabilities)?)
    .bind(
        request
            .default_target
            .map(serde_json::to_value)
            .transpose()?,
    )
    .bind(request.config)
    .bind(request.release)
    .bind(principal.user_id)
    .fetch_one(&mut *tx)
    .await?;
    let task = row_to_task(&row)?;
    sync::publish_canonical_task_tx(&mut tx, task_id).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(task)))
}

pub async fn release_task_version(
    State(state): State<AppState>,
    Path(task_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<ReleaseTaskVersionRequest>,
) -> Result<Json<TaskDefinition>, ApiError> {
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        "SELECT * FROM task_definitions WHERE id = $1 AND revision = $2 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(task_id)
    .bind(request.revision)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("task {task_id} revision {} was not found", request.revision)))?;
    let project_id: Uuid = row.try_get("project_id")?;
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Editor,
    )
    .await?;
    sqlx::query("UPDATE task_definitions SET released = FALSE WHERE id = $1 AND released")
        .bind(task_id)
        .execute(&mut *tx)
        .await?;
    let row = sqlx::query(
        "UPDATE task_definitions SET released = TRUE WHERE id = $1 AND revision = $2 RETURNING *",
    )
    .bind(task_id)
    .bind(request.revision)
    .fetch_one(&mut *tx)
    .await?;
    let task = row_to_task(&row)?;
    sync::publish_canonical_task_tx(&mut tx, task_id).await?;
    tx.commit().await?;
    Ok(Json(task))
}

pub async fn delete_task(
    State(state): State<AppState>,
    Path(task_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<DeleteTaskQuery>,
) -> Result<StatusCode, ApiError> {
    if query.expected_revision < 1 {
        return Err(ApiError::Unprocessable(
            "expected_revision must be positive".to_owned(),
        ));
    }
    let current = sqlx::query(
        r#"
        SELECT task.project_id, task.revision, project.owner_user_id, project.privacy
        FROM task_definitions task
        JOIN projects project ON project.id = task.project_id
        WHERE task.id = $1 AND task.deleted_at IS NULL
          AND project.deleted_at IS NULL
        ORDER BY task.revision DESC
        LIMIT 1
        "#,
    )
    .bind(task_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("task {task_id} was not found")))?;
    let project_id: Uuid = current.try_get("project_id")?;
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Editor,
    )
    .await?;
    if current.try_get::<i64, _>("revision")? != query.expected_revision {
        return Err(ApiError::Conflict(
            "task revision changed; reload before deleting".to_owned(),
        ));
    }
    let private = current.try_get::<&str, _>("privacy")? == "private_local";
    let owner_user_id: Uuid = current.try_get("owner_user_id")?;
    let mut tx = state.pool.begin().await?;
    let locked_revision = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT revision FROM task_definitions
        WHERE id = $1 AND deleted_at IS NULL
        ORDER BY revision DESC
        LIMIT 1
        FOR UPDATE
        "#,
    )
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("task {task_id} was not found")))?;
    if locked_revision != query.expected_revision {
        return Err(ApiError::Conflict(
            "task revision changed; reload before deleting".to_owned(),
        ));
    }
    let schedule_ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM schedules WHERE task_id = $1 AND deleted_at IS NULL ORDER BY id FOR UPDATE",
    )
    .bind(task_id)
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE task_definitions SET released = FALSE, deleted_at = now() WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(task_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE schedules SET enabled = FALSE, next_run_at = NULL, blocked_reason = 'task deleted', updated_at = now() WHERE task_id = $1 AND deleted_at IS NULL",
    )
    .bind(task_id)
    .execute(&mut *tx)
    .await?;
    if private {
        sync::publish_server_tombstone_tx(&mut tx, owner_user_id, "task", task_id).await?;
        for schedule_id in schedule_ids {
            sync::publish_canonical_schedule_tx(&mut tx, schedule_id).await?;
        }
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn create_schedule(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<CreateScheduleRequest>,
) -> Result<(StatusCode, Json<ScheduleRecord>), ApiError> {
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        request.project_id,
        ProjectRole::Runner,
    )
    .await?;
    organization::ensure_thread_project(&state.pool, request.thread_id, request.project_id).await?;
    ensure_task_project(&state.pool, request.task_id, request.project_id).await?;
    validate_target_for_project(
        &state.pool,
        principal.user_id,
        request.project_id,
        &request.executor_target,
    )
    .await?;
    providers::ensure_profile_for_target(
        &state.pool,
        principal.user_id,
        request.project_id,
        request.model_profile_id,
        &request.executor_target,
    )
    .await?;
    let next_run_at = if request.enabled {
        Some(next_occurrence(
            &request.cron,
            &request.timezone,
            Utc::now(),
        )?)
    } else {
        None
    };
    let id = Uuid::new_v4();
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        r#"
        INSERT INTO schedules (
            id, etag, task_id, project_id, thread_id, cron_expression,
            timezone, executor_target, input, model_profile_id, enabled,
            next_run_at, created_by
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(version_etag(id, 1))
    .bind(request.task_id)
    .bind(request.project_id)
    .bind(request.thread_id)
    .bind(normalized_cron(&request.cron)?)
    .bind(validated_timezone(&request.timezone)?.name())
    .bind(serde_json::to_value(request.executor_target)?)
    .bind(request.input)
    .bind(request.model_profile_id)
    .bind(request.enabled)
    .bind(next_run_at)
    .bind(principal.user_id)
    .fetch_one(&mut *tx)
    .await?;
    let schedule = row_to_schedule(&row)?;
    sync::publish_canonical_schedule_tx(&mut tx, id).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(schedule)))
}

pub async fn list_schedules(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<Vec<ScheduleRecord>>, ApiError> {
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        query.project_id,
        ProjectRole::Viewer,
    )
    .await?;
    let rows = sqlx::query(
        "SELECT * FROM schedules WHERE project_id = $1 AND deleted_at IS NULL ORDER BY created_at",
    )
    .bind(query.project_id)
    .fetch_all(&state.pool)
    .await?;
    rows.iter()
        .map(row_to_schedule)
        .collect::<Result<_, _>>()
        .map(Json)
}

pub async fn update_schedule(
    State(state): State<AppState>,
    Path(schedule_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<UpdateScheduleRequest>,
) -> Result<Json<ScheduleRecord>, ApiError> {
    let existing =
        sqlx::query("SELECT project_id FROM schedules WHERE id = $1 AND deleted_at IS NULL")
            .bind(schedule_id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("schedule {schedule_id} was not found")))?;
    let project_id: Uuid = existing.try_get("project_id")?;
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Runner,
    )
    .await?;
    validate_target_for_project(
        &state.pool,
        principal.user_id,
        project_id,
        &request.executor_target,
    )
    .await?;
    providers::ensure_profile_for_target(
        &state.pool,
        principal.user_id,
        project_id,
        request.model_profile_id,
        &request.executor_target,
    )
    .await?;
    let cron = normalized_cron(&request.cron)?;
    let timezone = validated_timezone(&request.timezone)?;
    let next_run_at = if request.enabled {
        Some(next_occurrence_normalized(&cron, timezone, Utc::now())?)
    } else {
        None
    };
    let next_revision = request.expected_revision + 1;
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        r#"
        UPDATE schedules SET revision = revision + 1, etag = $3,
            cron_expression = $4, timezone = $5, executor_target = $6,
            input = $7, model_profile_id = $8, enabled = $9,
            next_run_at = $10, blocked_reason = NULL, updated_at = now()
        WHERE id = $1 AND revision = $2 AND deleted_at IS NULL
        RETURNING *
        "#,
    )
    .bind(schedule_id)
    .bind(request.expected_revision)
    .bind(version_etag(schedule_id, next_revision))
    .bind(cron)
    .bind(timezone.name())
    .bind(serde_json::to_value(request.executor_target)?)
    .bind(request.input)
    .bind(request.model_profile_id)
    .bind(request.enabled)
    .bind(next_run_at)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| {
        ApiError::Conflict("schedule revision changed; reload before updating".to_owned())
    })?;
    let schedule = row_to_schedule(&row)?;
    sync::publish_canonical_schedule_tx(&mut tx, schedule_id).await?;
    tx.commit().await?;
    Ok(Json(schedule))
}

pub async fn delete_schedule(
    State(state): State<AppState>,
    Path(schedule_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
) -> Result<StatusCode, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT schedule.project_id, project.owner_user_id, project.privacy
        FROM schedules schedule
        JOIN projects project ON project.id = schedule.project_id
        WHERE schedule.id = $1 AND schedule.deleted_at IS NULL
          AND project.deleted_at IS NULL
        "#,
    )
    .bind(schedule_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("schedule {schedule_id} was not found")))?;
    let project_id: Uuid = row.try_get("project_id")?;
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Runner,
    )
    .await?;
    let owner_user_id: Uuid = row.try_get("owner_user_id")?;
    let private = row.try_get::<&str, _>("privacy")? == "private_local";
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "UPDATE schedules SET revision = revision + 1, enabled = FALSE, next_run_at = NULL, deleted_at = now(), updated_at = now() WHERE id = $1",
    )
    .bind(schedule_id)
    .execute(&mut *tx)
    .await?;
    if private {
        sync::publish_server_tombstone_tx(&mut tx, owner_user_id, "schedule", schedule_id).await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn create_approval(
    State(state): State<AppState>,
    Path((executor_id, run_id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<ExecutorPrincipal>,
    Json(request): Json<CreateApprovalRequest>,
) -> Result<(StatusCode, Json<ApprovalRequest>), ApiError> {
    ensure_executor_identity(executor_id, &principal)?;
    let expires_at = validated_expiry(request.expires_at)?;
    let mut tx = state.pool.begin().await?;
    let run = db::verify_lease(&mut tx, run_id, executor_id, request.lease_token).await?;
    if let Some(source_request_id) = request.source_request_id {
        if let Some(row) = sqlx::query(
            "SELECT * FROM approval_requests WHERE run_id = $1 AND source_request_id = $2",
        )
        .bind(run_id)
        .bind(source_request_id)
        .fetch_optional(&mut *tx)
        .await?
        {
            let approval = row_to_approval(&row)?;
            if approval.requested_action != request.requested_action
                || (approval.expires_at - expires_at)
                    .num_microseconds()
                    .is_none_or(|difference| difference.abs() > 1)
            {
                return Err(ApiError::Conflict(
                    "source_request_id was already used for a different approval".to_owned(),
                ));
            }
            tx.commit().await?;
            return Ok((StatusCode::OK, Json(approval)));
        }
    }
    if run.state != RunState::Running {
        return Err(ApiError::Conflict(
            "a run can request approval only while running".to_owned(),
        ));
    }
    let id = Uuid::new_v4();
    let row = sqlx::query(
        r#"
        INSERT INTO approval_requests (
            id, run_id, etag, requested_action, state, expires_at,
            requested_by_executor_id, source_request_id
        ) VALUES ($1, $2, $3, $4, 'pending', $5, $6, $7)
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(run_id)
    .bind(version_etag(id, 1))
    .bind(request.requested_action.clone())
    .bind(expires_at)
    .bind(executor_id)
    .bind(request.source_request_id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE runs SET state = 'waiting_approval', revision = revision + 1, updated_at = now() WHERE id = $1",
    )
    .bind(run_id)
    .execute(&mut *tx)
    .await?;
    db::append_event_tx(
        &mut tx,
        run_id,
        RunEventKind::ApprovalRequested,
        json!({"approval_id": id, "requested_action": request.requested_action, "expires_at": expires_at}),
    )
    .await?;
    db::append_event_tx(
        &mut tx,
        run_id,
        RunEventKind::StateChanged,
        json!({"from": "running", "to": "waiting_approval"}),
    )
    .await?;
    let record = row_to_approval(&row)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(record)))
}

pub async fn list_approvals(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<ApprovalRequest>>, ApiError> {
    ensure_run_role(&state.pool, principal.user_id, run_id, ProjectRole::Viewer).await?;
    let rows =
        sqlx::query("SELECT * FROM approval_requests WHERE run_id = $1 ORDER BY requested_at")
            .bind(run_id)
            .fetch_all(&state.pool)
            .await?;
    rows.iter()
        .map(row_to_approval)
        .collect::<Result<_, _>>()
        .map(Json)
}

pub async fn get_executor_approval(
    State(state): State<AppState>,
    Path((executor_id, run_id, approval_id)): Path<(Uuid, Uuid, Uuid)>,
    Extension(principal): Extension<ExecutorPrincipal>,
) -> Result<Json<ApprovalRequest>, ApiError> {
    ensure_executor_identity(executor_id, &principal)?;
    ensure_run_assigned_executor(&state.pool, run_id, executor_id).await?;
    let row = sqlx::query("SELECT * FROM approval_requests WHERE id = $1 AND run_id = $2")
        .bind(approval_id)
        .bind(run_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("approval {approval_id} was not found")))?;
    Ok(Json(row_to_approval(&row)?))
}

pub async fn resolve_approval(
    State(state): State<AppState>,
    Path((run_id, approval_id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<ResolveApprovalRequest>,
) -> Result<Json<ApprovalRequest>, ApiError> {
    // Deliberately Viewer: users who can observe a run may answer its explicit
    // approval request, while starting/canceling runs still requires Runner.
    ensure_run_role(&state.pool, principal.user_id, run_id, ProjectRole::Viewer).await?;
    let mut tx = state.pool.begin().await?;
    let existing =
        sqlx::query("SELECT * FROM approval_requests WHERE id = $1 AND run_id = $2 FOR UPDATE")
            .bind(approval_id)
            .bind(run_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("approval {approval_id} was not found")))?;
    let state_name: String = existing.try_get("state")?;
    let revision: i64 = existing.try_get("revision")?;
    if state_name != "pending" {
        return Err(ApiError::Conflict(
            "approval is no longer pending".to_owned(),
        ));
    }
    if revision != request.expected_revision {
        return Err(ApiError::Conflict(
            "approval revision changed; reload before resolving".to_owned(),
        ));
    }
    let decision = match request.decision {
        ApprovalDecision::Approved => "approved",
        ApprovalDecision::Rejected => "rejected",
    };
    let row = sqlx::query(
        r#"
        UPDATE approval_requests SET revision = revision + 1, etag = $3,
            state = $4, resolved_by = $5, resolved_at = now()
        WHERE id = $1 AND run_id = $2 RETURNING *
        "#,
    )
    .bind(approval_id)
    .bind(run_id)
    .bind(version_etag(approval_id, revision + 1))
    .bind(decision)
    .bind(principal.user_id)
    .fetch_one(&mut *tx)
    .await?;
    resume_waiting_run(&mut tx, run_id, "waiting_approval").await?;
    db::append_event_tx(
        &mut tx,
        run_id,
        RunEventKind::ApprovalResolved,
        json!({"approval_id": approval_id, "decision": decision, "resolved_by_user_id": principal.user_id}),
    )
    .await?;
    let record = row_to_approval(&row)?;
    tx.commit().await?;
    Ok(Json(record))
}

pub async fn create_input_request(
    State(state): State<AppState>,
    Path((executor_id, run_id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<ExecutorPrincipal>,
    Json(request): Json<CreateInputRequest>,
) -> Result<(StatusCode, Json<RunInputRequest>), ApiError> {
    ensure_executor_identity(executor_id, &principal)?;
    let expires_at = validated_expiry(request.expires_at)?;
    let mut tx = state.pool.begin().await?;
    let run = db::verify_lease(&mut tx, run_id, executor_id, request.lease_token).await?;
    if let Some(source_request_id) = request.source_request_id {
        if let Some(row) = sqlx::query(
            "SELECT * FROM run_input_requests WHERE run_id = $1 AND source_request_id = $2",
        )
        .bind(run_id)
        .bind(source_request_id)
        .fetch_optional(&mut *tx)
        .await?
        {
            let input = row_to_input_request(&row)?;
            if input.prompt != request.prompt
                || (input.expires_at - expires_at)
                    .num_microseconds()
                    .is_none_or(|difference| difference.abs() > 1)
            {
                return Err(ApiError::Conflict(
                    "source_request_id was already used for a different input request".to_owned(),
                ));
            }
            tx.commit().await?;
            return Ok((StatusCode::OK, Json(input)));
        }
    }
    if run.state != RunState::Running {
        return Err(ApiError::Conflict(
            "a run can request input only while running".to_owned(),
        ));
    }
    let id = Uuid::new_v4();
    let row = sqlx::query(
        r#"
        INSERT INTO run_input_requests (
            id, run_id, etag, prompt, state, expires_at, source_request_id
        ) VALUES ($1, $2, $3, $4, 'pending', $5, $6)
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(run_id)
    .bind(version_etag(id, 1))
    .bind(request.prompt.clone())
    .bind(expires_at)
    .bind(request.source_request_id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE runs SET state = 'waiting_input', revision = revision + 1, updated_at = now() WHERE id = $1",
    )
    .bind(run_id)
    .execute(&mut *tx)
    .await?;
    db::append_event_tx(
        &mut tx,
        run_id,
        RunEventKind::InputRequested,
        json!({"input_request_id": id, "prompt": request.prompt, "expires_at": expires_at}),
    )
    .await?;
    db::append_event_tx(
        &mut tx,
        run_id,
        RunEventKind::StateChanged,
        json!({"from": "running", "to": "waiting_input"}),
    )
    .await?;
    let record = row_to_input_request(&row)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(record)))
}

pub async fn list_input_requests(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<RunInputRequest>>, ApiError> {
    ensure_run_role(&state.pool, principal.user_id, run_id, ProjectRole::Viewer).await?;
    let rows =
        sqlx::query("SELECT * FROM run_input_requests WHERE run_id = $1 ORDER BY requested_at")
            .bind(run_id)
            .fetch_all(&state.pool)
            .await?;
    rows.iter()
        .map(row_to_input_request)
        .collect::<Result<_, _>>()
        .map(Json)
}

pub async fn get_executor_input_request(
    State(state): State<AppState>,
    Path((executor_id, run_id, input_id)): Path<(Uuid, Uuid, Uuid)>,
    Extension(principal): Extension<ExecutorPrincipal>,
) -> Result<Json<RunInputRequest>, ApiError> {
    ensure_executor_identity(executor_id, &principal)?;
    ensure_run_assigned_executor(&state.pool, run_id, executor_id).await?;
    let row = sqlx::query("SELECT * FROM run_input_requests WHERE id = $1 AND run_id = $2")
        .bind(input_id)
        .bind(run_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("input request {input_id} was not found")))?;
    Ok(Json(row_to_input_request(&row)?))
}

pub async fn submit_input_response(
    State(state): State<AppState>,
    Path((run_id, input_id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<SubmitInputResponseRequest>,
) -> Result<Json<RunInputRequest>, ApiError> {
    ensure_run_role(&state.pool, principal.user_id, run_id, ProjectRole::Viewer).await?;
    let mut tx = state.pool.begin().await?;
    let existing =
        sqlx::query("SELECT * FROM run_input_requests WHERE id = $1 AND run_id = $2 FOR UPDATE")
            .bind(input_id)
            .bind(run_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("input request {input_id} was not found")))?;
    let state_name: String = existing.try_get("state")?;
    let revision: i64 = existing.try_get("revision")?;
    if state_name != "pending" {
        return Err(ApiError::Conflict(
            "input request is no longer pending".to_owned(),
        ));
    }
    if revision != request.expected_revision {
        return Err(ApiError::Conflict(
            "input request revision changed; reload before responding".to_owned(),
        ));
    }
    let row = sqlx::query(
        r#"
        UPDATE run_input_requests SET revision = revision + 1, etag = $3,
            state = 'submitted', response = $4, responded_by = $5,
            responded_at = now()
        WHERE id = $1 AND run_id = $2 RETURNING *
        "#,
    )
    .bind(input_id)
    .bind(run_id)
    .bind(version_etag(input_id, revision + 1))
    .bind(request.response.clone())
    .bind(principal.user_id)
    .fetch_one(&mut *tx)
    .await?;
    resume_waiting_run(&mut tx, run_id, "waiting_input").await?;
    db::append_event_tx(
        &mut tx,
        run_id,
        RunEventKind::InputReceived,
        json!({"input_request_id": input_id, "response": request.response, "responded_by_user_id": principal.user_id}),
    )
    .await?;
    let record = row_to_input_request(&row)?;
    tx.commit().await?;
    Ok(Json(record))
}

pub async fn create_checkpoint(
    State(state): State<AppState>,
    Path((executor_id, run_id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<ExecutorPrincipal>,
    Json(request): Json<CreateCheckpointRequest>,
) -> Result<(StatusCode, Json<RunCheckpoint>), ApiError> {
    ensure_executor_identity(executor_id, &principal)?;
    let mut tx = state.pool.begin().await?;
    db::verify_lease(&mut tx, run_id, executor_id, request.lease_token).await?;
    if let Some(source_checkpoint_id) = request.source_checkpoint_id {
        if let Some(row) = sqlx::query("SELECT * FROM run_checkpoints WHERE id = $1")
            .bind(source_checkpoint_id)
            .fetch_optional(&mut *tx)
            .await?
        {
            let checkpoint = row_to_checkpoint(&row)?;
            if checkpoint.run_id != run_id
                || checkpoint.safe_to_resume != request.safe_to_resume
                || checkpoint.executor_state != request.executor_state
            {
                return Err(ApiError::Conflict(
                    "source_checkpoint_id was already used for different checkpoint content"
                        .to_owned(),
                ));
            }
            tx.commit().await?;
            return Ok((StatusCode::OK, Json(checkpoint)));
        }
    }
    let id = request.source_checkpoint_id.unwrap_or_else(Uuid::new_v4);
    let row = sqlx::query(
        r#"
        INSERT INTO run_checkpoints (id, run_id, sequence, safe_to_resume, executor_state)
        SELECT $1, $2, COALESCE(MAX(sequence), 0) + 1, $3, $4
        FROM run_checkpoints WHERE run_id = $2
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(run_id)
    .bind(request.safe_to_resume)
    .bind(request.executor_state)
    .fetch_one(&mut *tx)
    .await?;
    let checkpoint = row_to_checkpoint(&row)?;
    db::append_event_tx(
        &mut tx,
        run_id,
        RunEventKind::CheckpointCreated,
        json!({"checkpoint_id": id, "sequence": checkpoint.sequence, "safe_to_resume": checkpoint.safe_to_resume}),
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(checkpoint)))
}

pub async fn list_checkpoints(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<RunCheckpoint>>, ApiError> {
    ensure_run_role(&state.pool, principal.user_id, run_id, ProjectRole::Viewer).await?;
    let rows = sqlx::query("SELECT * FROM run_checkpoints WHERE run_id = $1 ORDER BY sequence")
        .bind(run_id)
        .fetch_all(&state.pool)
        .await?;
    rows.iter()
        .map(row_to_checkpoint)
        .collect::<Result<_, _>>()
        .map(Json)
}

/// Claims and triggers due schedules. The schedule row remains locked while
/// the idempotent run is inserted, so multiple worker processes cannot create
/// duplicate runs. The idempotency key also covers a crash between insertion
/// and advancing `next_run_at`.
pub async fn trigger_due_schedules(
    pool: &PgPool,
    server_capabilities: &[Capability],
    now: DateTime<Utc>,
    limit: usize,
) -> Result<usize, ApiError> {
    let mut triggered = 0;
    for _ in 0..limit.clamp(1, 100) {
        let mut tx = pool.begin().await?;
        let Some(row) = sqlx::query(
            r#"
            SELECT * FROM schedules
            WHERE enabled AND deleted_at IS NULL AND next_run_at <= $1
            ORDER BY next_run_at, id
            LIMIT 1 FOR UPDATE SKIP LOCKED
            "#,
        )
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?
        else {
            tx.commit().await?;
            break;
        };
        let schedule = row_to_schedule(&row)?;
        let due_at = schedule.next_run_at.ok_or_else(|| {
            ApiError::Internal(anyhow::anyhow!("due schedule has no next_run_at"))
        })?;
        let timezone = validated_timezone(&schedule.timezone)?;
        let cron = normalized_cron(&schedule.cron)?;
        let (missed_occurrences, catch_up_truncated) =
            count_occurrences(&cron, timezone, due_at, now)?;
        let next_run_at = next_occurrence_normalized(&cron, timezone, now)?;

        let trigger = prepare_scheduled_run(
            pool,
            server_capabilities,
            &schedule,
            due_at,
            now,
            missed_occurrences,
            catch_up_truncated,
        )
        .await;
        match trigger {
            Ok(spec) => {
                // `create_run` has its own transaction. Keeping the schedule
                // lock ensures only this worker advances the occurrence; the
                // deterministic key makes retry after a process crash safe.
                db::create_run(pool, &spec, RunState::Queued).await?;
                sqlx::query(
                    r#"
                    UPDATE schedules SET next_run_at = $2, last_triggered_at = $3,
                        blocked_reason = NULL, updated_at = now()
                    WHERE id = $1
                    "#,
                )
                .bind(schedule.id)
                .bind(next_run_at)
                .bind(now)
                .execute(&mut *tx)
                .await?;
                sync::publish_canonical_schedule_tx(&mut tx, schedule.id).await?;
                tx.commit().await?;
                triggered += 1;
            }
            Err(ScheduleBlock(reason)) => {
                sqlx::query(
                    r#"
                    UPDATE schedules SET next_run_at = $2, blocked_reason = $3,
                        updated_at = now() WHERE id = $1
                    "#,
                )
                .bind(schedule.id)
                .bind(next_run_at)
                .bind(reason)
                .execute(&mut *tx)
                .await?;
                sync::publish_canonical_schedule_tx(&mut tx, schedule.id).await?;
                tx.commit().await?;
            }
        }
    }
    Ok(triggered)
}

pub async fn expire_pending_workflows(
    pool: &PgPool,
    now: DateTime<Utc>,
) -> Result<usize, ApiError> {
    let approvals = sqlx::query(
        r#"
        UPDATE approval_requests SET state = 'expired', revision = revision + 1,
            etag = 'W/"' || id::text || ':' || (revision + 1)::text || '"'
        WHERE state = 'pending' AND expires_at <= $1 RETURNING run_id, id
        "#,
    )
    .bind(now)
    .fetch_all(pool)
    .await?;
    let inputs = sqlx::query(
        r#"
        UPDATE run_input_requests SET state = 'expired', revision = revision + 1,
            etag = 'W/"' || id::text || ':' || (revision + 1)::text || '"'
        WHERE state = 'pending' AND expires_at <= $1 RETURNING run_id, id
        "#,
    )
    .bind(now)
    .fetch_all(pool)
    .await?;
    for row in &approvals {
        let run_id: Uuid = row.try_get("run_id")?;
        if let Ok(run) = db::get_run(pool, run_id).await {
            if run.state == RunState::WaitingApproval {
                let _ = db::transition_run(pool, run_id, RunState::Expired, None, None).await;
            }
        }
    }
    for row in &inputs {
        let run_id: Uuid = row.try_get("run_id")?;
        if let Ok(run) = db::get_run(pool, run_id).await {
            if run.state == RunState::WaitingInput {
                let _ = db::transition_run(pool, run_id, RunState::Expired, None, None).await;
            }
        }
    }
    Ok(approvals.len() + inputs.len())
}

pub async fn await_worker_approval(
    pool: &PgPool,
    worker_id: Uuid,
    run_id: Uuid,
    lease_token: Uuid,
    requested_action: Value,
) -> Result<bool, ApiError> {
    let id = Uuid::new_v4();
    let expires_at = Utc::now() + Duration::days(DEFAULT_WAIT_DAYS);
    let mut tx = pool.begin().await?;
    let run = db::verify_lease(&mut tx, run_id, worker_id, lease_token).await?;
    if run.state != RunState::Running {
        return Err(ApiError::Conflict(
            "run must be running before requesting approval".to_owned(),
        ));
    }
    sqlx::query(
        r#"
        INSERT INTO approval_requests (
            id, run_id, etag, requested_action, state, expires_at
        ) VALUES ($1, $2, $3, $4, 'pending', $5)
        "#,
    )
    .bind(id)
    .bind(run_id)
    .bind(version_etag(id, 1))
    .bind(requested_action.clone())
    .bind(expires_at)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE runs SET state = 'waiting_approval', revision = revision + 1, updated_at = now() WHERE id = $1",
    )
    .bind(run_id)
    .execute(&mut *tx)
    .await?;
    db::append_event_tx(
        &mut tx,
        run_id,
        RunEventKind::ApprovalRequested,
        json!({"approval_id": id, "requested_action": requested_action, "expires_at": expires_at}),
    )
    .await?;
    db::append_event_tx(
        &mut tx,
        run_id,
        RunEventKind::StateChanged,
        json!({"from": "running", "to": "waiting_approval"}),
    )
    .await?;
    tx.commit().await?;
    loop {
        let state =
            sqlx::query_scalar::<_, String>("SELECT state FROM approval_requests WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await?
                .ok_or_else(|| ApiError::Conflict("approval request disappeared".to_owned()))?;
        match state.as_str() {
            "approved" => return Ok(true),
            "rejected" | "expired" => return Ok(false),
            "pending" => tokio::time::sleep(StdDuration::from_secs(1)).await,
            other => {
                return Err(ApiError::Internal(anyhow::anyhow!(
                    "invalid approval state: {other}"
                )))
            }
        }
    }
}

pub async fn await_worker_input(
    pool: &PgPool,
    worker_id: Uuid,
    run_id: Uuid,
    lease_token: Uuid,
    prompt: Value,
) -> Result<Option<Value>, ApiError> {
    let id = Uuid::new_v4();
    let expires_at = Utc::now() + Duration::days(DEFAULT_WAIT_DAYS);
    let mut tx = pool.begin().await?;
    let run = db::verify_lease(&mut tx, run_id, worker_id, lease_token).await?;
    if run.state != RunState::Running {
        return Err(ApiError::Conflict(
            "run must be running before requesting input".to_owned(),
        ));
    }
    sqlx::query(
        "INSERT INTO run_input_requests (id, run_id, etag, prompt, state, expires_at) VALUES ($1, $2, $3, $4, 'pending', $5)",
    )
    .bind(id)
    .bind(run_id)
    .bind(version_etag(id, 1))
    .bind(prompt.clone())
    .bind(expires_at)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE runs SET state = 'waiting_input', revision = revision + 1, updated_at = now() WHERE id = $1",
    )
    .bind(run_id)
    .execute(&mut *tx)
    .await?;
    db::append_event_tx(
        &mut tx,
        run_id,
        RunEventKind::InputRequested,
        json!({"input_request_id": id, "prompt": prompt, "expires_at": expires_at}),
    )
    .await?;
    db::append_event_tx(
        &mut tx,
        run_id,
        RunEventKind::StateChanged,
        json!({"from": "running", "to": "waiting_input"}),
    )
    .await?;
    tx.commit().await?;
    loop {
        let row = sqlx::query("SELECT state, response FROM run_input_requests WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .ok_or_else(|| ApiError::Conflict("input request disappeared".to_owned()))?;
        match row.try_get::<String, _>("state")?.as_str() {
            "submitted" => return Ok(row.try_get("response")?),
            "expired" => return Ok(None),
            "pending" => tokio::time::sleep(StdDuration::from_secs(1)).await,
            other => {
                return Err(ApiError::Internal(anyhow::anyhow!(
                    "invalid input request state: {other}"
                )))
            }
        }
    }
}

pub async fn create_worker_checkpoint(
    pool: &PgPool,
    worker_id: Uuid,
    run_id: Uuid,
    lease_token: Uuid,
    safe_to_resume: bool,
    executor_state: Value,
) -> Result<RunCheckpoint, ApiError> {
    let mut tx = pool.begin().await?;
    db::verify_lease(&mut tx, run_id, worker_id, lease_token).await?;
    let id = Uuid::new_v4();
    let row = sqlx::query(
        r#"
        INSERT INTO run_checkpoints (id, run_id, sequence, safe_to_resume, executor_state)
        SELECT $1, $2, COALESCE(MAX(sequence), 0) + 1, $3, $4
        FROM run_checkpoints WHERE run_id = $2 RETURNING *
        "#,
    )
    .bind(id)
    .bind(run_id)
    .bind(safe_to_resume)
    .bind(executor_state)
    .fetch_one(&mut *tx)
    .await?;
    let checkpoint = row_to_checkpoint(&row)?;
    db::append_event_tx(
        &mut tx,
        run_id,
        RunEventKind::CheckpointCreated,
        json!({"checkpoint_id": id, "sequence": checkpoint.sequence, "safe_to_resume": safe_to_resume}),
    )
    .await?;
    tx.commit().await?;
    Ok(checkpoint)
}

struct ScheduleBlock(String);

#[allow(clippy::too_many_arguments)]
async fn prepare_scheduled_run(
    pool: &PgPool,
    server_capabilities: &[Capability],
    schedule: &ScheduleRecord,
    due_at: DateTime<Utc>,
    now: DateTime<Utc>,
    missed_occurrences: usize,
    catch_up_truncated: bool,
) -> Result<RunSpec, ScheduleBlock> {
    organization::ensure_project_role(
        pool,
        schedule_created_by(pool, schedule.id)
            .await
            .map_err(block)?,
        schedule.project_id,
        ProjectRole::Runner,
    )
    .await
    .map_err(block)?;
    let project = sqlx::query(
        "SELECT revision, privacy, current_version_id FROM projects WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(schedule.project_id)
    .fetch_optional(pool)
    .await
    .map_err(block)?
    .ok_or_else(|| ScheduleBlock("project_missing".to_owned()))?;
    let project_revision: i64 = project.try_get("revision").map_err(block)?;
    let privacy =
        parse_project_privacy(project.try_get("privacy").map_err(block)?).map_err(block)?;
    let current_version_id: Option<Uuid> = project.try_get("current_version_id").map_err(block)?;
    let task_row = sqlx::query(
        "SELECT * FROM task_definitions WHERE id = $1 AND project_id = $2 AND released AND deleted_at IS NULL LIMIT 1",
    )
    .bind(schedule.task_id)
    .bind(schedule.project_id)
    .fetch_optional(pool)
    .await
    .map_err(block)?
    .ok_or_else(|| ScheduleBlock("task_has_no_released_version".to_owned()))?;
    let task = row_to_task(&task_row).map_err(block)?;

    match &schedule.executor_target {
        ExecutorTarget::ServerLinux {
            pool_id: Some(pool_id),
        } => {
            organization::ensure_pool_allowed_for_project(
                pool,
                *pool_id,
                schedule.project_id,
                ExecutorKind::ServerLinux,
            )
            .await
            .map_err(block)?;
        }
        ExecutorTarget::ManagedWindowsPool { pool_id } => {
            organization::ensure_pool_allowed_for_project(
                pool,
                *pool_id,
                schedule.project_id,
                ExecutorKind::ManagedWindows,
            )
            .await
            .map_err(block)?;
        }
        ExecutorTarget::PersonalDevice { .. } | ExecutorTarget::ServerLinux { pool_id: None } => {}
    }

    let target_available = match &schedule.executor_target {
        ExecutorTarget::ServerLinux { .. } => {
            let available: std::collections::HashSet<&str> = server_capabilities
                .iter()
                .map(|capability| capability.0.as_str())
                .collect();
            task.required_capabilities
                .iter()
                .all(|capability| available.contains(capability.0.as_str()))
        }
        target => db::target_has_executor(pool, target, &task.required_capabilities)
            .await
            .map_err(block)?,
    };
    if !target_available {
        return Err(ScheduleBlock(
            "required_executor_or_capability_unavailable".to_owned(),
        ));
    }

    let remote_target = !matches!(
        schedule.executor_target,
        ExecutorTarget::PersonalDevice { .. }
    );
    let snapshot_id = if privacy == ProjectPrivacy::PrivateLocal && remote_target {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM snapshot_manifests WHERE project_id = $1 AND status = 'ready' AND (expires_at IS NULL OR expires_at > now()) ORDER BY created_at DESC LIMIT 1",
        )
        .bind(schedule.project_id)
        .fetch_optional(pool)
        .await
        .map_err(block)?
        .ok_or_else(|| ScheduleBlock("private_project_snapshot_required".to_owned()))?
        .into()
    } else if let Some(version_id) = current_version_id {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT snapshot_manifest_id FROM project_versions WHERE id = $1",
        )
        .bind(version_id)
        .fetch_optional(pool)
        .await
        .map_err(block)?
    } else {
        None
    };
    let creator_user_id = schedule_created_by(pool, schedule.id)
        .await
        .map_err(block)?;
    let input = scheduled_input(
        schedule.input.clone(),
        &task.instructions,
        schedule.id,
        due_at,
        now,
        missed_occurrences,
        catch_up_truncated,
    );
    Ok(RunSpec {
        schema_version: SCHEMA_VERSION,
        id: Uuid::new_v4(),
        thread_id: schedule.thread_id,
        project_id: schedule.project_id,
        project: FrozenReference {
            id: schedule.project_id,
            revision: project_revision,
        },
        project_privacy: privacy,
        task: Some(FrozenReference {
            id: task.id,
            revision: task.revision,
        }),
        creator_user_id,
        executor_target: schedule.executor_target.clone(),
        required_capabilities: task.required_capabilities,
        input,
        model_profile_id: schedule.model_profile_id,
        snapshot_id,
        idempotency_key: format!("schedule:{}:{}", schedule.id, due_at.timestamp_millis()),
        created_at: now,
    })
}

fn scheduled_input(
    input: Value,
    instructions: &str,
    schedule_id: Uuid,
    due_at: DateTime<Utc>,
    triggered_at: DateTime<Utc>,
    missed_occurrences: usize,
    catch_up_truncated: bool,
) -> Value {
    let mut object = match input {
        Value::Object(object) => object,
        Value::Null => Map::new(),
        other => {
            let mut object = Map::new();
            object.insert("input".to_owned(), other);
            object
        }
    };
    object
        .entry("prompt".to_owned())
        .or_insert_with(|| Value::String(instructions.to_owned()));
    object.insert(
        "task_instructions".to_owned(),
        Value::String(instructions.to_owned()),
    );
    object.insert(
        "_schedule".to_owned(),
        json!({
            "schedule_id": schedule_id,
            "first_due_at": due_at,
            "triggered_at": triggered_at,
            "missed_occurrences": missed_occurrences,
            "catch_up_truncated": catch_up_truncated
        }),
    );
    Value::Object(object)
}

async fn resume_waiting_run(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    expected_state: &str,
) -> Result<(), ApiError> {
    let row = sqlx::query(
        "SELECT state, lease_expires_at, assigned_executor_id FROM runs WHERE id = $1 FOR UPDATE",
    )
    .bind(run_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("run {run_id} was not found")))?;
    let current: String = row.try_get("state")?;
    if current != expected_state {
        return Err(ApiError::Conflict(format!(
            "run is {current}, expected {expected_state}"
        )));
    }
    let lease_expires_at: Option<DateTime<Utc>> = row.try_get("lease_expires_at")?;
    if lease_expires_at.is_some_and(|expires_at| expires_at > Utc::now()) {
        sqlx::query(
            "UPDATE runs SET state = 'running', revision = revision + 1, updated_at = now() WHERE id = $1",
        )
        .bind(run_id)
        .execute(&mut **tx)
        .await?;
        db::append_event_tx(
            tx,
            run_id,
            RunEventKind::StateChanged,
            json!({"from": expected_state, "to": "running"}),
        )
        .await?;
    } else {
        let executor_id: Option<Uuid> = row.try_get("assigned_executor_id")?;
        sqlx::query(
            r#"
            UPDATE runs SET state = 'interrupted', revision = revision + 1,
                lease_owner = NULL, lease_token = NULL, lease_expires_at = NULL,
                error = jsonb_build_object(
                    'code', 'executor_lease_expired_while_waiting',
                    'message', 'The executor disconnected while the run was waiting.',
                    'retryable', false,
                    'details', '{}'::jsonb
                ), updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(run_id)
        .execute(&mut **tx)
        .await?;
        if let Some(executor_id) = executor_id {
            sqlx::query(
                "UPDATE executors SET active_runs = GREATEST(active_runs - 1, 0) WHERE id = $1",
            )
            .bind(executor_id)
            .execute(&mut **tx)
            .await?;
        }
        db::append_event_tx(
            tx,
            run_id,
            RunEventKind::StateChanged,
            json!({"from": expected_state, "to": "interrupted", "reason": "executor_lease_expired_while_waiting"}),
        )
        .await?;
    }
    Ok(())
}

async fn ensure_run_role(
    pool: &PgPool,
    user_id: Uuid,
    run_id: Uuid,
    role: ProjectRole,
) -> Result<(), ApiError> {
    let row = sqlx::query("SELECT project_id, thread_id FROM runs WHERE id = $1")
        .bind(run_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("run {run_id} was not found")))?;
    organization::ensure_thread_role(
        pool,
        user_id,
        row.try_get("project_id")?,
        row.try_get("thread_id")?,
        role,
    )
    .await?;
    Ok(())
}

async fn ensure_run_assigned_executor(
    pool: &PgPool,
    run_id: Uuid,
    executor_id: Uuid,
) -> Result<(), ApiError> {
    let assigned = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM runs WHERE id = $1 AND assigned_executor_id = $2)",
    )
    .bind(run_id)
    .bind(executor_id)
    .fetch_one(pool)
    .await?;
    if assigned {
        Ok(())
    } else {
        Err(ApiError::Unauthorized(
            "the run is not assigned to this executor".to_owned(),
        ))
    }
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

async fn validate_target_for_project(
    pool: &PgPool,
    user_id: Uuid,
    project_id: Uuid,
    target: &ExecutorTarget,
) -> Result<(), ApiError> {
    match target {
        ExecutorTarget::ServerLinux {
            pool_id: Some(pool_id),
        } => {
            organization::ensure_pool_allowed_for_project(
                pool,
                *pool_id,
                project_id,
                ExecutorKind::ServerLinux,
            )
            .await
        }
        ExecutorTarget::ManagedWindowsPool { pool_id } => {
            organization::ensure_pool_allowed_for_project(
                pool,
                *pool_id,
                project_id,
                ExecutorKind::ManagedWindows,
            )
            .await
        }
        ExecutorTarget::PersonalDevice { device_id } => {
            if db::user_can_target_personal_device(pool, user_id, *device_id).await? {
                Ok(())
            } else {
                Err(ApiError::Unauthorized(
                    "personal devices can only be targeted by their owner".to_owned(),
                ))
            }
        }
        ExecutorTarget::ServerLinux { pool_id: None } => Ok(()),
    }
}

async fn ensure_task_project(
    pool: &PgPool,
    task_id: Uuid,
    project_id: Uuid,
) -> Result<(), ApiError> {
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM task_definitions WHERE id = $1 AND project_id = $2 AND deleted_at IS NULL)",
    )
    .bind(task_id)
    .bind(project_id)
    .fetch_one(pool)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::Unprocessable(
            "task does not belong to the selected project".to_owned(),
        ))
    }
}

async fn schedule_created_by(pool: &PgPool, schedule_id: Uuid) -> Result<Uuid, ApiError> {
    sqlx::query_scalar("SELECT created_by FROM schedules WHERE id = $1")
        .bind(schedule_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("schedule {schedule_id} was not found")))
}

fn validate_task_fields(name: &str, instructions: &str) -> Result<(), ApiError> {
    if name.trim().is_empty() || name.len() > 200 {
        return Err(ApiError::Unprocessable(
            "task name must contain 1 to 200 characters".to_owned(),
        ));
    }
    if instructions.trim().is_empty() || instructions.len() > 1_000_000 {
        return Err(ApiError::Unprocessable(
            "task instructions must contain 1 to 1000000 characters".to_owned(),
        ));
    }
    Ok(())
}

fn validated_expiry(value: Option<DateTime<Utc>>) -> Result<DateTime<Utc>, ApiError> {
    let now = Utc::now();
    let expires_at = value.unwrap_or_else(|| now + Duration::days(DEFAULT_WAIT_DAYS));
    if expires_at <= now + Duration::minutes(1)
        || expires_at > now + Duration::days(DEFAULT_WAIT_DAYS)
    {
        return Err(ApiError::Unprocessable(
            "request expiry must be between one minute and seven days".to_owned(),
        ));
    }
    Ok(expires_at)
}

pub(crate) fn normalized_cron(expression: &str) -> Result<String, ApiError> {
    let expression = expression.trim();
    let fields = expression.split_whitespace().count();
    let normalized = match fields {
        5 => format!("0 {expression}"),
        6 | 7 => expression.to_owned(),
        _ => {
            return Err(ApiError::Unprocessable(
                "cron must have five, six, or seven fields".to_owned(),
            ))
        }
    };
    Schedule::from_str(&normalized)
        .map_err(|error| ApiError::Unprocessable(format!("invalid cron expression: {error}")))?;
    Ok(normalized)
}

pub(crate) fn validated_timezone(value: &str) -> Result<Tz, ApiError> {
    value
        .parse::<Tz>()
        .map_err(|_| ApiError::Unprocessable(format!("{value} is not a recognized IANA timezone")))
}

fn next_occurrence(
    expression: &str,
    timezone: &str,
    after: DateTime<Utc>,
) -> Result<DateTime<Utc>, ApiError> {
    let cron = normalized_cron(expression)?;
    next_occurrence_normalized(&cron, validated_timezone(timezone)?, after)
}

pub(crate) fn next_occurrence_normalized(
    expression: &str,
    timezone: Tz,
    after: DateTime<Utc>,
) -> Result<DateTime<Utc>, ApiError> {
    let schedule = Schedule::from_str(expression)
        .map_err(|error| ApiError::Unprocessable(format!("invalid cron expression: {error}")))?;
    schedule
        .after(&after.with_timezone(&timezone))
        .next()
        .map(|value| value.with_timezone(&Utc))
        .ok_or_else(|| ApiError::Unprocessable("cron has no future occurrence".to_owned()))
}

fn count_occurrences(
    expression: &str,
    timezone: Tz,
    first_due: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<(usize, bool), ApiError> {
    let schedule = Schedule::from_str(expression)
        .map_err(|error| ApiError::Unprocessable(format!("invalid cron expression: {error}")))?;
    let just_before = first_due - Duration::nanoseconds(1);
    let mut count = 0;
    for occurrence in schedule.after(&just_before.with_timezone(&timezone)) {
        if occurrence.with_timezone(&Utc) > now {
            break;
        }
        count += 1;
        if count == MAX_SCHEDULE_CATCH_UPS {
            return Ok((count, true));
        }
    }
    Ok((count.max(1), false))
}

fn parse_project_privacy(value: &str) -> Result<ProjectPrivacy, ApiError> {
    match value {
        "private_local" => Ok(ProjectPrivacy::PrivateLocal),
        "team_managed" => Ok(ProjectPrivacy::TeamManaged),
        other => Err(ApiError::Internal(anyhow::anyhow!(
            "invalid project privacy in database: {other}"
        ))),
    }
}

fn row_to_task(row: &PgRow) -> Result<TaskDefinition, ApiError> {
    Ok(TaskDefinition {
        schema_version: SCHEMA_VERSION,
        id: row.try_get("id")?,
        revision: row.try_get("revision")?,
        etag: row.try_get("etag")?,
        project_id: row.try_get("project_id")?,
        name: row.try_get("name")?,
        instructions: row.try_get("instructions")?,
        required_capabilities: serde_json::from_value(row.try_get("required_capabilities")?)?,
        default_target: row
            .try_get::<Option<Value>, _>("default_executor_target")?
            .map(serde_json::from_value)
            .transpose()?,
        config: row.try_get("config")?,
        released: row.try_get("released")?,
        created_at: row.try_get("created_at")?,
        deleted_at: row.try_get("deleted_at")?,
    })
}

fn row_to_schedule(row: &PgRow) -> Result<ScheduleRecord, ApiError> {
    Ok(ScheduleRecord {
        schema_version: SCHEMA_VERSION,
        id: row.try_get("id")?,
        revision: row.try_get("revision")?,
        etag: row.try_get("etag")?,
        task_id: row.try_get("task_id")?,
        project_id: row.try_get("project_id")?,
        thread_id: row.try_get("thread_id")?,
        cron: row.try_get("cron_expression")?,
        timezone: row.try_get("timezone")?,
        executor_target: serde_json::from_value(row.try_get("executor_target")?)?,
        input: row.try_get("input")?,
        model_profile_id: row.try_get("model_profile_id")?,
        enabled: row.try_get("enabled")?,
        next_run_at: row.try_get("next_run_at")?,
        last_triggered_at: row.try_get("last_triggered_at")?,
        blocked_reason: row.try_get("blocked_reason")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        deleted_at: row.try_get("deleted_at")?,
    })
}

fn row_to_approval(row: &PgRow) -> Result<ApprovalRequest, ApiError> {
    Ok(ApprovalRequest {
        schema_version: SCHEMA_VERSION,
        id: row.try_get("id")?,
        run_id: row.try_get("run_id")?,
        revision: row.try_get("revision")?,
        etag: row.try_get("etag")?,
        requested_action: row.try_get("requested_action")?,
        state: match row.try_get::<String, _>("state")?.as_str() {
            "pending" => ApprovalState::Pending,
            "approved" => ApprovalState::Approved,
            "rejected" => ApprovalState::Rejected,
            "expired" => ApprovalState::Expired,
            other => {
                return Err(ApiError::Internal(anyhow::anyhow!(
                    "invalid approval state in database: {other}"
                )))
            }
        },
        requested_at: row.try_get("requested_at")?,
        expires_at: row.try_get("expires_at")?,
        resolved_by_user_id: row.try_get("resolved_by")?,
        resolved_at: row.try_get("resolved_at")?,
    })
}

fn row_to_input_request(row: &PgRow) -> Result<RunInputRequest, ApiError> {
    Ok(RunInputRequest {
        schema_version: SCHEMA_VERSION,
        id: row.try_get("id")?,
        run_id: row.try_get("run_id")?,
        revision: row.try_get("revision")?,
        etag: row.try_get("etag")?,
        prompt: row.try_get("prompt")?,
        state: match row.try_get::<String, _>("state")?.as_str() {
            "pending" => InputRequestState::Pending,
            "submitted" => InputRequestState::Submitted,
            "expired" => InputRequestState::Expired,
            other => {
                return Err(ApiError::Internal(anyhow::anyhow!(
                    "invalid input request state in database: {other}"
                )))
            }
        },
        response: row.try_get("response")?,
        requested_at: row.try_get("requested_at")?,
        expires_at: row.try_get("expires_at")?,
        responded_by_user_id: row.try_get("responded_by")?,
        responded_at: row.try_get("responded_at")?,
    })
}

fn row_to_checkpoint(row: &PgRow) -> Result<RunCheckpoint, ApiError> {
    Ok(RunCheckpoint {
        schema_version: SCHEMA_VERSION,
        id: row.try_get("id")?,
        run_id: row.try_get("run_id")?,
        sequence: row.try_get("sequence")?,
        safe_to_resume: row.try_get("safe_to_resume")?,
        executor_state: row.try_get("executor_state")?,
        created_at: row.try_get("created_at")?,
    })
}

fn version_etag(id: Uuid, revision: i64) -> String {
    format!("W/\"{id}:{revision}\"")
}

fn block(error: impl std::fmt::Display) -> ScheduleBlock {
    ScheduleBlock(error.to_string())
}

#[cfg(test)]
mod tests {
    use chrono::{Datelike, TimeZone, Timelike};

    use super::*;

    #[test]
    fn five_field_cron_is_normalized() {
        assert_eq!(normalized_cron("30 9 * * 1-5").unwrap(), "0 30 9 * * 1-5");
    }

    #[test]
    fn berlin_schedule_keeps_local_time_across_dst() {
        let before_dst = Utc.with_ymd_and_hms(2026, 3, 28, 12, 0, 0).unwrap();
        let next = next_occurrence("0 9 * * *", "Europe/Berlin", before_dst).unwrap();
        let local = next.with_timezone(&chrono_tz::Europe::Berlin);
        assert_eq!((local.year(), local.month(), local.day()), (2026, 3, 29));
        assert_eq!((local.hour(), local.minute()), (9, 0));

        let after_dst = Utc.with_ymd_and_hms(2026, 10, 24, 12, 0, 0).unwrap();
        let next = next_occurrence("0 9 * * *", "Europe/Berlin", after_dst).unwrap();
        let local = next.with_timezone(&chrono_tz::Europe::Berlin);
        assert_eq!((local.year(), local.month(), local.day()), (2026, 10, 25));
        assert_eq!((local.hour(), local.minute()), (9, 0));
    }

    #[test]
    fn catch_up_counts_occurrences_but_creates_one_trigger() {
        let first = Utc.with_ymd_and_hms(2026, 1, 1, 9, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 1, 4, 9, 1, 0).unwrap();
        let (count, truncated) = count_occurrences(
            &normalized_cron("0 9 * * *").unwrap(),
            chrono_tz::UTC,
            first,
            now,
        )
        .unwrap();
        assert_eq!(count, 4);
        assert!(!truncated);
    }

    #[test]
    fn default_wait_is_bounded() {
        let expiry = validated_expiry(None).unwrap();
        let remaining = expiry - Utc::now();
        assert!(remaining > Duration::days(6));
        assert!(remaining <= Duration::days(7) + Duration::seconds(1));
    }

    #[test]
    fn scheduled_input_keeps_task_instructions_with_an_explicit_prompt() {
        let due = Utc.with_ymd_and_hms(2026, 1, 1, 9, 0, 0).unwrap();
        let input = scheduled_input(
            json!({"prompt": "today's customer input"}),
            "released task instructions",
            Uuid::new_v4(),
            due,
            due,
            1,
            false,
        );
        assert_eq!(input["prompt"], "today's customer input");
        assert_eq!(input["task_instructions"], "released task instructions");
    }
}
