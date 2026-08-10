use std::collections::HashSet;

use chrono::{DateTime, Utc};
use cowork_contracts::{
    ensure_run_transition, Capability, ExecutorKind, ExecutorRecord, ExecutorRegistration,
    ExecutorTarget, MessageRecord, MessageRole, RunError, RunEvent, RunEventKind, RunLease,
    RunRecord, RunSpec, RunState, SCHEMA_VERSION,
};
use serde_json::{json, Value};
use sqlx::{postgres::PgRow, PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{error::ApiError, governance, sync};

const TERMINAL_STATES: &[&str] = &["completed", "failed", "canceled", "expired"];

pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}

pub async fn create_run(
    pool: &PgPool,
    spec: &RunSpec,
    initial_state: RunState,
) -> Result<RunRecord, ApiError> {
    let mut tx = pool.begin().await?;
    let (record, _) = create_run_tx(&mut tx, spec, initial_state).await?;
    tx.commit().await?;
    Ok(record)
}

async fn create_run_tx(
    tx: &mut Transaction<'_, Postgres>,
    spec: &RunSpec,
    initial_state: RunState,
) -> Result<(RunRecord, bool), ApiError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("quota:user:{}", spec.creator_user_id))
        .execute(&mut **tx)
        .await?;
    if let Some(row) =
        sqlx::query("SELECT * FROM runs WHERE creator_user_id = $1 AND idempotency_key = $2")
            .bind(spec.creator_user_id)
            .bind(&spec.idempotency_key)
            .fetch_optional(&mut **tx)
            .await?
    {
        return Ok((row_to_run(&row)?, false));
    }
    governance::enforce_run_quota_tx(tx, spec.creator_user_id, spec.project_id).await?;
    let (target_kind, pool_id, device_id) = target_columns(&spec.executor_target);
    let spec_json = serde_json::to_value(spec)?;
    let state = state_name(initial_state);

    let inserted = sqlx::query(
        r#"
        INSERT INTO runs (
            id, thread_id, project_id, creator_user_id, idempotency_key,
            target_kind, target_pool_id, target_device_id, state, spec,
            snapshot_id, created_at, updated_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $12)
        ON CONFLICT (creator_user_id, idempotency_key) DO NOTHING
        RETURNING *
        "#,
    )
    .bind(spec.id)
    .bind(spec.thread_id)
    .bind(spec.project_id)
    .bind(spec.creator_user_id)
    .bind(&spec.idempotency_key)
    .bind(target_kind)
    .bind(pool_id)
    .bind(device_id)
    .bind(state)
    .bind(spec_json)
    .bind(spec.snapshot_id)
    .bind(spec.created_at)
    .fetch_optional(&mut **tx)
    .await?;

    let (record, inserted) = if let Some(row) = inserted {
        append_event_tx(tx, spec.id, RunEventKind::Created, json!({"state": state})).await?;
        (row_to_run(&row)?, true)
    } else {
        let row =
            sqlx::query("SELECT * FROM runs WHERE creator_user_id = $1 AND idempotency_key = $2")
                .bind(spec.creator_user_id)
                .bind(&spec.idempotency_key)
                .fetch_one(&mut **tx)
                .await?;
        (row_to_run(&row)?, false)
    };
    Ok((record, inserted))
}

pub async fn create_thread_message_run(
    pool: &PgPool,
    spec: &RunSpec,
    initial_state: RunState,
    content: Value,
) -> Result<(MessageRecord, RunRecord), ApiError> {
    let mut tx = pool.begin().await?;
    let (run, inserted) = create_run_tx(&mut tx, spec, initial_state).await?;
    if run.spec.thread_id != spec.thread_id {
        return Err(ApiError::Conflict(
            "idempotency key belongs to a run in a different thread".to_owned(),
        ));
    }
    let row = if inserted {
        let message_id = Uuid::new_v4();
        let etag = format!("W/\"{message_id}:1\"");
        let row = sqlx::query(
            r#"
            INSERT INTO messages (
                id, etag, thread_id, author_user_id, role, content, run_id
            ) VALUES ($1, $2, $3, $4, 'user', $5, $6)
            RETURNING *
            "#,
        )
        .bind(message_id)
        .bind(etag)
        .bind(spec.thread_id)
        .bind(spec.creator_user_id)
        .bind(content)
        .bind(run.spec.id)
        .fetch_one(&mut *tx)
        .await?;
        touch_thread_tx(&mut tx, spec.thread_id).await?;
        sync::publish_canonical_message_tx(&mut tx, message_id).await?;
        row
    } else {
        sqlx::query(
            "SELECT * FROM messages WHERE run_id = $1 AND role = 'user' AND deleted_at IS NULL",
        )
        .bind(run.spec.id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| {
            ApiError::Conflict(
                "idempotency key belongs to a run that was not created from a chat message"
                    .to_owned(),
            )
        })?
    };
    let message = row_to_message(&row)?;
    tx.commit().await?;
    Ok((message, run))
}

pub async fn list_thread_messages(
    pool: &PgPool,
    thread_id: Uuid,
    limit: i64,
) -> Result<Vec<MessageRecord>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT * FROM messages
        WHERE thread_id = $1 AND deleted_at IS NULL
        ORDER BY created_at, id
        LIMIT $2
        "#,
    )
    .bind(thread_id)
    .bind(limit.clamp(1, 1_000))
    .fetch_all(pool)
    .await?;
    rows.iter().map(row_to_message).collect()
}

pub async fn get_run(pool: &PgPool, run_id: Uuid) -> Result<RunRecord, ApiError> {
    let row = sqlx::query("SELECT * FROM runs WHERE id = $1")
        .bind(run_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("run {run_id} was not found")))?;
    row_to_run(&row)
}

pub async fn list_runs_for_user(
    pool: &PgPool,
    user_id: Uuid,
    limit: i64,
) -> Result<Vec<RunRecord>, ApiError> {
    let rows = sqlx::query(
        r#"
            SELECT DISTINCT run.* FROM runs run
            JOIN projects project ON project.id = run.project_id AND project.deleted_at IS NULL
            LEFT JOIN project_members pm ON pm.project_id = project.id AND pm.user_id = $1
            LEFT JOIN team_members tm ON tm.team_id = project.team_id AND tm.user_id = $1
            LEFT JOIN support_grants sg ON sg.support_user_id = $1
              AND sg.revoked_at IS NULL AND sg.expires_at > now()
              AND (sg.project_id = run.project_id OR sg.thread_id = run.thread_id)
            WHERE run.creator_user_id = $1 OR project.owner_user_id = $1
               OR pm.user_id IS NOT NULL OR tm.user_id IS NOT NULL OR sg.id IS NOT NULL
            ORDER BY run.created_at DESC, run.id DESC LIMIT $2
            "#,
    )
    .bind(user_id)
    .bind(limit.clamp(1, 200))
    .fetch_all(pool)
    .await?;
    rows.iter().map(row_to_run).collect()
}

