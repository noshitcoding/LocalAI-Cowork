use std::collections::HashSet;

use chrono::{DateTime, Utc};
use cowork_contracts::{
    ensure_run_transition, Capability, CreateThreadRequest, ExecutorKind, ExecutorRecord,
    ExecutorRegistration, ExecutorTarget, MessageRecord, MessageRole, RunError, RunEvent,
    RunEventKind, RunLease, RunRecord, RunSpec, RunState, ThreadMessageRunWithThread, ThreadRecord,
    SCHEMA_VERSION,
};
use serde_json::{json, Value};
use sqlx::{postgres::PgRow, PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{error::ApiError, governance, sync};

const TERMINAL_STATES: &[&str] = &["completed", "failed", "canceled", "expired"];

const CLAIM_LEASE_SQL: &str = r#"
    UPDATE runs SET state = 'running', revision = revision + 1,
        assigned_executor_id = $2, lease_owner = $2, lease_token = $3,
        lease_expires_at = now() + make_interval(secs => $4),
        started_at = COALESCE(started_at, now()), updated_at = now()
    WHERE id = $1 RETURNING *
"#;

const RECOVER_EXTERNAL_LEASE_SQL: &str = r#"
    UPDATE runs SET lease_expires_at = now() + make_interval(secs => $2), updated_at = now()
    WHERE id = $1 RETURNING *
"#;

const RENEW_LEASE_SQL: &str = r#"
    UPDATE runs SET lease_expires_at = now() + make_interval(secs => $4), updated_at = now()
    WHERE id = $1 AND lease_owner = $2 AND lease_token = $3
      AND state IN ('running', 'waiting_approval', 'waiting_input')
      AND lease_expires_at > now()
    RETURNING *
"#;

const INTERRUPT_SERVER_OWNED_LEASE_SQL: &str = r#"
    WITH owned AS (
        SELECT r.id, r.state AS previous_state,
            COALESCE((SELECT c.safe_to_resume FROM run_checkpoints c
                WHERE c.run_id = r.id ORDER BY c.sequence DESC LIMIT 1), TRUE) AS safe_to_resume
        FROM runs r
        WHERE r.id = $1 AND r.lease_owner = $2 AND r.lease_token = $3
          AND r.target_kind = 'server_linux'
          AND r.state IN ('running', 'waiting_approval', 'waiting_input')
          AND r.lease_expires_at > now()
        FOR UPDATE
    )
    UPDATE runs AS run SET state = 'interrupted', revision = run.revision + 1,
        lease_owner = NULL, lease_token = NULL, lease_expires_at = NULL, updated_at = now(),
        error = jsonb_build_object(
            'code', CASE WHEN owned.safe_to_resume THEN 'worker_shutdown' ELSE 'unsafe_tool_interrupted' END,
            'message', CASE WHEN owned.safe_to_resume
                THEN 'The server worker stopped at a safe checkpoint. The run was not retried.'
                ELSE 'The server worker stopped during or after an unsafe action. The action was not retried.' END,
            'retryable', false,
            'details', jsonb_build_object('safe_to_resume', owned.safe_to_resume,
                'manual_review_required', NOT owned.safe_to_resume)
        )
    FROM owned WHERE run.id = owned.id
    RETURNING run.*, owned.previous_state, owned.safe_to_resume
"#;

pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}

pub async fn create_run(
    pool: &PgPool,
    spec: &RunSpec,
    initial_state: RunState,
) -> Result<RunRecord, ApiError> {
    let mut tx = pool.begin().await?;
    crate::enterprise_grants::writer(&mut tx).await?;
    let (record, _) = create_run_tx(&mut tx, spec, initial_state).await?;
    tx.commit().await?;
    Ok(record)
}

pub(crate) async fn create_run_tx(
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
    let workspace_id = crate::chat_workspaces::assign(tx, spec).await?;

    let inserted = sqlx::query(
        r#"
        INSERT INTO runs (
            id, thread_id, project_id, creator_user_id, idempotency_key,
            target_kind, target_pool_id, target_device_id, state, spec,
            snapshot_id, created_at, updated_at, sandbox_workspace_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $12, $13)
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
    .bind(workspace_id)
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
    if inserted {
        crate::organisation_mfa::bind_run(tx, &record.spec).await?;
    }
    Ok((record, inserted))
}

pub async fn create_thread_message_run(
    pool: &PgPool,
    spec: &RunSpec,
    initial_state: RunState,
    content: Value,
) -> Result<(MessageRecord, RunRecord), ApiError> {
    let mut tx = pool.begin().await?;
    crate::enterprise_grants::writer(&mut tx).await?;
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

pub async fn create_thread_message_run_with_new_thread(
    pool: &PgPool,
    thread_request: &CreateThreadRequest,
    spec: &RunSpec,
    initial_state: RunState,
    content: Value,
) -> Result<ThreadMessageRunWithThread, ApiError> {
    let mut tx = pool.begin().await?;
    crate::enterprise_grants::writer(&mut tx).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "thread-run:{}:{}",
            spec.creator_user_id, spec.idempotency_key
        ))
        .execute(&mut *tx)
        .await?;

    if let Some(run_row) =
        sqlx::query("SELECT * FROM runs WHERE creator_user_id = $1 AND idempotency_key = $2")
            .bind(spec.creator_user_id)
            .bind(&spec.idempotency_key)
            .fetch_optional(&mut *tx)
            .await?
    {
        let run = row_to_run(&run_row)?;
        if run.spec.project_id != spec.project_id {
            return Err(ApiError::Conflict(
                "idempotency key belongs to a run in a different project".to_owned(),
            ));
        }
        let thread_row = sqlx::query("SELECT * FROM threads WHERE id = $1 AND deleted_at IS NULL")
            .bind(run.spec.thread_id)
            .fetch_one(&mut *tx)
            .await?;
        let thread = row_to_thread(&thread_row)?;
        if thread.title != thread_request.title {
            return Err(ApiError::Conflict(
                "idempotency key was reused with a different thread title".to_owned(),
            ));
        }
        let message_row = sqlx::query(
            "SELECT * FROM messages WHERE run_id = $1 AND role = 'user' AND deleted_at IS NULL",
        )
        .bind(run.spec.id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| {
            ApiError::Conflict(
                "idempotency key belongs to a run that has no initial message".to_owned(),
            )
        })?;
        if message_row.try_get::<Value, _>("content")? != content {
            return Err(ApiError::Conflict(
                "idempotency key was reused with different message content".to_owned(),
            ));
        }
        let message = row_to_message(&message_row)?;
        tx.commit().await?;
        return Ok(ThreadMessageRunWithThread {
            schema_version: SCHEMA_VERSION,
            thread,
            message,
            run,
        });
    }

    let thread = insert_thread_tx(
        &mut tx,
        spec.thread_id,
        spec.creator_user_id,
        thread_request,
    )
    .await?;
    let (run, inserted) = create_run_tx(&mut tx, spec, initial_state).await?;
    if !inserted {
        return Err(ApiError::Conflict(
            "idempotency key was concurrently used outside atomic thread creation".to_owned(),
        ));
    }
    let message_id = Uuid::new_v4();
    let message_row = sqlx::query(
        r#"
        INSERT INTO messages (
            id, etag, thread_id, author_user_id, role, content, run_id
        ) VALUES ($1, $2, $3, $4, 'user', $5, $6)
        RETURNING *
        "#,
    )
    .bind(message_id)
    .bind(format!("W/\"{message_id}:1\""))
    .bind(spec.thread_id)
    .bind(spec.creator_user_id)
    .bind(content)
    .bind(run.spec.id)
    .fetch_one(&mut *tx)
    .await?;
    touch_thread_tx(&mut tx, spec.thread_id).await?;
    sync::publish_canonical_message_tx(&mut tx, message_id).await?;
    let message = row_to_message(&message_row)?;
    tx.commit().await?;
    Ok(ThreadMessageRunWithThread {
        schema_version: SCHEMA_VERSION,
        thread,
        message,
        run,
    })
}

pub(crate) async fn insert_thread_tx(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    created_by: Uuid,
    request: &CreateThreadRequest,
) -> Result<ThreadRecord, ApiError> {
    let row = sqlx::query(
        r#"
        INSERT INTO threads (
            id, etag, project_id, created_by, forked_from_thread_id,
            forked_from_message_id, title
        ) VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(format!("W/\"{id}:1\""))
    .bind(request.project_id)
    .bind(created_by)
    .bind(request.forked_from_thread_id)
    .bind(request.forked_from_message_id)
    .bind(&request.title)
    .fetch_one(&mut **tx)
    .await?;
    sync::publish_canonical_thread_tx(tx, id).await?;
    row_to_thread(&row)
}

pub(crate) async fn get_thread_tx(
    tx: &mut Transaction<'_, Postgres>,
    thread_id: Uuid,
) -> Result<ThreadRecord, ApiError> {
    let row = sqlx::query("SELECT * FROM threads WHERE id = $1 AND deleted_at IS NULL")
        .bind(thread_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("thread {thread_id} was not found")))?;
    row_to_thread(&row)
}

pub async fn list_thread_messages(
    pool: &PgPool,
    thread_id: Uuid,
    limit: i64,
) -> Result<Vec<MessageRecord>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT * FROM (
            SELECT * FROM messages
            WHERE thread_id = $1 AND deleted_at IS NULL
            ORDER BY created_at DESC, id DESC
            LIMIT $2
        ) AS recent_messages
        ORDER BY created_at, id
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
    let access = crate::fleet_content::context(user_id, None).await?;
    let rows = sqlx::query(
        r#"
            SELECT DISTINCT run.* FROM runs run
            JOIN projects project ON project.id = run.project_id AND project.deleted_at IS NULL
            LEFT JOIN project_members pm ON pm.project_id = project.id AND pm.user_id = $1
            LEFT JOIN team_members tm ON tm.team_id = project.team_id AND tm.user_id = $1
            LEFT JOIN support_grants sg ON sg.support_user_id = $1
              AND sg.revoked_at IS NULL AND sg.expires_at > now()
              AND (sg.project_id = run.project_id OR sg.thread_id = run.thread_id)
            WHERE ($4 AND (run.creator_user_id = $1 OR project.owner_user_id = $1
               OR pm.user_id IS NOT NULL OR tm.user_id IS NOT NULL)) OR sg.id IS NOT NULL OR $3
            ORDER BY run.created_at DESC, run.id DESC LIMIT $2
            "#,
    )
    .bind(user_id)
    .bind(limit.clamp(1, 200))
    .bind(access.automatic_content_access)
    .bind(access.local_membership_allowed())
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
    let run = get_run(pool, run_id).await?;
    if matches!(run.spec.executor_target, ExecutorTarget::ServerLinux { .. }) {
        return Ok(begin_server_cancel(pool, run_id).await?.run);
    }
    transition_run(pool, run_id, RunState::Canceled, None, None).await
}

pub(crate) struct PendingServerCancel {
    pub run: RunRecord,
    pub cancel_id: Option<Uuid>,
    pub stop: Option<crate::runner_admission::StopRequest>,
}