pub async fn get_run_events(
    pool: &PgPool,
    run_id: Uuid,
    after: i64,
    limit: i64,
) -> Result<Vec<RunEvent>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT run_id, sequence, event_id, kind, payload, created_at
        FROM run_events
        WHERE run_id = $1 AND sequence > $2
        ORDER BY sequence ASC
        LIMIT $3
        "#,
    )
    .bind(run_id)
    .bind(after)
    .bind(limit.clamp(1, 1_000))
    .fetch_all(pool)
    .await?;
    rows.iter().map(row_to_event).collect()
}

pub async fn cancel_run(pool: &PgPool, run_id: Uuid) -> Result<RunRecord, ApiError> {
    transition_run(pool, run_id, RunState::Canceled, None, None).await
}

pub async fn register_executor(
    pool: &PgPool,
    registration: &ExecutorRegistration,
) -> Result<ExecutorRecord, ApiError> {
    cowork_contracts::ensure_compatible(registration.schema_version)
        .map_err(|error| ApiError::Unprocessable(error.to_string()))?;
    if registration.max_concurrent_runs == 0 {
        return Err(ApiError::Unprocessable(
            "max_concurrent_runs must be at least one".to_owned(),
        ));
    }
    if registration.kind == ExecutorKind::PersonalDevice && registration.owner_user_id.is_none() {
        return Err(ApiError::Unprocessable(
            "personal devices require owner_user_id".to_owned(),
        ));
    }

    let now = Utc::now();
    let row = sqlx::query(
        r#"
        INSERT INTO executors (
            id, kind, pool_id, owner_user_id, registration, protocol_version,
            max_concurrent_runs, last_seen_at, updated_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8)
        ON CONFLICT (id) DO UPDATE SET
            kind = EXCLUDED.kind,
            pool_id = EXCLUDED.pool_id,
            owner_user_id = EXCLUDED.owner_user_id,
            registration = EXCLUDED.registration,
            protocol_version = EXCLUDED.protocol_version,
            max_concurrent_runs = EXCLUDED.max_concurrent_runs,
            last_seen_at = EXCLUDED.last_seen_at,
            updated_at = EXCLUDED.updated_at
        RETURNING *
        "#,
    )
    .bind(registration.executor_id)
    .bind(executor_kind_name(registration.kind))
    .bind(registration.pool_id)
    .bind(registration.owner_user_id)
    .bind(serde_json::to_value(registration)?)
    .bind(i32::from(registration.protocol_version))
    .bind(i32::from(registration.max_concurrent_runs))
    .bind(now)
    .fetch_one(pool)
    .await?;
    row_to_executor(&row)
}

pub async fn refresh_executor_registration(
    pool: &PgPool,
    executor_id: Uuid,
    registration: &ExecutorRegistration,
) -> Result<ExecutorRecord, ApiError> {
    if registration.executor_id != executor_id {
        return Err(ApiError::Unauthorized(
            "executor credential and registration ID do not match".to_owned(),
        ));
    }
    cowork_contracts::ensure_compatible(registration.schema_version)
        .map_err(|error| ApiError::Unprocessable(error.to_string()))?;
    let existing = sqlx::query("SELECT * FROM executors WHERE id = $1")
        .bind(executor_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("executor {executor_id} was not found")))?;
    let existing = row_to_executor(&existing)?;
    let mut registration = registration.clone();
    if registration.kind == ExecutorKind::PersonalDevice && registration.owner_user_id.is_none() {
        registration.owner_user_id = existing.registration.owner_user_id;
    }
    if registration.kind == ExecutorKind::PersonalDevice {
        // An executor credential may refresh health/capability metadata, but it
        // cannot relax the owner's server-side remote-control ceiling.
        registration.personal_device_remote_control = Some(
            existing
                .registration
                .personal_device_remote_control
                .unwrap_or_default(),
        );
    }
    if registration.kind != ExecutorKind::PersonalDevice
        && registration.personal_device_remote_control.is_some()
    {
        return Err(ApiError::Unprocessable(
            "personal_device_remote_control is only valid for personal devices".to_owned(),
        ));
    }
    if existing.registration.kind != registration.kind
        || existing.registration.pool_id != registration.pool_id
        || existing.registration.owner_user_id != registration.owner_user_id
    {
        return Err(ApiError::Conflict(
            "an executor cannot change its kind, pool, or owner through self-registration"
                .to_owned(),
        ));
    }
    if registration.max_concurrent_runs == 0 {
        return Err(ApiError::Unprocessable(
            "max_concurrent_runs must be at least one".to_owned(),
        ));
    }
    let row = sqlx::query(
        r#"
        UPDATE executors SET registration = $2, protocol_version = $3,
            max_concurrent_runs = $4, last_seen_at = now(), updated_at = now()
        WHERE id = $1 RETURNING *
        "#,
    )
    .bind(executor_id)
    .bind(serde_json::to_value(&registration)?)
    .bind(i32::from(registration.protocol_version))
    .bind(i32::from(registration.max_concurrent_runs))
    .fetch_one(pool)
    .await?;
    row_to_executor(&row)
}

pub async fn heartbeat_executor(
    pool: &PgPool,
    executor_id: Uuid,
    active_runs: usize,
) -> Result<ExecutorRecord, ApiError> {
    let active_runs = i32::try_from(active_runs)
        .map_err(|_| ApiError::BadRequest("too many active run IDs".to_owned()))?;
    let row = sqlx::query(
        r#"
        UPDATE executors
        SET last_seen_at = now(), active_runs = $2, updated_at = now()
        WHERE id = $1
        RETURNING *
        "#,
    )
    .bind(executor_id)
    .bind(active_runs)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("executor {executor_id} was not found")))?;
    row_to_executor(&row)
}