async fn runner_attempts_tx(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    lease: Option<Uuid>,
) -> Result<Vec<crate::runner_admission::ExpectedAttempt>, ApiError> {
    let rows = sqlx::query("SELECT id,lease_token,request_sha256,workspace_id FROM run_runner_attempts WHERE run_id=$1 AND ($2::uuid IS NULL OR lease_token=$2) ORDER BY id")
        .bind(run_id).bind(lease).fetch_all(&mut **tx).await?;
    rows.iter()
        .map(|row| {
            Ok(crate::runner_admission::ExpectedAttempt {
                attempt_id: row.try_get("id")?,
                lease_token: row.try_get("lease_token")?,
                request_sha256: row.try_get("request_sha256")?,
                workspace_id: row.try_get("workspace_id")?,
            })
        })
        .collect()
}

/// Durable intent revokes execution authority before any remote stop call.
/// Interrupted keeps the thread workspace blocked until physical proof exists.
pub(crate) async fn begin_server_cancel(
    pool: &PgPool,
    run_id: Uuid,
) -> Result<PendingServerCancel, ApiError> {
    let mut tx = pool.begin().await?;
    crate::enterprise_grants::writer(&mut tx).await?;
    let row = sqlx::query("SELECT * FROM runs WHERE id=$1 FOR UPDATE")
        .bind(run_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("run {run_id} was not found")))?;
    let current = row_to_run(&row)?;
    if !matches!(
        current.spec.executor_target,
        ExecutorTarget::ServerLinux { .. }
    ) {
        return Err(ApiError::Conflict(
            "physical runner cancellation requires a server run".into(),
        ));
    }
    if current.state.is_terminal() {
        tx.commit().await?;
        return Ok(PendingServerCancel {
            run: current,
            cancel_id: None,
            stop: None,
        });
    }
    let attempts = runner_attempts_tx(&mut tx, run_id, None).await?;
    let desktop_exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM desktop_sessions WHERE run_id=$1 AND state IN ('starting','agent_controlled','user_controlled','paused'))")
        .bind(run_id).fetch_one(&mut *tx).await?;
    let needs_stop = current.started_at.is_some() || !attempts.is_empty() || desktop_exists;
    let next = if needs_stop {
        RunState::Interrupted
    } else {
        RunState::Canceled
    };
    let cancel_id = row
        .try_get::<Option<Uuid>, _>("runner_cancel_id")?
        .unwrap_or_else(Uuid::new_v4);
    let cancel_lease = row
        .try_get::<Option<Uuid>, _>("runner_cancel_lease")?
        .or(row.try_get("lease_token")?)
        .or(crate::fleet_execution::retained_cancel_lease(&mut tx, run_id).await?);
    if row
        .try_get::<Option<Uuid>, _>("runner_cancel_id")?
        .is_none()
    {
        if row.try_get::<Option<Uuid>, _>("lease_owner")?.is_some() {
            if let Some(executor) = current.assigned_executor_id {
                sqlx::query(
                    "UPDATE executors SET active_runs=GREATEST(active_runs-1,0) WHERE id=$1",
                )
                .bind(executor)
                .execute(&mut *tx)
                .await?;
            }
        }
        let error = if needs_stop {
            Some(
                json!({"code":"cancel_pending","message":"Cancellation requested; physical execution stop remains unconfirmed.","retryable":false,"details":{"manual_review_required":true}}),
            )
        } else {
            None
        };
        sqlx::query("UPDATE runs SET state=$2,revision=revision+1,error=$3,runner_cancel_id=$4,runner_cancel_lease=$5,lease_owner=NULL,lease_token=NULL,lease_expires_at=NULL,updated_at=clock_timestamp(),finished_at=CASE WHEN $2='canceled' THEN clock_timestamp() ELSE finished_at END WHERE id=$1")
            .bind(run_id).bind(state_name(next)).bind(error).bind(cancel_id).bind(cancel_lease).execute(&mut *tx).await?;
        append_event_tx(&mut tx, run_id, RunEventKind::StateChanged, json!({"from":state_name(current.state),"to":state_name(next),"reason":if needs_stop {"cancel_pending"} else {"canceled"}})).await?;
    }
    let row = sqlx::query("SELECT * FROM runs WHERE id=$1")
        .bind(run_id)
        .fetch_one(&mut *tx)
        .await?;
    let run = row_to_run(&row)?;
    tx.commit().await?;
    Ok(PendingServerCancel {
        run,
        cancel_id: needs_stop.then_some(cancel_id),
        stop: needs_stop.then_some(crate::runner_admission::StopRequest {
            version: 1,
            run_id,
            lease_token: None,
            cancel_lease,
            attempts,
        }),
    })
}

pub(crate) async fn prepare_runner_attempt(
    pool: &PgPool,
    run_id: Uuid,
    worker_id: Uuid,
    lease_token: Uuid,
    attempt_id: Uuid,
    workspace_id: Option<Uuid>,
    body: &[u8],
) -> Result<(), ApiError> {
    use sha2::{Digest, Sha256};
    let mut tx = pool.begin().await?;
    crate::enterprise_grants::writer(&mut tx).await?;
    fence_runner_request_tx(&mut tx, run_id, worker_id, lease_token).await?;
    sqlx::query("INSERT INTO run_runner_attempts(id,run_id,lease_owner,lease_token,workspace_id,request_sha256) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(attempt_id).bind(run_id).bind(worker_id).bind(lease_token).bind(workspace_id).bind(hex::encode(Sha256::digest(body))).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

/// Register the shared-core attempt before any agent effect. An interrupted
/// attempt is never inferred from checkpoints or external executor identities.
pub(crate) async fn begin_server_agent_execution(
    pool: &PgPool,
    run_id: Uuid,
    worker_id: Uuid,
    lease_token: Uuid,
) -> Result<Option<Value>, ApiError> {
    let mut tx = pool.begin().await?;
    crate::enterprise_grants::writer(&mut tx).await?;
    let run = verify_lease(&mut tx, run_id, worker_id, lease_token).await?;
    let row = sqlx::query("SELECT server_resume_checkpoint_id,runner_cancel_id FROM runs WHERE id=$1 AND lease_expires_at>clock_timestamp()")
        .bind(run_id).fetch_optional(&mut *tx).await?.ok_or_else(|| ApiError::Conflict("server execution lease expired".into()))?;
    if row
        .try_get::<Option<Uuid>, _>("runner_cancel_id")?
        .is_some()
        || !matches!(run.spec.executor_target, ExecutorTarget::ServerLinux { .. })
    {
        return Err(ApiError::Conflict(
            "server agent execution is fenced".into(),
        ));
    }
    let checkpoint_id: Option<Uuid> = row.try_get("server_resume_checkpoint_id")?;
    let checkpoint = if let Some(id) = checkpoint_id {
        let row = sqlx::query("SELECT id,origin,safe_to_resume,executor_state FROM run_checkpoints WHERE run_id=$1 ORDER BY sequence DESC LIMIT 1")
            .bind(run_id).fetch_one(&mut *tx).await?;
        if row.try_get::<Uuid, _>("id")? != id
            || row.try_get::<String, _>("origin")? != "server_agent_v1"
            || !row.try_get::<bool, _>("safe_to_resume")?
        {
            return Err(ApiError::Conflict(
                "selected resume checkpoint is no longer the latest trusted safe cursor".into(),
            ));
        }
        let state: Value = row.try_get("executor_state")?;
        cowork_runtime::checkpoint::validate_for_run(&run.spec, &state)
            .map_err(|_| ApiError::Conflict("selected agent checkpoint is invalid".into()))?;
        Some(state)
    } else {
        None
    };
    sqlx::query("INSERT INTO run_server_execution_attempts(id,run_id,worker_id,lease_token,resume_checkpoint_id) VALUES($1,$2,$3,$4,$5)")
        .bind(Uuid::new_v4()).bind(run_id).bind(worker_id).bind(lease_token).bind(checkpoint_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(checkpoint)
}

pub(crate) async fn runner_lease_stop_request(
    pool: &PgPool,
    run_id: Uuid,
    lease_token: Uuid,
) -> Result<crate::runner_admission::StopRequest, ApiError> {
    let mut tx = pool.begin().await?;
    crate::enterprise_grants::writer(&mut tx).await?;
    let attempts = runner_attempts_tx(&mut tx, run_id, Some(lease_token)).await?;
    tx.commit().await?;
    Ok(crate::runner_admission::StopRequest {
        version: 1,
        run_id,
        lease_token: Some(lease_token),
        cancel_lease: None,
        attempts,
    })
}

pub(crate) async fn finish_server_cancel(
    pool: &PgPool,
    run_id: Uuid,
    cancel_id: Uuid,
    proof: &crate::runner_admission::VerifiedStop,
) -> Result<RunRecord, ApiError> {
    let mut tx = pool.begin().await?;
    crate::enterprise_grants::writer(&mut tx).await?;
    let row = sqlx::query("SELECT * FROM runs WHERE id=$1 FOR UPDATE")
        .bind(run_id)
        .fetch_one(&mut *tx)
        .await?;
    let current = row_to_run(&row)?;
    if row.try_get::<Option<Uuid>, _>("runner_cancel_id")? != Some(cancel_id) {
        return Err(ApiError::Conflict("cancellation intent changed".into()));
    }
    let lease = row.try_get::<Option<Uuid>, _>("runner_cancel_lease")?;
    let attempts = runner_attempts_tx(&mut tx, run_id, None).await?;
    if !proof.matches(run_id, None, lease, &attempts) {
        return Err(ApiError::Conflict(
            "physical stop does not cover the exact execution identities".into(),
        ));
    }
    if current.state == RunState::Canceled {
        tx.commit().await?;
        return Ok(current);
    }
    if current.state != RunState::Interrupted {
        return Err(ApiError::Conflict("cancellation state changed".into()));
    }
    // The run-wide durable runner fence also prevents a delayed desktop create.
    // This stronger proof can safely resolve an earlier lost start response.
    let sessions = sqlx::query_scalar::<_,Uuid>("UPDATE desktop_sessions SET state='ended',controller_user_id=NULL,ended_at=clock_timestamp(),runner_metadata=COALESCE(runner_metadata,'{}'::jsonb)||'{\"cleanup_state\":\"confirmed\",\"start_pending\":false}'::jsonb WHERE run_id=$1 AND state IN ('starting','agent_controlled','user_controlled','paused') RETURNING id")
        .bind(run_id).fetch_all(&mut *tx).await?;
    for session in sessions {
        append_event_tx(
            &mut tx,
            run_id,
            RunEventKind::DesktopSessionChanged,
            json!({"session_id":session,"state":"ended","automatic":true}),
        )
        .await?;
    }
    if let Some(lease) = lease {
        crate::fleet_execution::completion_after_verified_stop(
            &mut tx, &current, lease, &attempts, proof,
        )
        .await?;
    }
    let row = sqlx::query("UPDATE runs SET state='canceled',revision=revision+1,error=NULL,runner_stop_receipt=$2,finished_at=clock_timestamp(),updated_at=clock_timestamp() WHERE id=$1 RETURNING *")
        .bind(run_id).bind(proof.receipt()).fetch_one(&mut *tx).await?;
    append_event_tx(
        &mut tx,
        run_id,
        RunEventKind::StateChanged,
        json!({"from":"interrupted","to":"canceled","execution_stopped":true}),
    )
    .await?;
    let run = row_to_run(&row)?;
    tx.commit().await?;
    Ok(run)
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
        SET last_seen_at = now(), active_runs = GREATEST($2, (
            SELECT COUNT(*)::integer FROM runs r WHERE r.assigned_executor_id=$1
              AND r.lease_token IS NOT NULL
              AND (r.state IN ('running','waiting_approval','waiting_input')
                   OR (r.state='interrupted' AND EXISTS(
                       SELECT 1 FROM enterprise_workflow_steps s WHERE s.child_run_id=r.id
                   )))
        )), updated_at = now()
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
    input: &Value,
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
        if supports_run(&executor.registration, required, input) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub async fn claim_external_run(
    pool: &PgPool,
    executor_id: Uuid,
    credential_id: Uuid,
    lease_seconds: i64,
) -> Result<Option<RunLease>, ApiError> {
    let mut tx = pool.begin().await?;
    crate::enterprise_grants::writer(&mut tx).await?;
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

    let lease_token = Uuid::new_v4();
    let mut selected = None;
    for row in candidates {
        let run = row_to_run(&row)?;
        if crate::enterprise_grants::execution_credential(&mut tx, run.spec.id, credential_id)
            .await
            .is_err()
        {
            continue;
        }
        if crate::organisation_mfa::execution(&mut tx, &run)
            .await
            .is_err()
        {
            continue;
        }
        if crate::enterprise_grants::execution(&mut tx, &run)
            .await
            .is_err()
        {
            continue;
        }
        if supports_run(
            &executor.registration,
            &run.spec.required_capabilities,
            &run.spec.input,
        ) && crate::fleet_execution::reserve(&mut tx, &run, lease_token)
            .await?
            .is_admitted()
        {
            selected = Some(run);
            break;
        }
    }
    let Some(mut run) = selected else {
        tx.commit().await?;
        return Ok(None);
    };

    crate::organisation_mfa::execution(&mut tx, &run).await?;
    let row = sqlx::query(CLAIM_LEASE_SQL)
        .bind(run.spec.id)
        .bind(executor_id)
        .bind(lease_token)
        .bind(lease_seconds as f64)
        .fetch_one(&mut *tx)
        .await?;
    let lease_expires_at: DateTime<Utc> = row.try_get("lease_expires_at")?;
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
    credential_id: Uuid,
    lease_seconds: i64,
) -> Result<Option<RunLease>, ApiError> {
    let mut tx = pool.begin().await?;
    crate::enterprise_grants::writer(&mut tx).await?;
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
    let run_id: Uuid = row.try_get("id")?;
    crate::enterprise_grants::execution_credential(&mut tx, run_id, credential_id).await?;
    let offline: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM enterprise_offline_permits WHERE run_id=$1)",
    )
    .bind(run_id)
    .fetch_one(&mut *tx)
    .await?;
    if offline {
        tx.commit().await?;
        return Ok(None);
    }
    let previous_state: String = row.try_get("state")?;
    let safe_to_resume = sqlx::query_scalar::<_, bool>(
        "SELECT safe_to_resume FROM run_checkpoints WHERE run_id = $1 ORDER BY sequence DESC LIMIT 1",
    )
    .bind(run_id)
    .fetch_optional(&mut *tx)
    .await?;
    if safe_to_resume == Some(false) {
        sqlx::query(
            r#"
            UPDATE runs SET state = 'interrupted', revision = revision + 1,
                lease_owner = NULL, lease_token = NULL, lease_expires_at = NULL,
                error = jsonb_build_object(
                    'code', 'unsafe_tool_interrupted',
                    'message', 'The executor disconnected during or after an unsafe action. The action was not retried.',
                    'retryable', false,
                    'details', jsonb_build_object(
                        'safe_to_resume', false,
                        'manual_review_required', true,
                        'detected_during_reconnect', true
                    )
                ), updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(run_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE executors SET active_runs = GREATEST(active_runs - 1, 0) WHERE id = $1",
        )
        .bind(executor_id)
        .execute(&mut *tx)
        .await?;
        append_event_tx(
            &mut tx,
            run_id,
            RunEventKind::StateChanged,
            json!({
                "from": previous_state,
                "to": "interrupted",
                "reason": "unsafe_tool_interrupted",
                "safe_to_resume": false,
                "manual_review_required": true,
                "detected_during_reconnect": true,
            }),
        )
        .await?;
        tx.commit().await?;
        return Ok(None);
    }
    let lease_token: Uuid = row
        .try_get::<Option<Uuid>, _>("lease_token")?
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("recoverable run has no lease token")))?;
    crate::organisation_mfa::execution(&mut tx, &row_to_run(&row)?).await?;
    crate::enterprise_grants::execution(&mut tx, &row_to_run(&row)?).await?;
    let row = sqlx::query(RECOVER_EXTERNAL_LEASE_SQL)
        .bind(run_id)
        .bind(lease_seconds as f64)
        .fetch_one(&mut *tx)
        .await?;
    let lease_expires_at: DateTime<Utc> = row.try_get("lease_expires_at")?;
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
    let mut tx = pool.begin().await?;
    crate::enterprise_grants::writer(&mut tx).await?;
    let current = get_run(pool, run_id).await?;
    crate::organisation_mfa::execution(&mut tx, &current).await?;
    crate::enterprise_grants::execution(&mut tx, &current).await?;
    let row = sqlx::query(RENEW_LEASE_SQL)
        .bind(run_id)
        .bind(executor_id)
        .bind(lease_token)
        .bind(lease_seconds as f64)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError::Conflict("the run lease is no longer valid".to_owned()))?;
    let record = row_to_run(&row)?;
    tx.commit().await?;
    Ok(record)
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
    append_leased_event_with_accounting(
        pool,
        LeasedEventInput {
            run_id,
            executor_id,
            lease_token,
            source_event_id,
            kind,
            payload,
        },
        false,
    )
    .await
}

pub async fn append_external_leased_event(
    pool: &PgPool,
    run_id: Uuid,
    executor_id: Uuid,
    lease_token: Uuid,
    source_event_id: Option<Uuid>,
    kind: RunEventKind,
    payload: Value,
) -> Result<RunEvent, ApiError> {
    append_leased_event_with_accounting(
        pool,
        LeasedEventInput {
            run_id,
            executor_id,
            lease_token,
            source_event_id,
            kind,
            payload,
        },
        true,
    )
    .await
}

struct LeasedEventInput {
    run_id: Uuid,
    executor_id: Uuid,
    lease_token: Uuid,
    source_event_id: Option<Uuid>,
    kind: RunEventKind,
    payload: Value,
}

async fn append_leased_event_with_accounting(
    pool: &PgPool,
    input: LeasedEventInput,
    account_external_model_usage: bool,
) -> Result<RunEvent, ApiError> {
    let LeasedEventInput {
        run_id,
        executor_id,
        lease_token,
        source_event_id,
        kind,
        payload,
    } = input;
    if account_external_model_usage
        && matches!(
            kind,
            RunEventKind::ModelStarted | RunEventKind::ModelCompleted
        )
        && source_event_id.is_none()
    {
        return Err(ApiError::Unprocessable(
            "external model events require a stable source_event_id".to_owned(),
        ));
    }
    let mut tx = pool.begin().await?;
    crate::enterprise_grants::writer(&mut tx).await?;
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
    let usage_tokens = if account_external_model_usage {
        external_model_usage_tokens(kind, &payload)?
    } else {
        None
    };
    if account_external_model_usage && kind == RunEventKind::ModelStarted {
        governance::ensure_model_quota_for_run_tx(&mut tx, run_id).await?;
    }
    let event = append_event_tx_with_id(
        &mut tx,
        run_id,
        source_event_id.unwrap_or_else(Uuid::new_v4),
        kind,
        payload,
    )
    .await?;
    if let Some(tokens) = usage_tokens {
        governance::record_model_usage_for_run_tx(&mut tx, run_id, tokens, 0).await?;
    }
    tx.commit().await?;
    Ok(event)
}

fn external_model_usage_tokens(
    kind: RunEventKind,
    payload: &Value,
) -> Result<Option<u64>, ApiError> {
    if kind != RunEventKind::ModelCompleted {
        return Ok(None);
    }
    let Some(usage) = payload.get("usage") else {
        return Ok(None);
    };
    if usage.is_null() {
        return Ok(None);
    }
    let usage = usage.as_object().ok_or_else(|| {
        ApiError::Unprocessable("external ModelCompleted usage must be an object".to_owned())
    })?;
    let field = |name: &str| -> Result<Option<u64>, ApiError> {
        usage
            .get(name)
            .map(|value| {
                value.as_u64().ok_or_else(|| {
                    ApiError::Unprocessable(format!(
                        "external ModelCompleted usage.{name} must be an unsigned integer"
                    ))
                })
            })
            .transpose()
    };
    let prompt_tokens = field("prompt_tokens")?;
    let completion_tokens = field("completion_tokens")?;
    let total_tokens = field("total_tokens")?;
    if prompt_tokens.is_none() && completion_tokens.is_none() && total_tokens.is_none() {
        return Ok(None);
    }
    Ok(Some(
        total_tokens.unwrap_or(0).max(
            prompt_tokens
                .unwrap_or(0)
                .saturating_add(completion_tokens.unwrap_or(0)),
        ),
    ))
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
            server_execution_stopped: ExecutionStopEvidence::Unconfirmed,
        },
    )
    .await
}

/// A server worker supplies this proof only after execution and desktop cleanup
/// have returned successfully. It is never accepted from an HTTP request body.
pub async fn complete_server_leased_run(
    pool: &PgPool,
    run_id: Uuid,
    worker_id: Uuid,
    lease_token: Uuid,
    result: Value,
    execution_stopped: bool,
) -> Result<RunRecord, ApiError> {
    finish_leased_run(
        pool,
        run_id,
        worker_id,
        lease_token,
        LeasedRunOutcome {
            state: RunState::Completed,
            result: Some(result),
            error: None,
            result_snapshot_manifest_id: None,
            result_diff_summary: Value::Null,
            server_execution_stopped: if execution_stopped { ExecutionStopEvidence::Returned } else { ExecutionStopEvidence::Unconfirmed },
        },
    )
    .await
}

pub(crate) const SERVER_DESKTOPS_STOPPED_SQL: &str = "SELECT NOT EXISTS(SELECT 1 FROM desktop_sessions WHERE run_id=$1 AND (state IN ('starting','agent_controlled','user_controlled','paused') OR runner_metadata->>'cleanup_state'='stop_pending' OR runner_metadata->>'start_pending'='true'))";

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
            server_execution_stopped: ExecutionStopEvidence::Unconfirmed,
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
            server_execution_stopped: ExecutionStopEvidence::Unconfirmed,
        },
    )
    .await
}