pub async fn list_executors_for_user(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Vec<ExecutorRecord>, ApiError> {
    let is_admin = user_is_platform_admin(pool, user_id).await?;
    let rows = if is_admin {
        sqlx::query("SELECT * FROM executors ORDER BY last_seen_at DESC")
            .fetch_all(pool)
            .await?
    } else {
        sqlx::query(
            r#"
            SELECT DISTINCT executor.* FROM executors executor
            LEFT JOIN executor_pools pool ON pool.id = executor.pool_id AND pool.deleted_at IS NULL
            LEFT JOIN team_members pool_tm ON pool_tm.team_id = pool.team_id AND pool_tm.user_id = $1
            LEFT JOIN executor_pool_project_grants grant_row ON grant_row.pool_id = pool.id
            LEFT JOIN projects project ON project.id = grant_row.project_id AND project.deleted_at IS NULL
            LEFT JOIN project_members pm ON pm.project_id = project.id AND pm.user_id = $1
            LEFT JOIN team_members project_tm ON project_tm.team_id = project.team_id AND project_tm.user_id = $1
            WHERE (executor.kind = 'personal_device' AND executor.owner_user_id = $1)
               OR (executor.kind <> 'personal_device' AND (
                    pool_tm.user_id IS NOT NULL OR project.owner_user_id = $1
                    OR pm.user_id IS NOT NULL OR project_tm.user_id IS NOT NULL
               ))
            ORDER BY executor.last_seen_at DESC
            "#,
        )
        .bind(user_id)
        .fetch_all(pool)
        .await?
    };
    rows.iter().map(row_to_executor).collect()
}

pub async fn user_is_platform_admin(pool: &PgPool, user_id: Uuid) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT platform_admin FROM users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?
    .unwrap_or(false))
}

pub async fn user_can_manage_executor(
    pool: &PgPool,
    user_id: Uuid,
    executor_id: Uuid,
) -> Result<bool, ApiError> {
    if user_is_platform_admin(pool, user_id).await? {
        return Ok(true);
    }
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM executors WHERE id = $1 AND kind = 'personal_device' AND owner_user_id = $2)",
    )
    .bind(executor_id)
    .bind(user_id)
    .fetch_one(pool)
    .await?)
}

pub async fn user_can_target_personal_device(
    pool: &PgPool,
    user_id: Uuid,
    executor_id: Uuid,
) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM executors WHERE id = $1 AND kind = 'personal_device' AND owner_user_id = $2)",
    )
    .bind(executor_id)
    .bind(user_id)
    .fetch_one(pool)
    .await?)
}

pub async fn target_has_executor(
    pool: &PgPool,
    target: &ExecutorTarget,
    required: &[Capability],
) -> Result<bool, ApiError> {
    let rows = match target {
        ExecutorTarget::ServerLinux { .. } => return Ok(true),
        ExecutorTarget::ManagedWindowsPool { pool_id } => {
            sqlx::query(
                "SELECT * FROM executors WHERE kind = 'managed_windows' AND pool_id = $1 AND NOT draining AND last_seen_at > now() - interval '60 seconds'",
            )
            .bind(pool_id)
            .fetch_all(pool)
            .await?
        }
        ExecutorTarget::PersonalDevice { device_id } => {
            sqlx::query(
                "SELECT * FROM executors WHERE kind = 'personal_device' AND id = $1 AND NOT draining AND last_seen_at > now() - interval '60 seconds'",
            )
            .bind(device_id)
            .fetch_all(pool)
            .await?
        }
    };
    for row in rows {
        let executor = row_to_executor(&row)?;
        if has_capabilities(&executor.registration, required) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub async fn claim_external_run(
    pool: &PgPool,
    executor_id: Uuid,
    lease_seconds: i64,
) -> Result<Option<RunLease>, ApiError> {
    let mut tx = pool.begin().await?;
    let executor_row = sqlx::query("SELECT * FROM executors WHERE id = $1 FOR UPDATE")
        .bind(executor_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("executor {executor_id} was not found")))?;
    let executor = row_to_executor(&executor_row)?;
    if executor.draining || executor.active_runs >= executor.registration.max_concurrent_runs {
        tx.commit().await?;
        return Ok(None);
    }

    let candidates = sqlx::query(
        r#"
        SELECT r.* FROM runs r
        WHERE r.state IN ('queued', 'waiting_for_executor')
          AND (
            (r.target_kind = 'managed_windows_pool' AND $2 = 'managed_windows' AND r.target_pool_id = $3)
            OR (r.target_kind = 'personal_device' AND $2 = 'personal_device' AND r.target_device_id = $1)
          )
          AND NOT EXISTS (
            SELECT 1 FROM runs prior
            WHERE prior.thread_id = r.thread_id
              AND (prior.created_at, prior.id) < (r.created_at, r.id)
              AND prior.state <> ALL($4)
          )
        ORDER BY r.created_at ASC, r.id ASC
        LIMIT 25
        FOR UPDATE SKIP LOCKED
        "#,
    )
    .bind(executor_id)
    .bind(executor_kind_name(executor.registration.kind))
    .bind(executor.registration.pool_id)
    .bind(TERMINAL_STATES)
    .fetch_all(&mut *tx)
    .await?;

    let selected = candidates
        .into_iter()
        .find_map(|row| match row_to_run(&row) {
            Ok(run)
                if has_capabilities(&executor.registration, &run.spec.required_capabilities) =>
            {
                Some(Ok(run))
            }
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .transpose()?;
    let Some(mut run) = selected else {
        tx.commit().await?;
        return Ok(None);
    };

    let lease_token = Uuid::new_v4();
    let lease_expires_at = Utc::now() + chrono::Duration::seconds(lease_seconds);
    let row = sqlx::query(
        r#"
        UPDATE runs
        SET state = 'running', revision = revision + 1, assigned_executor_id = $2,
            lease_owner = $2, lease_token = $3, lease_expires_at = $4,
            started_at = COALESCE(started_at, now()), updated_at = now()
        WHERE id = $1
        RETURNING *
        "#,
    )
    .bind(run.spec.id)
    .bind(executor_id)
    .bind(lease_token)
    .bind(lease_expires_at)
    .fetch_one(&mut *tx)
    .await?;
    run = row_to_run(&row)?;
    sqlx::query(
        "UPDATE executors SET active_runs = active_runs + 1, last_seen_at = now() WHERE id = $1",
    )
    .bind(executor_id)
    .execute(&mut *tx)
    .await?;
    append_event_tx(
        &mut tx,
        run.spec.id,
        RunEventKind::ExecutorAssigned,
        json!({"executor_id": executor_id, "lease_expires_at": lease_expires_at}),
    )
    .await?;
    append_event_tx(
        &mut tx,
        run.spec.id,
        RunEventKind::StateChanged,
        json!({"from": "queued", "to": "running"}),
    )
    .await?;
    tx.commit().await?;

    Ok(Some(RunLease {
        schema_version: SCHEMA_VERSION,
        run,
        lease_token,
        lease_expires_at,
    }))
}

pub async fn recover_external_run(
    pool: &PgPool,
    executor_id: Uuid,
    lease_seconds: i64,
) -> Result<Option<RunLease>, ApiError> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        r#"
        SELECT * FROM runs
        WHERE assigned_executor_id = $1 AND lease_owner = $1
          AND lease_token IS NOT NULL AND lease_expires_at > now()
          AND state IN ('running', 'waiting_approval', 'waiting_input')
        ORDER BY updated_at DESC LIMIT 1 FOR UPDATE
        "#,
    )
    .bind(executor_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    let lease_token: Uuid = row
        .try_get::<Option<Uuid>, _>("lease_token")?
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("recoverable run has no lease token")))?;
    let lease_expires_at = Utc::now() + chrono::Duration::seconds(lease_seconds);
    let row = sqlx::query(
        "UPDATE runs SET lease_expires_at = $2, updated_at = now() WHERE id = $1 RETURNING *",
    )
    .bind(row.try_get::<Uuid, _>("id")?)
    .bind(lease_expires_at)
    .fetch_one(&mut *tx)
    .await?;
    let run = row_to_run(&row)?;
    append_event_tx(
        &mut tx,
        run.spec.id,
        RunEventKind::ExecutorHeartbeat,
        json!({"executor_id": executor_id, "lease_recovered": true, "lease_expires_at": lease_expires_at}),
    )
    .await?;
    tx.commit().await?;
    Ok(Some(RunLease {
        schema_version: SCHEMA_VERSION,
        run,
        lease_token,
        lease_expires_at,
    }))
}