/// Interrupt only this worker's still-valid lease. Another worker's lease and
/// the expiry reaper retain ownership of their outcomes. This is not evidence
/// that remote execution stopped, so central reservations remain fenced.
pub async fn interrupt_server_owned_lease(
    pool: &PgPool,
    worker_id: Uuid,
    run_id: Uuid,
    lease_token: Uuid,
) -> Result<RunRecord, ApiError> {
    let mut tx = pool.begin().await?;
    crate::enterprise_grants::writer(&mut tx).await?;
    // Lock the run before reading checkpoints. A checkpoint committed by a
    // concurrent event writer must be visible before deciding safety.
    let current = verify_lease(&mut tx, run_id, worker_id, lease_token).await?;
    if !matches!(
        current.spec.executor_target,
        ExecutorTarget::ServerLinux { .. }
    ) {
        return Err(ApiError::Conflict(
            "only server-owned leases may be interrupted during worker shutdown".to_owned(),
        ));
    }
    let row = sqlx::query(INTERRUPT_SERVER_OWNED_LEASE_SQL)
        .bind(run_id)
        .bind(worker_id)
        .bind(lease_token)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| {
            ApiError::Conflict("the server worker no longer owns this run lease".to_owned())
        })?;
    let previous_state: String = row.try_get("previous_state")?;
    let safe_to_resume: bool = row.try_get("safe_to_resume")?;
    let record = row_to_run(&row)?;
    sqlx::query("UPDATE executors SET active_runs = GREATEST(active_runs - 1, 0) WHERE id = $1")
        .bind(worker_id)
        .execute(&mut *tx)
        .await?;
    append_event_tx(
        &mut tx,
        run_id,
        RunEventKind::StateChanged,
        json!({
            "from": previous_state,
            "to": "interrupted",
            "reason": if safe_to_resume { "worker_shutdown" } else { "unsafe_tool_interrupted" },
            "safe_to_resume": safe_to_resume,
            "manual_review_required": !safe_to_resume,
            "retryable": false,
        }),
    )
    .await?;
    tx.commit().await?;
    Ok(record)
}

enum ExecutionStopEvidence {
    Unconfirmed,
    Returned,
    DeniedBeforeDispatch,
}

// A dispatch denial is not a general physical-stop proof. Release only when
// this run never dispatched a tool/runner job and all model calls returned to
// a trusted safe cursor. Timeouts, unsafe cursors and external origins stay held.
const SERVER_DISPATCH_DENIAL_STOPPED_SQL: &str = r#"
    SELECT NOT EXISTS(SELECT 1 FROM run_runner_attempts WHERE run_id=$1)
      AND NOT EXISTS(SELECT 1 FROM run_events WHERE run_id=$1 AND kind='tool_started')
      AND (SELECT count(*) FROM run_events WHERE run_id=$1 AND kind='model_started')
        = (SELECT count(*) FROM run_events WHERE run_id=$1 AND kind='model_completed')
      AND (NOT EXISTS(SELECT 1 FROM run_events WHERE run_id=$1 AND kind='model_started')
        OR COALESCE((SELECT safe_to_resume AND origin='server_agent_v1' FROM run_checkpoints
            WHERE run_id=$1 ORDER BY sequence DESC LIMIT 1),FALSE))
"#;

/// Only the in-process worker's typed pre-dispatch denial may enter this path.
/// HTTP executor failures continue to provide no server stop evidence.
pub(crate) async fn fail_server_dispatch_denied(
    pool: &PgPool,
    run_id: Uuid,
    worker_id: Uuid,
    lease_token: Uuid,
    error: RunError,
) -> Result<RunRecord, ApiError> {
    finish_leased_run(pool,run_id,worker_id,lease_token,LeasedRunOutcome {
        state:RunState::Failed,result:None,error:Some(error),result_snapshot_manifest_id:None,
        result_diff_summary:Value::Null,server_execution_stopped:ExecutionStopEvidence::DeniedBeforeDispatch,
    }).await
}

struct LeasedRunOutcome {
    state: RunState,
    result: Option<Value>,
    error: Option<RunError>,
    result_snapshot_manifest_id: Option<Uuid>,
    result_diff_summary: Value,
    server_execution_stopped: ExecutionStopEvidence,
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
        server_execution_stopped,
    } = outcome;
    let mut tx = pool.begin().await?;
    crate::enterprise_grants::writer(&mut tx).await?;
    let current = verify_lease(&mut tx, run_id, executor_id, lease_token).await?;
    let server_execution_stopped = match server_execution_stopped {
        ExecutionStopEvidence::Unconfirmed => false,
        ExecutionStopEvidence::Returned => true,
        ExecutionStopEvidence::DeniedBeforeDispatch => {
            matches!(current.spec.executor_target,ExecutorTarget::ServerLinux { .. })
                && sqlx::query_scalar::<_,bool>(SERVER_DISPATCH_DENIAL_STOPPED_SQL)
                    .bind(run_id).fetch_one(&mut *tx).await?
        }
    };
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
    // Desktop cleanup commits before this transition. Recheck while the run
    // row is still locked: creation must take the same lock, and a pending
    // start/stop is never proof that server execution has stopped.
    let server_execution_stopped = if server_execution_stopped
        && matches!(
            current.spec.executor_target,
            ExecutorTarget::ServerLinux { .. }
        ) {
        sqlx::query_scalar::<_, bool>(SERVER_DESKTOPS_STOPPED_SQL)
            .bind(run_id)
            .fetch_one(&mut *tx)
            .await?
    } else {
        server_execution_stopped
    };
    crate::fleet_execution::completion(
        &mut tx,
        &current,
        lease_token,
        state,
        server_execution_stopped,
    )
    .await?;
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
    crate::enterprise_grants::writer(&mut tx).await?;
    let row = sqlx::query("SELECT * FROM runs WHERE id = $1 FOR UPDATE")
        .bind(run_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("run {run_id} was not found")))?;
    let current = row_to_run(&row)?;
    if row
        .try_get::<Option<Uuid>, _>("runner_cancel_id")?
        .is_some()
        && current.state == RunState::Interrupted
    {
        return Err(ApiError::Conflict(
            "physical cancellation remains unconfirmed".into(),
        ));
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunWaitingReason {
    PlatformCapacity,
    OrganisationCapacity,
    UserCapacity,
    InstanceCapacity,
    ChatWorkspaceBusy,
    AdmissionUnavailable,
    RunnerCapacity,
}

impl RunWaitingReason {
    fn name(self) -> &'static str {
        match self {
            Self::PlatformCapacity => "platform_capacity",
            Self::OrganisationCapacity => "organisation_capacity",
            Self::UserCapacity => "user_capacity",
            Self::InstanceCapacity => "instance_capacity",
            Self::ChatWorkspaceBusy => "chat_workspace_busy",
            Self::AdmissionUnavailable => "admission_unavailable",
            Self::RunnerCapacity => "runner_capacity",
        }
    }

    fn parse(value: &str) -> Result<Self, ApiError> {
        match value {
            "platform_capacity" => Ok(Self::PlatformCapacity),
            "organisation_capacity" => Ok(Self::OrganisationCapacity),
            "user_capacity" => Ok(Self::UserCapacity),
            "instance_capacity" => Ok(Self::InstanceCapacity),
            "chat_workspace_busy" => Ok(Self::ChatWorkspaceBusy),
            "admission_unavailable" => Ok(Self::AdmissionUnavailable),
            "runner_capacity" => Ok(Self::RunnerCapacity),
            _ => Err(ApiError::Internal(anyhow::anyhow!(
                "invalid stored waiting reason"
            ))),
        }
    }
}