pub async fn renew_lease(
    pool: &PgPool,
    run_id: Uuid,
    executor_id: Uuid,
    lease_token: Uuid,
    lease_seconds: i64,
) -> Result<RunRecord, ApiError> {
    let row = sqlx::query(
        r#"
        UPDATE runs SET lease_expires_at = now() + make_interval(secs => $4), updated_at = now()
        WHERE id = $1 AND lease_owner = $2 AND lease_token = $3
          AND state IN ('running', 'waiting_approval', 'waiting_input')
        RETURNING *
        "#,
    )
    .bind(run_id)
    .bind(executor_id)
    .bind(lease_token)
    .bind(lease_seconds as f64)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::Conflict("the run lease is no longer valid".to_owned()))?;
    row_to_run(&row)
}

pub async fn append_leased_event(
    pool: &PgPool,
    run_id: Uuid,
    executor_id: Uuid,
    lease_token: Uuid,
    source_event_id: Option<Uuid>,
    kind: RunEventKind,
    payload: Value,
) -> Result<RunEvent, ApiError> {
    let mut tx = pool.begin().await?;
    verify_lease(&mut tx, run_id, executor_id, lease_token).await?;
    if let Some(source_event_id) = source_event_id {
        if let Some(row) = sqlx::query(
            "SELECT run_id, sequence, event_id, kind, payload, created_at FROM run_events WHERE event_id = $1",
        )
        .bind(source_event_id)
        .fetch_optional(&mut *tx)
        .await?
        {
            let existing = row_to_event(&row)?;
            if existing.run_id != run_id || existing.kind != kind || existing.payload != payload {
                return Err(ApiError::Conflict(
                    "source_event_id was already used for different event content".to_owned(),
                ));
            }
            tx.commit().await?;
            return Ok(existing);
        }
    }
    let event = append_event_tx_with_id(
        &mut tx,
        run_id,
        source_event_id.unwrap_or_else(Uuid::new_v4),
        kind,
        payload,
    )
    .await?;
    tx.commit().await?;
    Ok(event)
}

pub async fn complete_leased_run(
    pool: &PgPool,
    run_id: Uuid,
    executor_id: Uuid,
    lease_token: Uuid,
    result: Value,
    result_snapshot_manifest_id: Option<Uuid>,
    result_diff_summary: Value,
) -> Result<RunRecord, ApiError> {
    finish_leased_run(
        pool,
        run_id,
        executor_id,
        lease_token,
        LeasedRunOutcome {
            state: RunState::Completed,
            result: Some(result),
            error: None,
            result_snapshot_manifest_id,
            result_diff_summary,
        },
    )
    .await
}

pub async fn fail_leased_run(
    pool: &PgPool,
    run_id: Uuid,
    executor_id: Uuid,
    lease_token: Uuid,
    error: RunError,
) -> Result<RunRecord, ApiError> {
    finish_leased_run(
        pool,
        run_id,
        executor_id,
        lease_token,
        LeasedRunOutcome {
            state: RunState::Failed,
            result: None,
            error: Some(error),
            result_snapshot_manifest_id: None,
            result_diff_summary: Value::Null,
        },
    )
    .await
}

pub async fn interrupt_leased_run(
    pool: &PgPool,
    run_id: Uuid,
    executor_id: Uuid,
    lease_token: Uuid,
    error: RunError,
) -> Result<RunRecord, ApiError> {
    finish_leased_run(
        pool,
        run_id,
        executor_id,
        lease_token,
        LeasedRunOutcome {
            state: RunState::Interrupted,
            result: None,
            error: Some(error),
            result_snapshot_manifest_id: None,
            result_diff_summary: Value::Null,
        },
    )
    .await
}

struct LeasedRunOutcome {
    state: RunState,
    result: Option<Value>,
    error: Option<RunError>,
    result_snapshot_manifest_id: Option<Uuid>,
    result_diff_summary: Value,
}

async fn finish_leased_run(
    pool: &PgPool,
    run_id: Uuid,
    executor_id: Uuid,
    lease_token: Uuid,
    outcome: LeasedRunOutcome,
) -> Result<RunRecord, ApiError> {
    let LeasedRunOutcome {
        state,
        mut result,
        error,
        result_snapshot_manifest_id,
        result_diff_summary,
    } = outcome;
    let mut tx = pool.begin().await?;
    let current = verify_lease(&mut tx, run_id, executor_id, lease_token).await?;
    ensure_run_transition(current.state, state)
        .map_err(|error| ApiError::Conflict(error.to_string()))?;
    if let Some(manifest_id) = result_snapshot_manifest_id {
        if state != RunState::Completed {
            return Err(ApiError::Unprocessable(
                "only completed runs may publish a result snapshot".to_owned(),
            ));
        }
        let version_id =
            create_run_result_version_tx(&mut tx, &current, manifest_id, result_diff_summary)
                .await?;
        let result_value = result.get_or_insert_with(|| json!({}));
        if !result_value.is_object() {
            *result_value = json!({"value": result_value.take()});
        }
        let object = result_value
            .as_object_mut()
            .expect("result was normalized to an object");
        object.insert("project_version_id".to_owned(), json!(version_id));
        object.insert("result_snapshot_manifest_id".to_owned(), json!(manifest_id));
    }
    let row = sqlx::query(
        r#"
        UPDATE runs SET state = $2, revision = revision + 1, result = $3, error = $4,
            lease_owner = NULL, lease_token = NULL, lease_expires_at = NULL,
            finished_at = now(), updated_at = now()
        WHERE id = $1 RETURNING *
        "#,
    )
    .bind(run_id)
    .bind(state_name(state))
    .bind(result.clone())
    .bind(error.as_ref().map(serde_json::to_value).transpose()?)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("UPDATE executors SET active_runs = GREATEST(active_runs - 1, 0), last_seen_at = now() WHERE id = $1")
        .bind(executor_id)
        .execute(&mut *tx)
        .await?;
    append_event_tx(
        &mut tx,
        run_id,
        if state == RunState::Completed {
            RunEventKind::Completed
        } else {
            RunEventKind::Failed
        },
        result
            .clone()
            .or_else(|| error.and_then(|value| serde_json::to_value(value).ok()))
            .unwrap_or(Value::Null),
    )
    .await?;
    if state == RunState::Completed {
        append_assistant_message_tx(&mut tx, run_id, result.unwrap_or(Value::Null)).await?;
    }
    let record = row_to_run(&row)?;
    tx.commit().await?;
    Ok(record)
}

async fn append_assistant_message_tx(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    content: Value,
) -> Result<(), ApiError> {
    let message_id = Uuid::new_v4();
    let etag = format!("W/\"{message_id}:1\"");
    let inserted = sqlx::query(
        r#"
        INSERT INTO messages (
            id, etag, thread_id, author_user_id, role, content, run_id
        )
        SELECT $1, $2, user_message.thread_id, NULL, 'assistant', $3, $4
        FROM messages user_message
        WHERE user_message.run_id = $4
          AND user_message.role = 'user'
          AND user_message.deleted_at IS NULL
        ON CONFLICT DO NOTHING
        RETURNING thread_id
        "#,
    )
    .bind(message_id)
    .bind(etag)
    .bind(content)
    .bind(run_id)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(row) = inserted {
        touch_thread_tx(tx, row.try_get("thread_id")?).await?;
        sync::publish_canonical_message_tx(tx, message_id).await?;
    }
    Ok(())
}

async fn touch_thread_tx(
    tx: &mut Transaction<'_, Postgres>,
    thread_id: Uuid,
) -> Result<(), ApiError> {
    let updated = sqlx::query(
        r#"
        UPDATE threads
        SET revision = revision + 1,
            etag = 'W/"' || id::text || ':' || (revision + 1)::text || '"',
            updated_at = now()
        WHERE id = $1 AND deleted_at IS NULL
        "#,
    )
    .bind(thread_id)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(ApiError::NotFound(format!(
            "thread {thread_id} was not found"
        )));
    }
    Ok(())
}

async fn create_run_result_version_tx(
    tx: &mut Transaction<'_, Postgres>,
    run: &RunRecord,
    manifest_id: Uuid,
    diff_summary: Value,
) -> Result<Uuid, ApiError> {
    let manifest_valid = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM snapshot_manifests
            WHERE id = $1 AND source_run_id = $2 AND project_id = $3
              AND created_by = $4 AND status = 'ready'
              AND (expires_at IS NULL OR expires_at > now())
        )
        "#,
    )
    .bind(manifest_id)
    .bind(run.spec.id)
    .bind(run.spec.project_id)
    .bind(run.spec.creator_user_id)
    .fetch_one(&mut **tx)
    .await?;
    if !manifest_valid {
        return Err(ApiError::Unprocessable(
            "result_snapshot_manifest_id must be a ready snapshot produced by this leased run"
                .to_owned(),
        ));
    }
    sqlx::query("SELECT id FROM projects WHERE id = $1 AND deleted_at IS NULL FOR UPDATE")
        .bind(run.spec.project_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!("project {} was not found", run.spec.project_id))
        })?;
    let base_version_id = match run.spec.snapshot_id {
        Some(snapshot_id) => {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM project_versions WHERE project_id = $1 AND snapshot_manifest_id = $2 ORDER BY revision DESC LIMIT 1",
            )
            .bind(run.spec.project_id)
            .bind(snapshot_id)
            .fetch_optional(&mut **tx)
            .await?
        }
        None => None,
    };
    let revision = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(revision), 0) + 1 FROM project_versions WHERE project_id = $1",
    )
    .bind(run.spec.project_id)
    .fetch_one(&mut **tx)
    .await?;
    let version_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO project_versions (
            id, project_id, revision, parent_version_id, merge_base_version_id,
            snapshot_manifest_id, created_by_user_id, created_by_run_id,
            diff_summary
        ) VALUES ($1, $2, $3, $4, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(version_id)
    .bind(run.spec.project_id)
    .bind(revision)
    .bind(base_version_id)
    .bind(manifest_id)
    .bind(run.spec.creator_user_id)
    .bind(run.spec.id)
    .bind(diff_summary)
    .execute(&mut **tx)
    .await?;
    Ok(version_id)
}