impl From<cowork_contracts::enterprise_execution::ExecutionCapacityReason> for RunWaitingReason {
    fn from(reason: cowork_contracts::enterprise_execution::ExecutionCapacityReason) -> Self {
        use cowork_contracts::enterprise_execution::ExecutionCapacityReason;
        match reason {
            ExecutionCapacityReason::Platform => Self::PlatformCapacity,
            ExecutionCapacityReason::Organisation => Self::OrganisationCapacity,
            ExecutionCapacityReason::User => Self::UserCapacity,
            ExecutionCapacityReason::Instance => Self::InstanceCapacity,
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct RunWaitingDto {
    pub run_id: Uuid,
    pub reason: Option<RunWaitingReason>,
    pub checked_at: Option<DateTime<Utc>>,
}

/// Admission diagnostics are content-free, but reveal activity in a thread.
/// Apply the same viewer authorization as the run endpoint before reading them.
pub async fn get_run_waiting(
    pool: &PgPool,
    user_id: Uuid,
    run_id: Uuid,
) -> Result<RunWaitingDto, ApiError> {
    let scope = sqlx::query("SELECT project_id, thread_id FROM runs WHERE id = $1")
        .bind(run_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("run {run_id} was not found")))?;
    crate::organization::ensure_thread_role(
        pool,
        user_id,
        scope.try_get("project_id")?,
        scope.try_get("thread_id")?,
        cowork_contracts::ProjectRole::Viewer,
    )
    .await?;
    let row = sqlx::query(
        "SELECT waiting.reason, waiting.checked_at FROM run_waiting_reasons waiting \
         JOIN runs run ON run.id = waiting.run_id WHERE run.id = $1 \
         AND ((run.state IN ('queued', 'waiting_for_executor', 'waiting_for_snapshot') \
               AND waiting.reason <> 'runner_capacity') \
              OR (run.state = 'running' AND waiting.reason = 'runner_capacity' \
                  AND waiting.lease_owner = run.lease_owner AND waiting.lease_token = run.lease_token \
                  AND run.lease_expires_at > clock_timestamp()))",
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await?;
    Ok(match row {
        Some(row) => RunWaitingDto {
            run_id,
            reason: Some(RunWaitingReason::parse(row.try_get::<&str, _>("reason")?)?),
            checked_at: Some(row.try_get("checked_at")?),
        },
        None => RunWaitingDto {
            run_id,
            reason: None,
            checked_at: None,
        },
    })
}

async fn record_run_waiting(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    reason: RunWaitingReason,
) -> Result<(), ApiError> {
    // The caller holds the run lock, so terminal cleanup cannot race this write.
    sqlx::query(
        "INSERT INTO run_waiting_reasons (run_id, reason, checked_at) \
         SELECT id, $2, clock_timestamp() FROM runs WHERE id = $1 \
         AND state IN ('queued', 'waiting_for_executor', 'waiting_for_snapshot') \
         ON CONFLICT (run_id) DO UPDATE \
         SET reason = EXCLUDED.reason, checked_at = EXCLUDED.checked_at",
    )
    .bind(run_id)
    .bind(reason.name())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

// A capacity wait is owned by an executing lease, not a queue/requeue state.
const RUNNER_WAITING_FENCE_SQL: &str = "SELECT id FROM runs WHERE id=$1 AND lease_owner=$2 \
    AND lease_token=$3 AND state='running' AND target_kind='server_linux' \
    AND lease_expires_at > clock_timestamp() FOR UPDATE";

async fn fence_runner_request_tx(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    worker_id: Uuid,
    lease_token: Uuid,
) -> Result<(), ApiError> {
    sqlx::query_scalar::<_, Uuid>(RUNNER_WAITING_FENCE_SQL)
        .bind(run_id)
        .bind(worker_id)
        .bind(lease_token)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| {
            ApiError::Conflict("the runner request lease is no longer valid".to_owned())
        })?;
    Ok(())
}

/// Set an unsafe dispatch checkpoint and clear an old wait before every POST.
pub(crate) async fn prepare_runner_request(
    pool: &PgPool,
    run_id: Uuid,
    worker_id: Uuid,
    lease_token: Uuid,
    executor_state: Value,
) -> Result<(), ApiError> {
    update_runner_capacity_waiting(pool, run_id, worker_id, lease_token, false, executor_state)
        .await
}

/// Called only after the exact current request's signed NoStart proof succeeds.
/// Both the safe checkpoint and the lease-owned diagnostic commit together.
pub(crate) async fn mark_runner_capacity_waiting(
    pool: &PgPool,
    run_id: Uuid,
    worker_id: Uuid,
    lease_token: Uuid,
) -> Result<(), ApiError> {
    update_runner_capacity_waiting(
        pool,
        run_id,
        worker_id,
        lease_token,
        true,
        json!({"phase":"sandbox_waiting_capacity","reason":"runner_capacity"}),
    )
    .await
}

async fn update_runner_capacity_waiting(
    pool: &PgPool,
    run_id: Uuid,
    worker_id: Uuid,
    lease_token: Uuid,
    waiting: bool,
    executor_state: Value,
) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;
    crate::enterprise_grants::writer(&mut tx).await?;
    fence_runner_request_tx(&mut tx, run_id, worker_id, lease_token).await?;
    let checkpoint_id = Uuid::new_v4();
    let sequence = sqlx::query_scalar::<_, i64>(
        "INSERT INTO run_checkpoints (id,run_id,sequence,safe_to_resume,executor_state) \
         SELECT $1,$2,COALESCE(MAX(sequence),0)+1,$3,$4 FROM run_checkpoints WHERE run_id=$2 RETURNING sequence",
    ).bind(checkpoint_id).bind(run_id).bind(waiting).bind(executor_state)
        .fetch_one(&mut *tx).await?;
    append_event_tx(
        &mut tx,
        run_id,
        RunEventKind::CheckpointCreated,
        json!({"checkpoint_id":checkpoint_id,"sequence":sequence,"safe_to_resume":waiting}),
    )
    .await?;
    if waiting {
        sqlx::query("INSERT INTO run_waiting_reasons (run_id,reason,checked_at,lease_owner,lease_token) \
            VALUES ($1,'runner_capacity',clock_timestamp(),$2,$3) ON CONFLICT (run_id) DO UPDATE \
            SET reason=EXCLUDED.reason,checked_at=EXCLUDED.checked_at,lease_owner=EXCLUDED.lease_owner,lease_token=EXCLUDED.lease_token")
            .bind(run_id).bind(worker_id).bind(lease_token).execute(&mut *tx).await?;
    } else {
        sqlx::query("DELETE FROM run_waiting_reasons WHERE run_id=$1")
            .bind(run_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Recheck cancellation and ownership while waiting, without minting authority.
pub(crate) async fn check_runner_capacity_waiting_lease(
    pool: &PgPool,
    run_id: Uuid,
    worker_id: Uuid,
    lease_token: Uuid,
) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;
    crate::enterprise_grants::writer(&mut tx).await?;
    fence_runner_request_tx(&mut tx, run_id, worker_id, lease_token).await?;
    tx.commit().await?;
    Ok(())
}

const SERVER_CLAIM_CANDIDATES_SQL: &str = r#"
    SELECT r.* FROM runs r
    LEFT JOIN run_waiting_reasons waiting ON waiting.run_id = r.id
    WHERE r.state = 'queued' AND r.target_kind = 'server_linux'
      AND NOT EXISTS (
        SELECT 1 FROM runs prior
        WHERE prior.thread_id = r.thread_id
          AND (prior.created_at, prior.id) < (r.created_at, r.id)
          AND prior.state <> ALL($1)
      )
    ORDER BY COALESCE(waiting.checked_at, LEAST(r.created_at, now())) ASC,
        r.created_at ASC, r.id ASC
    LIMIT $2
    FOR UPDATE OF r SKIP LOCKED
"#;

pub async fn claim_server_run(
    pool: &PgPool,
    worker_id: Uuid,
    lease_seconds: i64,
) -> Result<Option<RunLease>, ApiError> {
    let mut tx = pool.begin().await?;
    crate::enterprise_grants::writer(&mut tx).await?;
    crate::chat_workspaces::use_contract(&mut tx).await?;
    let candidates = sqlx::query(SERVER_CLAIM_CANDIDATES_SQL)
        .bind(TERMINAL_STATES)
        // The bounded scan rotates by last check. A denial must not repeatedly
        // occupy the oldest 25 positions and starve later independent threads.
        .bind(if crate::fleet_execution::enabled() {
            25_i64
        } else {
            1_i64
        })
        .fetch_all(&mut *tx)
        .await?;
    let lease_token = Uuid::new_v4();
    let mut selected = None;
    for row in candidates {
        let run = row_to_run(&row)?;
        if row
            .try_get::<Option<Uuid>, _>("sandbox_workspace_id")?
            .is_some()
            && !crate::chat_workspaces::claimable(&mut tx, &run.spec).await?
        {
            record_run_waiting(&mut tx, run.spec.id, RunWaitingReason::ChatWorkspaceBusy).await?;
            continue;
        }
        if crate::organisation_mfa::execution(&mut tx, &run)
            .await
            .is_err()
        {
            record_run_waiting(&mut tx, run.spec.id, RunWaitingReason::AdmissionUnavailable)
                .await?;
            continue;
        }
        match crate::fleet_execution::reserve(&mut tx, &run, lease_token).await {
            Ok(crate::fleet_execution::AdmissionOutcome::Admitted) => {
                selected = Some(run);
                break;
            }
            Ok(crate::fleet_execution::AdmissionOutcome::Waiting(reason)) => {
                record_run_waiting(&mut tx, run.spec.id, reason.into()).await?;
            }
            Err(error) => {
                record_run_waiting(&mut tx, run.spec.id, RunWaitingReason::AdmissionUnavailable)
                    .await?;
                // No local lease is minted. Persist the content-free check,
                // even when the manager is missing or its response is invalid.
                tx.commit().await?;
                return Err(error);
            }
        }
    }
    let Some(current) = selected else {
        tx.commit().await?;
        return Ok(None);
    };
    crate::organisation_mfa::execution(&mut tx, &current).await?;
    let row = sqlx::query(CLAIM_LEASE_SQL)
        .bind(current.spec.id)
        .bind(worker_id)
        .bind(lease_token)
        .bind(lease_seconds as f64)
        .fetch_one(&mut *tx)
        .await?;
    let lease_expires_at: DateTime<Utc> = row.try_get("lease_expires_at")?;
    sqlx::query("DELETE FROM run_waiting_reasons WHERE run_id = $1")
        .bind(current.spec.id)
        .execute(&mut *tx)
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
    crate::enterprise_grants::writer(&mut tx).await?;
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
              AND NOT EXISTS (SELECT 1 FROM enterprise_offline_permits p WHERE p.run_id=r.id AND p.reconciled_at IS NULL)
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
    let run = row_to_run(&row)?;
    crate::enterprise_grants::execution(tx, &run).await?;
    Ok(run)
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
    crate::enterprise_grants::writer(&mut tx).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, user_id, device_id
        FROM auth_sessions
        WHERE (expires_at < $1 - interval '90 days'
           OR revoked_at < $1 - interval '90 days')
          AND NOT EXISTS(SELECT 1 FROM run_mfa_admissions a WHERE a.auth_session_id=auth_sessions.id)
          AND NOT EXISTS(SELECT 1 FROM run_mfa_continuations a WHERE a.auth_session_id=auth_sessions.id)
          AND NOT EXISTS(SELECT 1 FROM schedule_mfa_admissions a WHERE a.auth_session_id=auth_sessions.id)
          AND NOT EXISTS(SELECT 1 FROM terminal_stream_tickets a WHERE a.auth_session_id=auth_sessions.id)
          AND NOT EXISTS(SELECT 1 FROM terminal_sessions a WHERE a.auth_session_id=auth_sessions.id)
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

fn supports_run(
    registration: &ExecutorRegistration,
    required: &[Capability],
    input: &Value,
) -> bool {
    if !has_capabilities(registration, required) {
        return false;
    }
    if registration.kind != ExecutorKind::ManagedWindows {
        return true;
    }
    if input.get("task_runner").and_then(Value::as_str) == Some("crew")
        && !registration
            .capabilities
            .iter()
            .any(|descriptor| descriptor.name.0 == "crew.python")
    {
        return false;
    }
    let selected = input
        .get("frozen_runtime_context")
        .and_then(|context| context.get("mcp_metadata"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            item.get("definition")
                .and_then(|definition| definition.get("name"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
        })
        .collect::<HashSet<_>>();
    if selected.is_empty() {
        return true;
    }
    let configured = registration
        .capabilities
        .iter()
        .find(|descriptor| descriptor.name.0 == "tool.mcp.invoke")
        .and_then(|descriptor| descriptor.attributes.get("server_names"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<HashSet<_>>();
    selected.is_subset(&configured)
}

pub(crate) fn row_to_run(row: &PgRow) -> Result<RunRecord, ApiError> {
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

fn row_to_thread(row: &PgRow) -> Result<ThreadRecord, ApiError> {
    Ok(ThreadRecord {
        schema_version: SCHEMA_VERSION,
        id: row.try_get("id")?,
        revision: row.try_get("revision")?,
        etag: row.try_get("etag")?,
        project_id: row.try_get("project_id")?,
        forked_from_thread_id: row.try_get("forked_from_thread_id")?,
        forked_from_message_id: row.try_get("forked_from_message_id")?,
        title: row.try_get("title")?,
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

#[cfg(test)]
mod tests {
    use super::*;
    use cowork_contracts::CapabilityDescriptor;
    use std::collections::BTreeMap;

    async fn waiting_test_pool() -> PgPool {
        assert!(
            !crate::fleet::enabled(),
            "isolated waiting tests must not contact a configured management service"
        );
        let url = std::env::var("COWORK_TEST_DATABASE_URL").expect("isolated test database URL");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        let mut connection = pool.acquire().await.unwrap();
        // One connection and session-local tables isolate both the production
        // queue SQL and its real authorization helpers from application rows.
        sqlx::raw_sql(
            r#"
            CREATE TEMP TABLE runs (
                id uuid PRIMARY KEY, thread_id uuid, project_id uuid, creator_user_id uuid,
                state text NOT NULL DEFAULT 'queued', target_kind text NOT NULL DEFAULT 'server_linux',
                created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
                revision bigint NOT NULL DEFAULT 1, assigned_executor_id uuid,
                lease_owner uuid, lease_token uuid, lease_expires_at timestamptz,
                started_at timestamptz, updated_at timestamptz
            );
            CREATE TEMP TABLE projects (id uuid PRIMARY KEY, owner_user_id uuid, team_id uuid, deleted_at timestamptz);
            CREATE TEMP TABLE threads (id uuid PRIMARY KEY, project_id uuid, deleted_at timestamptz);
            CREATE TEMP TABLE project_members (project_id uuid, user_id uuid, role text);
            CREATE TEMP TABLE team_members (team_id uuid, user_id uuid, role text);
            CREATE TEMP TABLE users (id uuid PRIMARY KEY, platform_admin boolean NOT NULL DEFAULT false, deleted_at timestamptz);
        "#,
        )
        .execute(&mut *connection)
        .await
        .unwrap();
        let migration = include_str!("../migrations/0054_run_waiting_reasons.sql")
            .replace("CREATE TABLE", "CREATE TEMP TABLE")
            .replace(
                "clear_run_waiting_reason()",
                "pg_temp.clear_run_waiting_reason()",
            );
        sqlx::raw_sql(&migration)
            .execute(&mut *connection)
            .await
            .unwrap();
        drop(connection);
        pool
    }

    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to an isolated PostgreSQL test database"]
    async fn postgres_runner_cancel_keeps_workspace_and_reservations_until_exact_physical_proof() {
        let pool = waiting_test_pool().await;
        sqlx::raw_sql(r#"
            ALTER TABLE runs ADD COLUMN spec jsonb;
            ALTER TABLE runs ADD COLUMN finished_at timestamptz;
            ALTER TABLE runs ADD COLUMN result jsonb;
            ALTER TABLE runs ADD COLUMN error jsonb;
            ALTER TABLE runs ADD COLUMN next_event_sequence bigint NOT NULL DEFAULT 1;
            CREATE TEMP TABLE desktop_sessions(id uuid PRIMARY KEY,run_id uuid,state text,controller_user_id uuid,ended_at timestamptz,runner_metadata jsonb);
            CREATE TEMP TABLE executors(id uuid PRIMARY KEY,active_runs integer);
            CREATE TEMP TABLE run_events(run_id uuid,sequence bigint,event_id uuid,kind text,payload jsonb,created_at timestamptz);
            CREATE TEMP TABLE instance_execution_reservations(run_id uuid PRIMARY KEY,lease_token uuid,completion jsonb);
        "#).execute(&pool).await.unwrap();
        let migration = include_str!("../migrations/0057_runner_execution_stops.sql")
            .replace("CREATE TABLE", "CREATE TEMP TABLE")
            .replace(
                "immutable_runner_attempt()",
                "pg_temp.immutable_runner_attempt()",
            );
        sqlx::raw_sql(&migration).execute(&pool).await.unwrap();
        let run = Uuid::new_v4();
        let successor = Uuid::new_v4();
        let thread = Uuid::new_v4();
        let project = Uuid::new_v4();
        let user = Uuid::new_v4();
        let worker = Uuid::new_v4();
        let token = Uuid::new_v4();
        let attempt = Uuid::new_v4();
        let workspace = Uuid::new_v4();
        let session = Uuid::new_v4();
        let spec = RunSpec {
            schema_version: SCHEMA_VERSION,
            id: run,
            thread_id: thread,
            project_id: project,
            project: cowork_contracts::FrozenReference {
                id: project,
                revision: 1,
            },
            project_privacy: cowork_contracts::ProjectPrivacy::PrivateLocal,
            task: None,
            creator_user_id: user,
            executor_target: ExecutorTarget::ServerLinux { pool_id: None },
            required_capabilities: vec![],
            input: json!({}),
            model_profile_id: None,
            snapshot_id: None,
            idempotency_key: Uuid::new_v4().to_string(),
            created_at: Utc::now(),
        };
        sqlx::query("INSERT INTO runs(id,thread_id,project_id,creator_user_id,state,spec,assigned_executor_id,lease_owner,lease_token,lease_expires_at,started_at,updated_at) VALUES($1,$2,$3,$4,'running',$5,$6,$6,$7,clock_timestamp()+interval '1 minute',clock_timestamp(),clock_timestamp())")
            .bind(run).bind(thread).bind(project).bind(user).bind(serde_json::to_value(&spec).unwrap()).bind(worker).bind(token).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO runs(id,thread_id,state) VALUES($1,$2,'queued')")
            .bind(successor)
            .bind(thread)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO executors VALUES($1,1)")
            .bind(worker)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO instance_execution_reservations(run_id,lease_token) VALUES($1,$2)",
        )
        .bind(run)
        .bind(token)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO desktop_sessions VALUES($1,$2,'starting',NULL,NULL,'{\"start_pending\":true}')").bind(session).bind(run).execute(&pool).await.unwrap();
        prepare_runner_attempt(&pool, run, worker, token, attempt, Some(workspace), b"body")
            .await
            .unwrap();
        let pending = begin_server_cancel(&pool, run).await.unwrap();
        assert_eq!(pending.run.state, RunState::Interrupted);
        assert_eq!(pending.run.error.as_ref().unwrap().code, "cancel_pending");
        let again = begin_server_cancel(&pool, run).await.unwrap();
        assert_eq!(pending.cancel_id, again.cancel_id);
        assert_eq!(
            sqlx::query_scalar::<_, i32>("SELECT active_runs FROM executors WHERE id=$1")
                .bind(worker)
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        assert!(sqlx::query_scalar::<_, Option<Value>>(
            "SELECT completion FROM instance_execution_reservations WHERE run_id=$1"
        )
        .bind(run)
        .fetch_one(&pool)
        .await
        .unwrap()
        .is_none());
        assert!(prepare_runner_attempt(
            &pool,
            run,
            worker,
            token,
            Uuid::new_v4(),
            Some(workspace),
            b"late"
        )
        .await
        .is_err());
        assert!(transition_run(&pool, run, RunState::Canceled, None, None)
            .await
            .is_err());
        let mut tx = pool.begin().await.unwrap();
        let claimed = sqlx::query(SERVER_CLAIM_CANDIDATES_SQL)
            .bind(TERMINAL_STATES)
            .bind(25_i64)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        assert!(!claimed
            .iter()
            .any(|row| row.get::<Uuid, _>("id") == successor));
        tx.rollback().await.unwrap();
        let request = pending.stop.unwrap();
        let mut wrong = request.clone();
        wrong.attempts.clear();
        let proof = crate::runner_admission::verified_stop_for_test(&wrong);
        assert!(
            finish_server_cancel(&pool, run, pending.cancel_id.unwrap(), &proof)
                .await
                .is_err()
        );
        assert_eq!(
            get_run(&pool, run).await.unwrap().state,
            RunState::Interrupted
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT state FROM desktop_sessions WHERE id=$1")
                .bind(session)
                .fetch_one(&pool)
                .await
                .unwrap(),
            "starting"
        );
        let proof = crate::runner_admission::verified_stop_for_test(&request);
        let canceled = finish_server_cancel(&pool, run, pending.cancel_id.unwrap(), &proof)
            .await
            .unwrap();
        assert_eq!(canceled.state, RunState::Canceled);
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT state FROM desktop_sessions WHERE id=$1")
                .bind(session)
                .fetch_one(&pool)
                .await
                .unwrap(),
            "ended"
        );
        assert_eq!(
            finish_server_cancel(&pool, run, pending.cancel_id.unwrap(), &proof)
                .await
                .unwrap()
                .revision,
            canceled.revision
        );
        let mut tx = pool.begin().await.unwrap();
        let claimed = sqlx::query(SERVER_CLAIM_CANDIDATES_SQL)
            .bind(TERMINAL_STATES)
            .bind(25_i64)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        assert!(claimed
            .iter()
            .any(|row| row.get::<Uuid, _>("id") == successor));
        tx.rollback().await.unwrap();
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to an isolated PostgreSQL test database"]
    async fn postgres_server_claim_rotation_reaches_candidate_26_without_overtaking_a_thread() {
        let pool = waiting_test_pool().await;
        let creator = Uuid::new_v4();
        let mut runs = Vec::new();
        let mut threads = Vec::new();
        for index in 0..26 {
            let run = Uuid::new_v4();
            let thread = Uuid::new_v4();
            sqlx::query("INSERT INTO runs (id,thread_id,creator_user_id,created_at) VALUES ($1,$2,$3,now()-interval '1 minute'+make_interval(secs => $4))")
                .bind(run).bind(thread).bind(creator).bind(f64::from(index)).execute(&pool).await.unwrap();
            runs.push(run);
            threads.push(thread);
        }
        let mut tx = pool.begin().await.unwrap();
        let first_scan = sqlx::query(SERVER_CLAIM_CANDIDATES_SQL)
            .bind(TERMINAL_STATES)
            .bind(25_i64)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        assert_eq!(first_scan.len(), 25);
        assert!(first_scan
            .iter()
            .all(|row| row.get::<Uuid, _>("id") != runs[25]));
        for row in first_scan {
            // Twenty-five permanently blocked runs from the same user formerly
            // filled every poll's candidate window, starving the next thread.
            record_run_waiting(&mut tx, row.get("id"), RunWaitingReason::UserCapacity)
                .await
                .unwrap();
        }
        tx.commit().await.unwrap();

        let successor = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO runs (id,thread_id,creator_user_id,created_at) VALUES ($1,$2,$3,now())",
        )
        .bind(successor)
        .bind(threads[0])
        .bind(creator)
        .execute(&pool)
        .await
        .unwrap();
        let mut tx = pool.begin().await.unwrap();
        let next_scan = sqlx::query(SERVER_CLAIM_CANDIDATES_SQL)
            .bind(TERMINAL_STATES)
            .bind(25_i64)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        assert_eq!(next_scan[0].get::<Uuid, _>("id"), runs[25]);
        assert!(!next_scan
            .iter()
            .any(|row| row.get::<Uuid, _>("id") == successor));
        // A previously waiting, now admitted run keeps the exact owner/token
        // fence and loses its stale diagnostic in the same claim transaction.
        record_run_waiting(&mut tx, runs[25], RunWaitingReason::InstanceCapacity)
            .await
            .unwrap();
        let worker = Uuid::new_v4();
        let token = Uuid::new_v4();
        let claimed = sqlx::query(CLAIM_LEASE_SQL)
            .bind(runs[25])
            .bind(worker)
            .bind(token)
            .bind(30_f64)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(claimed.get::<String, _>("state"), "running");
        assert_eq!(claimed.get::<Uuid, _>("lease_owner"), worker);
        assert_eq!(claimed.get::<Uuid, _>("lease_token"), token);
        let still_waiting: i64 =
            sqlx::query_scalar("SELECT count(*) FROM run_waiting_reasons WHERE run_id=$1")
                .bind(runs[25])
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert_eq!(still_waiting, 0);
        tx.commit().await.unwrap();

        // Continued arrivals must not preempt older already-checked candidates:
        // their creation time follows those candidates' previous check time.
        let newcomer = Uuid::new_v4();
        sqlx::query("INSERT INTO runs (id,thread_id,creator_user_id) VALUES ($1,$2,$3)")
            .bind(newcomer)
            .bind(Uuid::new_v4())
            .bind(creator)
            .execute(&pool)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        let rotation = sqlx::query(SERVER_CLAIM_CANDIDATES_SQL)
            .bind(TERMINAL_STATES)
            .bind(25_i64)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        assert_eq!(rotation[0].get::<Uuid, _>("id"), runs[0]);
        assert!(!rotation
            .iter()
            .any(|row| row.get::<Uuid, _>("id") == newcomer));
        tx.rollback().await.unwrap();
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to an isolated PostgreSQL test database"]
    async fn postgres_waiting_reasons_require_current_thread_access_and_clear_on_state_change() {
        let pool = waiting_test_pool().await;
        let owner = Uuid::new_v4();
        let viewer = Uuid::new_v4();
        let outsider = Uuid::new_v4();
        let project = Uuid::new_v4();
        let thread = Uuid::new_v4();
        let run = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id) VALUES ($1),($2),($3)")
            .bind(owner)
            .bind(viewer)
            .bind(outsider)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO projects (id,owner_user_id) VALUES ($1,$2)")
            .bind(project)
            .bind(owner)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO threads (id,project_id) VALUES ($1,$2)")
            .bind(thread)
            .bind(project)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO project_members VALUES ($1,$2,'viewer')")
            .bind(project)
            .bind(viewer)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO runs (id,thread_id,project_id,creator_user_id) VALUES ($1,$2,$3,$4)",
        )
        .bind(run)
        .bind(thread)
        .bind(project)
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();

        use cowork_contracts::enterprise_execution::ExecutionCapacityReason;
        for reason in [
            RunWaitingReason::from(ExecutionCapacityReason::Platform),
            RunWaitingReason::from(ExecutionCapacityReason::Organisation),
            RunWaitingReason::from(ExecutionCapacityReason::User),
            RunWaitingReason::from(ExecutionCapacityReason::Instance),
            RunWaitingReason::ChatWorkspaceBusy,
            RunWaitingReason::AdmissionUnavailable,
        ] {
            let mut tx = pool.begin().await.unwrap();
            sqlx::query("SELECT id FROM runs WHERE id=$1 FOR UPDATE")
                .bind(run)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            record_run_waiting(&mut tx, run, reason).await.unwrap();
            tx.commit().await.unwrap();
            for user in [owner, viewer] {
                let waiting = get_run_waiting(&pool, user, run).await.unwrap();
                assert_eq!(waiting.reason, Some(reason));
                assert!(waiting.checked_at.is_some());
                let serialized = serde_json::to_value(waiting).unwrap();
                assert_eq!(serialized.as_object().unwrap().len(), 3);
                assert_eq!(serialized["reason"], reason.name());
            }
            assert!(matches!(
                get_run_waiting(&pool, outsider, run).await,
                Err(ApiError::Unauthorized(_))
            ));
        }
        sqlx::query("DELETE FROM project_members WHERE user_id=$1")
            .bind(viewer)
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            get_run_waiting(&pool, viewer, run).await,
            Err(ApiError::Unauthorized(_))
        ));
        sqlx::query("UPDATE threads SET deleted_at=now() WHERE id=$1")
            .bind(thread)
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            get_run_waiting(&pool, owner, run).await,
            Err(ApiError::Unprocessable(_))
        ));
        sqlx::query("UPDATE threads SET deleted_at=NULL WHERE id=$1")
            .bind(thread)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE runs SET state='failed' WHERE id=$1")
            .bind(run)
            .execute(&pool)
            .await
            .unwrap();
        let waiting = get_run_waiting(&pool, owner, run).await.unwrap();
        assert!(waiting.reason.is_none());
        assert!(waiting.checked_at.is_none());
        let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM run_waiting_reasons")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, 0);
        sqlx::query("UPDATE runs SET state='queued' WHERE id=$1")
            .bind(run)
            .execute(&pool)
            .await
            .unwrap();
        let waiting = get_run_waiting(&pool, owner, run).await.unwrap();
        assert!(
            waiting.reason.is_none(),
            "a requeued run must not inherit an old reason"
        );
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to an isolated PostgreSQL test database"]
    async fn postgres_runner_capacity_waiting_is_atomic_and_fenced_to_the_current_lease() {
        let pool = waiting_test_pool().await;
        sqlx::raw_sql(r#"
            ALTER TABLE runs ADD COLUMN next_event_sequence bigint NOT NULL DEFAULT 1;
            CREATE TEMP TABLE run_checkpoints (
                id uuid PRIMARY KEY,run_id uuid,sequence bigint,safe_to_resume boolean,executor_state jsonb
            );
            CREATE TEMP TABLE run_events (
                run_id uuid,sequence bigint,event_id uuid,kind text,payload jsonb,created_at timestamptz
            );
        "#).execute(&pool).await.unwrap();
        let user = Uuid::new_v4();
        let project = Uuid::new_v4();
        let thread = Uuid::new_v4();
        let run = Uuid::new_v4();
        let worker = Uuid::new_v4();
        let token = Uuid::new_v4();
        sqlx::query("INSERT INTO users(id) VALUES ($1)")
            .bind(user)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO projects(id,owner_user_id) VALUES ($1,$2)")
            .bind(project)
            .bind(user)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO threads(id,project_id) VALUES ($1,$2)")
            .bind(thread)
            .bind(project)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO runs(id,thread_id,project_id,creator_user_id,state,lease_owner,lease_token,lease_expires_at) VALUES ($1,$2,$3,$4,'running',$5,$6,clock_timestamp()+interval '1 minute')")
            .bind(run).bind(thread).bind(project).bind(user).bind(worker).bind(token).execute(&pool).await.unwrap();
        prepare_runner_request(
            &pool,
            run,
            worker,
            token,
            json!({"phase":"sandbox_dispatched"}),
        )
        .await
        .unwrap();
        assert!(get_run_waiting(&pool, user, run)
            .await
            .unwrap()
            .reason
            .is_none());
        // A failure writing the wait rolls back its checkpoint AND its event.
        sqlx::raw_sql(
            r#"
            CREATE FUNCTION pg_temp.reject_test_wait() RETURNS trigger LANGUAGE plpgsql AS $$
            BEGIN RAISE EXCEPTION 'isolated wait write failure'; END; $$;
            CREATE TRIGGER reject_test_wait BEFORE INSERT ON run_waiting_reasons
            FOR EACH ROW EXECUTE FUNCTION pg_temp.reject_test_wait();
        "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(mark_runner_capacity_waiting(&pool, run, worker, token)
            .await
            .is_err());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM run_checkpoints")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM run_events")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
        sqlx::query("DROP TRIGGER reject_test_wait ON run_waiting_reasons")
            .execute(&pool)
            .await
            .unwrap();
        mark_runner_capacity_waiting(&pool, run, worker, token)
            .await
            .unwrap();
        let waiting = get_run_waiting(&pool, user, run).await.unwrap();
        assert_eq!(waiting.reason, Some(RunWaitingReason::RunnerCapacity));
        assert!(waiting.checked_at.is_some());
        assert_eq!(
            serde_json::to_value(waiting)
                .unwrap()
                .as_object()
                .unwrap()
                .len(),
            3
        );
        assert!(sqlx::query_scalar::<_, bool>(
            "SELECT safe_to_resume FROM run_checkpoints ORDER BY sequence DESC LIMIT 1"
        )
        .fetch_one(&pool)
        .await
        .unwrap());
        for (other_worker, other_token) in [(Uuid::new_v4(), token), (worker, Uuid::new_v4())] {
            assert!(
                mark_runner_capacity_waiting(&pool, run, other_worker, other_token)
                    .await
                    .is_err()
            );
            assert!(
                prepare_runner_request(&pool, run, other_worker, other_token, json!({}))
                    .await
                    .is_err()
            );
            assert!(
                check_runner_capacity_waiting_lease(&pool, run, other_worker, other_token)
                    .await
                    .is_err()
            );
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM run_checkpoints")
                .fetch_one(&pool)
                .await
                .unwrap(),
            2
        );
        prepare_runner_request(
            &pool,
            run,
            worker,
            token,
            json!({"phase":"sandbox_dispatched"}),
        )
        .await
        .unwrap();
        assert!(get_run_waiting(&pool, user, run)
            .await
            .unwrap()
            .reason
            .is_none());
        assert!(!sqlx::query_scalar::<_, bool>(
            "SELECT safe_to_resume FROM run_checkpoints ORDER BY sequence DESC LIMIT 1"
        )
        .fetch_one(&pool)
        .await
        .unwrap());
        mark_runner_capacity_waiting(&pool, run, worker, token)
            .await
            .unwrap();
        // Owner replacement clears old waits; old proofs cannot grant a new lease.
        let replacement = Uuid::new_v4();
        let replacement_token = Uuid::new_v4();
        sqlx::query("UPDATE runs SET lease_owner=$2,lease_token=$3 WHERE id=$1")
            .bind(run)
            .bind(replacement)
            .bind(replacement_token)
            .execute(&pool)
            .await
            .unwrap();
        assert!(get_run_waiting(&pool, user, run)
            .await
            .unwrap()
            .reason
            .is_none());
        assert!(mark_runner_capacity_waiting(&pool, run, worker, token)
            .await
            .is_err());
        mark_runner_capacity_waiting(&pool, run, replacement, replacement_token)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE runs SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE id=$1",
        )
        .bind(run)
        .execute(&pool)
        .await
        .unwrap();
        assert!(get_run_waiting(&pool, user, run)
            .await
            .unwrap()
            .reason
            .is_none());
        assert!(
            check_runner_capacity_waiting_lease(&pool, run, replacement, replacement_token)
                .await
                .is_err()
        );
        assert!(
            mark_runner_capacity_waiting(&pool, run, replacement, replacement_token)
                .await
                .is_err()
        );
        sqlx::query(
            "UPDATE runs SET lease_expires_at=clock_timestamp()+interval '1 minute' WHERE id=$1",
        )
        .bind(run)
        .execute(&pool)
        .await
        .unwrap();
        mark_runner_capacity_waiting(&pool, run, replacement, replacement_token)
            .await
            .unwrap();
        sqlx::query("UPDATE runs SET state='canceled' WHERE id=$1")
            .bind(run)
            .execute(&pool)
            .await
            .unwrap();
        assert!(get_run_waiting(&pool, user, run)
            .await
            .unwrap()
            .reason
            .is_none());
        assert!(
            check_runner_capacity_waiting_lease(&pool, run, replacement, replacement_token)
                .await
                .is_err()
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM run_waiting_reasons")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to an isolated PostgreSQL test database"]
    async fn postgres_claims_recovery_and_renewal_mint_expiry_with_database_time() {
        use sqlx::Connection;
        let url = std::env::var("COWORK_TEST_DATABASE_URL").expect("isolated test database URL");
        let mut connection = sqlx::PgConnection::connect(&url).await.unwrap();
        // Session-local rows exercise the production SQL without changing a
        // database clock or any application table.
        sqlx::raw_sql(
            r#"
            CREATE TEMP TABLE runs (
                id uuid PRIMARY KEY, state text, revision bigint NOT NULL DEFAULT 1,
                assigned_executor_id uuid, lease_owner uuid, lease_token uuid,
                lease_expires_at timestamptz, started_at timestamptz, updated_at timestamptz
            );
        "#,
        )
        .execute(&mut connection)
        .await
        .unwrap();
        let run = Uuid::new_v4();
        let worker = Uuid::new_v4();
        let token = Uuid::new_v4();
        sqlx::query("INSERT INTO runs (id,state) VALUES ($1,'queued')")
            .bind(run)
            .execute(&mut connection)
            .await
            .unwrap();
        for offset_seconds in [-300_i64, 300_i64] {
            let mut tx = connection.begin().await.unwrap();
            let database_now: DateTime<Utc> = sqlx::query_scalar("SELECT now()")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            // An application wall clock five minutes ahead or behind must not
            // supply either claim's or renewal's authoritative timestamp.
            let simulated_application_now =
                database_now + chrono::Duration::seconds(offset_seconds);
            sqlx::query("UPDATE runs SET state='queued',lease_expires_at=$1")
                .bind(simulated_application_now + chrono::Duration::seconds(30))
                .execute(&mut *tx)
                .await
                .unwrap();
            // Server and external-executor claims share this exact mint SQL.
            let claimed = sqlx::query(CLAIM_LEASE_SQL)
                .bind(run)
                .bind(worker)
                .bind(token)
                .bind(30_f64)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            let database_expiry = database_now + chrono::Duration::seconds(30);
            assert_eq!(
                claimed.get::<DateTime<Utc>, _>("lease_expires_at"),
                database_expiry
            );
            assert_eq!(claimed.get::<Uuid, _>("lease_token"), token);
            let recovered = sqlx::query(RECOVER_EXTERNAL_LEASE_SQL)
                .bind(run)
                .bind(45_f64)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            assert_eq!(
                recovered.get::<DateTime<Utc>, _>("lease_expires_at"),
                database_now + chrono::Duration::seconds(45)
            );
            assert_eq!(recovered.get::<Uuid, _>("lease_token"), token);
            let renewed = sqlx::query(RENEW_LEASE_SQL)
                .bind(run)
                .bind(worker)
                .bind(token)
                .bind(30_f64)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            assert_eq!(
                renewed.get::<DateTime<Utc>, _>("lease_expires_at"),
                database_expiry
            );
            tx.rollback().await.unwrap();
        }
    }

    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to an isolated PostgreSQL test database"]
    async fn postgres_renewal_and_shutdown_interrupt_enforce_owner_token_and_expiry() {
        use sqlx::Connection;
        let url = std::env::var("COWORK_TEST_DATABASE_URL").expect("isolated test database URL");
        let mut connection = sqlx::PgConnection::connect(&url).await.unwrap();
        // Session-local tables mask public tables; no application migration or
        // live run is changed by this test.
        sqlx::raw_sql(r#"
            CREATE TEMP TABLE runs (
                id uuid PRIMARY KEY, lease_owner uuid, lease_token uuid,
                lease_expires_at timestamptz, state text, target_kind text,
                revision bigint NOT NULL DEFAULT 1, error jsonb, updated_at timestamptz
            );
            CREATE TEMP TABLE run_checkpoints (run_id uuid, sequence bigint, safe_to_resume boolean);
        "#).execute(&mut connection).await.unwrap();
        let run = Uuid::new_v4();
        let worker = Uuid::new_v4();
        let token = Uuid::new_v4();
        sqlx::query("INSERT INTO runs (id, lease_owner, lease_token, lease_expires_at, state, target_kind) VALUES ($1,$2,$3,now()-interval '1 second','running','server_linux')")
            .bind(run).bind(worker).bind(token).execute(&mut connection).await.unwrap();
        let renewal = sqlx::query(RENEW_LEASE_SQL)
            .bind(run)
            .bind(worker)
            .bind(token)
            .bind(30_f64)
            .fetch_optional(&mut connection)
            .await
            .unwrap();
        assert!(renewal.is_none(), "an expired lease must never be revived");
        sqlx::query("UPDATE runs SET lease_expires_at=now()+interval '30 seconds'")
            .execute(&mut connection)
            .await
            .unwrap();
        for (owner, lease) in [(Uuid::new_v4(), token), (worker, Uuid::new_v4())] {
            assert!(sqlx::query(RENEW_LEASE_SQL)
                .bind(run)
                .bind(owner)
                .bind(lease)
                .bind(30_f64)
                .fetch_optional(&mut connection)
                .await
                .unwrap()
                .is_none());
            assert!(sqlx::query(INTERRUPT_SERVER_OWNED_LEASE_SQL)
                .bind(run)
                .bind(owner)
                .bind(lease)
                .fetch_optional(&mut connection)
                .await
                .unwrap()
                .is_none());
        }
        assert!(sqlx::query(RENEW_LEASE_SQL)
            .bind(run)
            .bind(worker)
            .bind(token)
            .bind(30_f64)
            .fetch_optional(&mut connection)
            .await
            .unwrap()
            .is_some());
        sqlx::query("INSERT INTO run_checkpoints VALUES ($1,1,true),($1,2,false)")
            .bind(run)
            .execute(&mut connection)
            .await
            .unwrap();
        let interrupted = sqlx::query(INTERRUPT_SERVER_OWNED_LEASE_SQL)
            .bind(run)
            .bind(worker)
            .bind(token)
            .fetch_one(&mut connection)
            .await
            .unwrap();
        assert_eq!(interrupted.get::<String, _>("state"), "interrupted");
        assert!(interrupted.get::<Option<Uuid>, _>("lease_owner").is_none());
        assert!(interrupted.get::<Option<Uuid>, _>("lease_token").is_none());
        let error: Value = interrupted.get("error");
        assert_eq!(error["code"], "unsafe_tool_interrupted");
        assert_eq!(error["retryable"], false);
        assert_eq!(error["details"]["manual_review_required"], true);
        assert!(sqlx::query(INTERRUPT_SERVER_OWNED_LEASE_SQL)
            .bind(run)
            .bind(worker)
            .bind(token)
            .fetch_optional(&mut connection)
            .await
            .unwrap()
            .is_none());
        // Target and expiry are independently fenced even with the right token.
        sqlx::query("UPDATE runs SET state='running',lease_owner=$1,lease_token=$2,target_kind='personal_device',lease_expires_at=now()+interval '30 seconds'")
            .bind(worker).bind(token).execute(&mut connection).await.unwrap();
        assert!(sqlx::query(INTERRUPT_SERVER_OWNED_LEASE_SQL)
            .bind(run)
            .bind(worker)
            .bind(token)
            .fetch_optional(&mut connection)
            .await
            .unwrap()
            .is_none());
        sqlx::query(
            "UPDATE runs SET target_kind='server_linux',lease_expires_at=now()-interval '1 second'",
        )
        .execute(&mut connection)
        .await
        .unwrap();
        assert!(sqlx::query(INTERRUPT_SERVER_OWNED_LEASE_SQL)
            .bind(run)
            .bind(worker)
            .bind(token)
            .fetch_optional(&mut connection)
            .await
            .unwrap()
            .is_none());
    }

    fn registration(kind: ExecutorKind, server_names: &[&str]) -> ExecutorRegistration {
        ExecutorRegistration {
            schema_version: SCHEMA_VERSION,
            executor_id: Uuid::new_v4(),
            kind,
            pool_id: Some(Uuid::new_v4()),
            owner_user_id: None,
            display_name: "executor".to_owned(),
            protocol_version: SCHEMA_VERSION,
            capabilities: vec![CapabilityDescriptor {
                schema_version: SCHEMA_VERSION,
                name: Capability::from("tool.mcp.invoke"),
                version: "test".to_owned(),
                attributes: BTreeMap::from([("server_names".to_owned(), json!(server_names))]),
            }],
            labels: BTreeMap::new(),
            personal_device_remote_control: None,
            max_concurrent_runs: 1,
        }
    }

    #[test]
    fn managed_windows_claims_only_runs_matching_its_local_mcp_names() {
        let input = json!({
            "frozen_runtime_context":{"mcp_metadata":[
                {"definition":{"name":"Docs"}}
            ]}
        });
        let required = [Capability::from("tool.mcp.invoke")];
        assert!(supports_run(
            &registration(ExecutorKind::ManagedWindows, &["Docs", "CRM"]),
            &required,
            &input,
        ));
        assert!(!supports_run(
            &registration(ExecutorKind::ManagedWindows, &["CRM"]),
            &required,
            &input,
        ));
        let mut crew_executor = registration(ExecutorKind::ManagedWindows, &["Docs"]);
        crew_executor.capabilities.push(CapabilityDescriptor {
            schema_version: SCHEMA_VERSION,
            name: Capability::from("crew.python"),
            version: "test".to_owned(),
            attributes: BTreeMap::new(),
        });
        assert!(supports_run(
            &crew_executor,
            &required,
            &json!({
                "task_runner":"crew",
                "frozen_runtime_context":{"mcp_metadata":[
                    {"definition":{"name":"Docs"}}
                ]}
            }),
        ));
        assert!(!supports_run(
            &registration(ExecutorKind::ManagedWindows, &["Docs"]),
            &required,
            &json!({
                "task_runner":"crew",
                "frozen_runtime_context":{"mcp_metadata":[
                    {"definition":{"name":"Docs"}}
                ]}
            }),
        ));
        assert!(supports_run(
            &registration(ExecutorKind::PersonalDevice, &[]),
            &required,
            &input,
        ));
    }

    #[test]
    fn external_model_usage_is_bounded_and_never_understates_the_split() {
        assert_eq!(
            external_model_usage_tokens(
                RunEventKind::ModelCompleted,
                &json!({"usage":{"prompt_tokens":20,"completion_tokens":10,"total_tokens":5}}),
            )
            .unwrap(),
            Some(30),
        );
        assert_eq!(
            external_model_usage_tokens(
                RunEventKind::ModelCompleted,
                &json!({"usage":{"total_tokens":42}}),
            )
            .unwrap(),
            Some(42),
        );
        assert_eq!(
            external_model_usage_tokens(RunEventKind::ModelDelta, &json!({"usage":99})).unwrap(),
            None,
        );
        assert!(external_model_usage_tokens(
            RunEventKind::ModelCompleted,
            &json!({"usage":{"total_tokens":-1}}),
        )
        .is_err());
    }

    #[tokio::test]
    async fn external_model_events_require_a_stable_id_before_database_access() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://127.0.0.1/unused")
            .unwrap();
        let error = append_external_leased_event(
            &pool,
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            None,
            RunEventKind::ModelStarted,
            json!({}),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, ApiError::Unprocessable(_)));
    }
}

#[cfg(test)]
mod dispatch_denial_tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to an isolated PostgreSQL test database"]
    async fn postgres_dispatch_denial_requires_observed_model_and_no_tool_effects() {
        let url=std::env::var("COWORK_TEST_DATABASE_URL").expect("isolated test database");
        let pool=sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(&url).await.unwrap();
        sqlx::raw_sql("CREATE TEMP TABLE run_runner_attempts(run_id uuid); CREATE TEMP TABLE run_events(run_id uuid,kind text); CREATE TEMP TABLE run_checkpoints(run_id uuid,sequence bigint,safe_to_resume boolean,origin text);")
            .execute(&pool).await.unwrap();
        let run=Uuid::new_v4();
        async fn stopped(pool:&PgPool,run:Uuid)->bool {
            sqlx::query_scalar(SERVER_DISPATCH_DENIAL_STOPPED_SQL).bind(run).fetch_one(pool).await.unwrap()
        }
        assert!(stopped(&pool,run).await,"denial before any model or tool dispatch");
        sqlx::query("INSERT INTO run_events VALUES($1,'model_started')").bind(run).execute(&pool).await.unwrap();
        assert!(!stopped(&pool,run).await,"unobserved model remains fenced");
        sqlx::query("INSERT INTO run_checkpoints VALUES($1,1,true,'server_agent_v1')").bind(run).execute(&pool).await.unwrap();
        assert!(!stopped(&pool,run).await,"a safe cursor cannot hide an unobserved model call");
        sqlx::query("INSERT INTO run_events VALUES($1,'model_completed')").bind(run).execute(&pool).await.unwrap();
        assert!(stopped(&pool,run).await,"observed model and trusted safe cursor");
        sqlx::query("UPDATE run_checkpoints SET origin='executor'").execute(&pool).await.unwrap();
        assert!(!stopped(&pool,run).await,"external checkpoint cannot prove server stop");
        sqlx::query("UPDATE run_checkpoints SET origin='server_agent_v1',safe_to_resume=false").execute(&pool).await.unwrap();
        assert!(!stopped(&pool,run).await,"unsafe cursor remains fenced");
        sqlx::query("UPDATE run_checkpoints SET safe_to_resume=true").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO run_runner_attempts VALUES($1)").bind(run).execute(&pool).await.unwrap();
        assert!(!stopped(&pool,run).await,"runner attempts require their physical stop proof");
        sqlx::query("DELETE FROM run_runner_attempts").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO run_events VALUES($1,'tool_started')").bind(run).execute(&pool).await.unwrap();
        assert!(!stopped(&pool,run).await,"tool effects cannot use a dispatch-denial release");
        pool.close().await;
    }
}