pub async fn transition_run(
    pool: &PgPool,
    run_id: Uuid,
    next: RunState,
    result: Option<Value>,
    error: Option<RunError>,
) -> Result<RunRecord, ApiError> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query("SELECT * FROM runs WHERE id = $1 FOR UPDATE")
        .bind(run_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("run {run_id} was not found")))?;
    let current = row_to_run(&row)?;
    ensure_run_transition(current.state, next)
        .map_err(|error| ApiError::Conflict(error.to_string()))?;
    if next.is_terminal() && !current.state.is_terminal() {
        if let Some(executor_id) = current.assigned_executor_id {
            sqlx::query(
                "UPDATE executors SET active_runs = GREATEST(active_runs - 1, 0), last_seen_at = now() WHERE id = $1",
            )
            .bind(executor_id)
            .execute(&mut *tx)
            .await?;
        }
    }
    let row = sqlx::query(
        r#"
        UPDATE runs SET state = $2, revision = revision + 1, result = COALESCE($3, result),
            error = COALESCE($4, error), updated_at = now(),
            finished_at = CASE WHEN $2 = ANY($5) THEN now() ELSE finished_at END,
            lease_owner = CASE WHEN $2 = ANY($5) THEN NULL ELSE lease_owner END,
            lease_token = CASE WHEN $2 = ANY($5) THEN NULL ELSE lease_token END,
            lease_expires_at = CASE WHEN $2 = ANY($5) THEN NULL ELSE lease_expires_at END
        WHERE id = $1 RETURNING *
        "#,
    )
    .bind(run_id)
    .bind(state_name(next))
    .bind(result)
    .bind(error.as_ref().map(serde_json::to_value).transpose()?)
    .bind(TERMINAL_STATES)
    .fetch_one(&mut *tx)
    .await?;
    append_event_tx(
        &mut tx,
        run_id,
        RunEventKind::StateChanged,
        json!({"from": state_name(current.state), "to": state_name(next)}),
    )
    .await?;
    let record = row_to_run(&row)?;
    tx.commit().await?;
    Ok(record)
}

pub async fn claim_server_run(
    pool: &PgPool,
    worker_id: Uuid,
    lease_seconds: i64,
) -> Result<Option<RunLease>, ApiError> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        r#"
        SELECT r.* FROM runs r
        WHERE r.state = 'queued' AND r.target_kind = 'server_linux'
          AND NOT EXISTS (
            SELECT 1 FROM runs prior
            WHERE prior.thread_id = r.thread_id
              AND (prior.created_at, prior.id) < (r.created_at, r.id)
              AND prior.state <> ALL($1)
          )
        ORDER BY r.created_at ASC, r.id ASC
        LIMIT 1
        FOR UPDATE SKIP LOCKED
        "#,
    )
    .bind(TERMINAL_STATES)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    let current = row_to_run(&row)?;
    let lease_token = Uuid::new_v4();
    let lease_expires_at = Utc::now() + chrono::Duration::seconds(lease_seconds);
    let row = sqlx::query(
        r#"
        UPDATE runs SET state = 'running', revision = revision + 1,
            assigned_executor_id = $2, lease_owner = $2, lease_token = $3,
            lease_expires_at = $4, started_at = COALESCE(started_at, now()), updated_at = now()
        WHERE id = $1 RETURNING *
        "#,
    )
    .bind(current.spec.id)
    .bind(worker_id)
    .bind(lease_token)
    .bind(lease_expires_at)
    .fetch_one(&mut *tx)
    .await?;
    append_event_tx(
        &mut tx,
        current.spec.id,
        RunEventKind::ExecutorAssigned,
        json!({"executor_id": worker_id, "lease_expires_at": lease_expires_at}),
    )
    .await?;
    append_event_tx(
        &mut tx,
        current.spec.id,
        RunEventKind::StateChanged,
        json!({"from": state_name(current.state), "to": "running"}),
    )
    .await?;
    let run = row_to_run(&row)?;
    tx.commit().await?;
    Ok(Some(RunLease {
        schema_version: SCHEMA_VERSION,
        run,
        lease_token,
        lease_expires_at,
    }))
}

pub async fn interrupt_expired_leases(pool: &PgPool) -> Result<usize, ApiError> {
    let mut tx = pool.begin().await?;
    let rows = sqlx::query(
        r#"
        WITH expired AS (
            SELECT r.id, r.state AS previous_state, r.assigned_executor_id,
                COALESCE((
                    SELECT checkpoint.safe_to_resume
                    FROM run_checkpoints checkpoint
                    WHERE checkpoint.run_id = r.id
                    ORDER BY checkpoint.sequence DESC
                    LIMIT 1
                ), TRUE) AS safe_to_resume
            FROM runs r
            WHERE r.state IN ('running', 'waiting_approval', 'waiting_input')
              AND r.lease_expires_at < now()
            FOR UPDATE
        )
        UPDATE runs AS run SET state = 'interrupted', revision = run.revision + 1,
            lease_owner = NULL, lease_token = NULL, lease_expires_at = NULL, updated_at = now(),
            error = jsonb_build_object(
                'code', CASE WHEN expired.safe_to_resume
                    THEN 'executor_lease_expired' ELSE 'unsafe_tool_interrupted' END,
                'message', CASE WHEN expired.safe_to_resume
                    THEN 'The executor stopped heartbeating at a safe checkpoint.'
                    ELSE 'The executor stopped heartbeating during an unsafe action. The action was not retried.' END,
                'retryable', false,
                'details', jsonb_build_object(
                    'safe_to_resume', expired.safe_to_resume,
                    'manual_review_required', NOT expired.safe_to_resume
                )
            )
        FROM expired
        WHERE run.id = expired.id
        RETURNING run.id, expired.assigned_executor_id, expired.previous_state,
            expired.safe_to_resume
        "#,
    )
    .fetch_all(&mut *tx)
    .await?;
    for row in &rows {
        let run_id: Uuid = row.try_get("id")?;
        let executor_id: Option<Uuid> = row.try_get("assigned_executor_id")?;
        let previous_state: String = row.try_get("previous_state")?;
        let safe_to_resume: bool = row.try_get("safe_to_resume")?;
        if let Some(executor_id) = executor_id {
            sqlx::query(
                "UPDATE executors SET active_runs = GREATEST(active_runs - 1, 0) WHERE id = $1",
            )
            .bind(executor_id)
            .execute(&mut *tx)
            .await?;
        }
        append_event_tx(
            &mut tx,
            run_id,
            RunEventKind::StateChanged,
            json!({
                "from": previous_state,
                "to": "interrupted",
                "reason": if safe_to_resume { "executor_lease_expired" } else { "unsafe_tool_interrupted" },
                "safe_to_resume": safe_to_resume,
                "manual_review_required": !safe_to_resume,
            }),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(rows.len())
}

pub(crate) async fn verify_lease(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    executor_id: Uuid,
    lease_token: Uuid,
) -> Result<RunRecord, ApiError> {
    let row = sqlx::query(
        "SELECT * FROM runs WHERE id = $1 AND lease_owner = $2 AND lease_token = $3 AND state IN ('running', 'waiting_approval', 'waiting_input') AND lease_expires_at > now() FOR UPDATE",
    )
    .bind(run_id)
    .bind(executor_id)
    .bind(lease_token)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::Conflict("the run lease is no longer valid".to_owned()))?;
    row_to_run(&row)
}

pub(crate) async fn append_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    kind: RunEventKind,
    payload: Value,
) -> Result<RunEvent, ApiError> {
    append_event_tx_with_id(tx, run_id, Uuid::new_v4(), kind, payload).await
}

pub(crate) async fn append_event_tx_with_id(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    event_id: Uuid,
    kind: RunEventKind,
    payload: Value,
) -> Result<RunEvent, ApiError> {
    let kind_name = event_kind_name(kind);
    sqlx::query("SELECT id FROM runs WHERE id = $1 FOR UPDATE")
        .bind(run_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("run {run_id} was not found")))?;
    if let Some(row) = sqlx::query(
        "SELECT run_id, sequence, event_id, kind, payload, created_at FROM run_events WHERE event_id = $1",
    )
    .bind(event_id)
    .fetch_optional(&mut **tx)
    .await?
    {
        let existing = row_to_event(&row)?;
        if existing.run_id != run_id || existing.kind != kind || existing.payload != payload {
            return Err(ApiError::Conflict(
                "event ID was already used for different event content".to_owned(),
            ));
        }
        return Ok(existing);
    }
    let row = sqlx::query(
        r#"
        WITH next_event AS (
            UPDATE runs
            SET next_event_sequence = next_event_sequence + 1
            WHERE id = $1
            RETURNING next_event_sequence - 1 AS sequence
        )
        INSERT INTO run_events (run_id, sequence, event_id, kind, payload, created_at)
        SELECT $1, next_event.sequence, $2, $3, $4, now()
        FROM next_event
        RETURNING run_id, sequence, event_id, kind, payload, created_at
        "#,
    )
    .bind(run_id)
    .bind(event_id)
    .bind(kind_name)
    .bind(payload)
    .fetch_one(&mut **tx)
    .await?;
    let event = row_to_event(&row)?;
    if should_push(kind) {
        sqlx::query(
            r#"
            INSERT INTO push_deliveries
                (id, run_id, user_id, event_sequence, event_kind)
            SELECT $1, run.id, run.creator_user_id, $2, $3
            FROM runs run
            WHERE run.id = $4
              AND EXISTS (
                  SELECT 1 FROM push_subscriptions subscription
                  WHERE subscription.user_id = run.creator_user_id
                    AND subscription.revoked_at IS NULL
              )
            ON CONFLICT (user_id, run_id, event_sequence) DO NOTHING
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(event.sequence)
        .bind(kind_name)
        .bind(run_id)
        .execute(&mut **tx)
        .await?;
    }
    Ok(event)
}

/// Removes event-log payloads after the documented 90-day window, but only
/// from terminal runs. `runs.next_event_sequence` makes later administrative
/// events monotonic even after every retained event row has been deleted.
pub async fn enforce_run_event_retention(
    pool: &PgPool,
    now: DateTime<Utc>,
    limit: i64,
) -> Result<u64, ApiError> {
    let result = sqlx::query(
        r#"
        WITH expired AS (
            SELECT event.run_id, event.sequence
            FROM run_events event
            JOIN runs run ON run.id = event.run_id
            WHERE event.created_at < $1 - interval '90 days'
              AND run.state IN ('completed', 'failed', 'canceled', 'expired')
              AND COALESCE(run.finished_at, run.updated_at) < $1 - interval '90 days'
            ORDER BY event.created_at, event.run_id, event.sequence
            LIMIT $2
            FOR UPDATE OF event SKIP LOCKED
        )
        DELETE FROM run_events event
        USING expired
        WHERE event.run_id = expired.run_id AND event.sequence = expired.sequence
        "#,
    )
    .bind(now)
    .bind(limit.clamp(1, 10_000))
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Keeps revoked and expired authentication material for a bounded support and
/// security-review window. Foreign-key cascades remove access tokens, refresh
/// history, and reauthentication grants with the selected sessions. A push
/// subscription is removed only when no other active session remains for that
/// user/device pair.
pub async fn enforce_auth_session_retention(
    pool: &PgPool,
    now: DateTime<Utc>,
    limit: i64,
) -> Result<u64, ApiError> {
    let mut tx = pool.begin().await?;
    let rows = sqlx::query(
        r#"
        SELECT id, user_id, device_id
        FROM auth_sessions
        WHERE expires_at < $1 - interval '90 days'
           OR revoked_at < $1 - interval '90 days'
        ORDER BY COALESCE(revoked_at, expires_at), id
        LIMIT $2
        FOR UPDATE SKIP LOCKED
        "#,
    )
    .bind(now)
    .bind(limit.clamp(1, 10_000))
    .fetch_all(&mut *tx)
    .await?;
    if rows.is_empty() {
        tx.commit().await?;
        return Ok(0);
    }
    let session_ids: Vec<Uuid> = rows.iter().map(|row| row.get("id")).collect();
    let user_ids: Vec<Uuid> = rows.iter().map(|row| row.get("user_id")).collect();
    let device_ids: Vec<Uuid> = rows.iter().map(|row| row.get("device_id")).collect();
    sqlx::query(
        r#"
        DELETE FROM push_subscriptions subscription
        WHERE EXISTS (
            SELECT 1
            FROM unnest($1::uuid[], $2::uuid[]) AS expired(user_id, device_id)
            WHERE expired.user_id = subscription.user_id
              AND expired.device_id = subscription.device_id
        )
          AND NOT EXISTS (
            SELECT 1 FROM auth_sessions active
            WHERE active.user_id = subscription.user_id
              AND active.device_id = subscription.device_id
              AND active.revoked_at IS NULL
              AND active.expires_at > $3
          )
        "#,
    )
    .bind(&user_ids)
    .bind(&device_ids)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    let result = sqlx::query("DELETE FROM auth_sessions WHERE id = ANY($1)")
        .bind(&session_ids)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(result.rows_affected())
}

fn should_push(kind: RunEventKind) -> bool {
    matches!(
        kind,
        RunEventKind::ApprovalRequested
            | RunEventKind::InputRequested
            | RunEventKind::ArtifactCreated
            | RunEventKind::Warning
            | RunEventKind::Failed
            | RunEventKind::Completed
    )
}

fn has_capabilities(registration: &ExecutorRegistration, required: &[Capability]) -> bool {
    let available: HashSet<&str> = registration
        .capabilities
        .iter()
        .map(|descriptor| descriptor.name.0.as_str())
        .collect();
    required
        .iter()
        .all(|item| available.contains(item.0.as_str()))
}

fn row_to_run(row: &PgRow) -> Result<RunRecord, ApiError> {
    let spec: RunSpec = serde_json::from_value(row.try_get("spec")?)?;
    let state = parse_state(row.try_get("state")?)?;
    let revision: i64 = row.try_get("revision")?;
    Ok(RunRecord {
        etag: format!("W/\"{}:{}\"", spec.id, revision),
        spec,
        state,
        revision,
        assigned_executor_id: row.try_get("assigned_executor_id")?,
        lease_expires_at: row.try_get("lease_expires_at")?,
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at")?,
        result: row.try_get("result")?,
        error: row
            .try_get::<Option<Value>, _>("error")?
            .map(serde_json::from_value)
            .transpose()?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_message(row: &PgRow) -> Result<MessageRecord, ApiError> {
    let role = match row.try_get::<&str, _>("role")? {
        "user" => MessageRole::User,
        "assistant" => MessageRole::Assistant,
        "system" => MessageRole::System,
        "tool" => MessageRole::Tool,
        other => {
            return Err(ApiError::Internal(anyhow::anyhow!(
                "unknown message role {other}"
            )))
        }
    };
    Ok(MessageRecord {
        schema_version: SCHEMA_VERSION,
        id: row.try_get("id")?,
        revision: row.try_get("revision")?,
        etag: row.try_get("etag")?,
        thread_id: row.try_get("thread_id")?,
        author_user_id: row.try_get("author_user_id")?,
        role,
        content: row.try_get("content")?,
        run_id: row.try_get("run_id")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        deleted_at: row.try_get("deleted_at")?,
    })
}

fn row_to_event(row: &PgRow) -> Result<RunEvent, ApiError> {
    Ok(RunEvent {
        schema_version: SCHEMA_VERSION,
        run_id: row.try_get("run_id")?,
        sequence: row.try_get("sequence")?,
        event_id: row.try_get("event_id")?,
        kind: parse_event_kind(row.try_get("kind")?)?,
        payload: row.try_get("payload")?,
        created_at: row.try_get("created_at")?,
    })
}

fn row_to_executor(row: &PgRow) -> Result<ExecutorRecord, ApiError> {
    let registration: ExecutorRegistration = serde_json::from_value(row.try_get("registration")?)?;
    let last_seen_at: DateTime<Utc> = row.try_get("last_seen_at")?;
    Ok(ExecutorRecord {
        registration,
        online: last_seen_at > Utc::now() - chrono::Duration::seconds(60),
        draining: row.try_get("draining")?,
        active_runs: u16::try_from(row.try_get::<i32, _>("active_runs")?)
            .map_err(|error| ApiError::Internal(error.into()))?,
        last_seen_at,
    })
}

fn target_columns(target: &ExecutorTarget) -> (&'static str, Option<Uuid>, Option<Uuid>) {
    match target {
        ExecutorTarget::ServerLinux { pool_id } => ("server_linux", *pool_id, None),
        ExecutorTarget::ManagedWindowsPool { pool_id } => {
            ("managed_windows_pool", Some(*pool_id), None)
        }
        ExecutorTarget::PersonalDevice { device_id } => ("personal_device", None, Some(*device_id)),
    }
}

pub fn state_name(state: RunState) -> &'static str {
    match state {
        RunState::Queued => "queued",
        RunState::WaitingForExecutor => "waiting_for_executor",
        RunState::WaitingForSnapshot => "waiting_for_snapshot",
        RunState::Running => "running",
        RunState::WaitingApproval => "waiting_approval",
        RunState::WaitingInput => "waiting_input",
        RunState::Interrupted => "interrupted",
        RunState::Completed => "completed",
        RunState::Failed => "failed",
        RunState::Canceled => "canceled",
        RunState::Expired => "expired",
    }
}

fn parse_state(value: &str) -> Result<RunState, ApiError> {
    match value {
        "queued" => Ok(RunState::Queued),
        "waiting_for_executor" => Ok(RunState::WaitingForExecutor),
        "waiting_for_snapshot" => Ok(RunState::WaitingForSnapshot),
        "running" => Ok(RunState::Running),
        "waiting_approval" => Ok(RunState::WaitingApproval),
        "waiting_input" => Ok(RunState::WaitingInput),
        "interrupted" => Ok(RunState::Interrupted),
        "completed" => Ok(RunState::Completed),
        "failed" => Ok(RunState::Failed),
        "canceled" => Ok(RunState::Canceled),
        "expired" => Ok(RunState::Expired),
        other => Err(ApiError::Internal(anyhow::anyhow!(
            "unknown run state {other}"
        ))),
    }
}

fn executor_kind_name(kind: ExecutorKind) -> &'static str {
    match kind {
        ExecutorKind::ServerLinux => "server_linux",
        ExecutorKind::ManagedWindows => "managed_windows",
        ExecutorKind::PersonalDevice => "personal_device",
    }
}

fn event_kind_name(kind: RunEventKind) -> &'static str {
    match kind {
        RunEventKind::Created => "created",
        RunEventKind::StateChanged => "state_changed",
        RunEventKind::ModelStarted => "model_started",
        RunEventKind::ModelDelta => "model_delta",
        RunEventKind::ModelCompleted => "model_completed",
        RunEventKind::ToolStarted => "tool_started",
        RunEventKind::ToolCompleted => "tool_completed",
        RunEventKind::ToolFailed => "tool_failed",
        RunEventKind::CheckpointCreated => "checkpoint_created",
        RunEventKind::ApprovalRequested => "approval_requested",
        RunEventKind::ApprovalResolved => "approval_resolved",
        RunEventKind::InputRequested => "input_requested",
        RunEventKind::InputReceived => "input_received",
        RunEventKind::ArtifactCreated => "artifact_created",
        RunEventKind::DesktopSessionChanged => "desktop_session_changed",
        RunEventKind::ExecutorAssigned => "executor_assigned",
        RunEventKind::ExecutorHeartbeat => "executor_heartbeat",
        RunEventKind::Warning => "warning",
        RunEventKind::Failed => "failed",
        RunEventKind::Completed => "completed",
    }
}

fn parse_event_kind(value: &str) -> Result<RunEventKind, ApiError> {
    serde_json::from_value(Value::String(value.to_owned())).map_err(ApiError::from)
}
