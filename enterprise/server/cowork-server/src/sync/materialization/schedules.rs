//! Projects schedules and validates their existing task and provider references.

use crate::{
    error::ApiError,
    sync::{
        materialization::{is_materialized, remember_materialization},
        validation::{object_payload, required_text, required_uuid},
    },
};
use chrono::{DateTime, Utc};
use cowork_contracts::{ExecutorTarget, SyncOperation};
use serde_json::Value;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

pub(in crate::sync) async fn reconcile_schedules_for_reference(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    reference_key: &str,
    reference_id: Uuid,
) -> Result<(), ApiError> {
    if !matches!(
        reference_key,
        "task_id" | "thread_id" | "project_id" | "model_profile_id"
    ) {
        return Err(ApiError::Internal(anyhow::anyhow!(
            "unsupported schedule reference key {reference_key}"
        )));
    }
    let schedules = sqlx::query(
        r#"
        SELECT entity_id, tombstone, payload FROM sync_entities
        WHERE user_id = $1 AND entity_type = 'schedule'
          AND payload ->> $2 = $3
        ORDER BY entity_id
        "#,
    )
    .bind(user_id)
    .bind(reference_key)
    .bind(reference_id.to_string())
    .fetch_all(&mut **tx)
    .await?;
    for schedule in schedules {
        let schedule_id = schedule.try_get("entity_id")?;
        let operation = if schedule.try_get("tombstone")? {
            SyncOperation::Delete
        } else {
            SyncOperation::Upsert
        };
        let payload = schedule.try_get::<Option<Value>, _>("payload")?;
        materialize_schedule(tx, user_id, schedule_id, operation, payload.as_ref()).await?;
    }
    Ok(())
}

async fn validate_synced_schedule_target(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    project_id: Uuid,
    target: &ExecutorTarget,
) -> Result<(), ApiError> {
    let allowed = match target {
        ExecutorTarget::ServerLinux { pool_id: None } => true,
        ExecutorTarget::ServerLinux {
            pool_id: Some(pool_id),
        } => {
            sqlx::query_scalar::<_, bool>(
                r#"
                SELECT EXISTS(
                    SELECT 1 FROM executor_pools pool
                    JOIN executor_pool_project_grants grant_row ON grant_row.pool_id = pool.id
                    WHERE pool.id = $1 AND grant_row.project_id = $2
                      AND pool.kind = 'server_linux' AND pool.deleted_at IS NULL
                )
                "#,
            )
            .bind(pool_id)
            .bind(project_id)
            .fetch_one(&mut **tx)
            .await?
        }
        ExecutorTarget::ManagedWindowsPool { pool_id } => {
            sqlx::query_scalar::<_, bool>(
                r#"
                SELECT EXISTS(
                    SELECT 1 FROM executor_pools pool
                    JOIN executor_pool_project_grants grant_row ON grant_row.pool_id = pool.id
                    WHERE pool.id = $1 AND grant_row.project_id = $2
                      AND pool.kind = 'managed_windows' AND pool.deleted_at IS NULL
                )
                "#,
            )
            .bind(pool_id)
            .bind(project_id)
            .fetch_one(&mut **tx)
            .await?
        }
        ExecutorTarget::PersonalDevice { device_id } => {
            sqlx::query_scalar::<_, bool>(
                r#"
                SELECT EXISTS(
                    SELECT 1 FROM executors
                    WHERE id = $1 AND owner_user_id = $2 AND kind = 'personal_device'
                )
                "#,
            )
            .bind(device_id)
            .bind(user_id)
            .fetch_one(&mut **tx)
            .await?
        }
    };
    if allowed {
        Ok(())
    } else {
        Err(ApiError::Unauthorized(
            "schedule executor target is not available to this private project".to_owned(),
        ))
    }
}

pub(in crate::sync) async fn materialize_schedule(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    schedule_id: Uuid,
    operation: SyncOperation,
    payload: Option<&Value>,
) -> Result<(), ApiError> {
    let tracked = is_materialized(tx, user_id, "schedule", schedule_id).await?;
    let current = sqlx::query("SELECT * FROM schedules WHERE id = $1 FOR UPDATE")
        .bind(schedule_id)
        .fetch_optional(&mut **tx)
        .await?;
    if current.is_some() && !tracked {
        return Err(ApiError::Conflict(
            "synced schedule ID collides with an independent server schedule".to_owned(),
        ));
    }
    if operation == SyncOperation::Delete {
        if tracked {
            sqlx::query(
                r#"
                UPDATE schedules
                SET revision = revision + 1,
                    etag = 'W/"' || id::text || ':' || (revision + 1)::text || '"',
                    enabled = FALSE, next_run_at = NULL, deleted_at = now(), updated_at = now()
                WHERE id = $1 AND deleted_at IS NULL
                "#,
            )
            .bind(schedule_id)
            .execute(&mut **tx)
            .await?;
        }
        return Ok(());
    }
    let object = object_payload(payload, "schedule")?;
    let task_id = required_uuid(object, "task_id", "schedule task_id")?;
    let project_id = required_uuid(object, "project_id", "schedule project_id")?;
    let thread_id = required_uuid(object, "thread_id", "schedule thread_id")?;
    let project_allowed = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM projects
            WHERE id = $1 AND owner_user_id = $2 AND privacy = 'private_local'
              AND deleted_at IS NULL
        )
        "#,
    )
    .bind(project_id)
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await?;
    if !project_allowed {
        return Ok(());
    }
    let task_allowed = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM task_definitions WHERE id = $1 AND project_id = $2 AND deleted_at IS NULL)",
    )
    .bind(task_id)
    .bind(project_id)
    .fetch_one(&mut **tx)
    .await?;
    let thread_allowed = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM threads WHERE id = $1 AND project_id = $2 AND deleted_at IS NULL)",
    )
    .bind(thread_id)
    .bind(project_id)
    .fetch_one(&mut **tx)
    .await?;
    if !task_allowed || !thread_allowed {
        return Ok(());
    }
    let cron = crate::workflow::normalized_cron(&required_text(
        object,
        &["cron", "cron_expression"],
        "schedule cron",
        500,
    )?)?;
    let timezone = crate::workflow::validated_timezone(&required_text(
        object,
        &["timezone"],
        "schedule timezone",
        200,
    )?)?;
    let target_value = object.get("executor_target").cloned().ok_or_else(|| {
        ApiError::Unprocessable("schedule executor_target is required".to_owned())
    })?;
    let target: ExecutorTarget = serde_json::from_value(target_value.clone()).map_err(|error| {
        ApiError::Unprocessable(format!("invalid schedule executor_target: {error}"))
    })?;
    validate_synced_schedule_target(tx, user_id, project_id, &target).await?;
    let input = object
        .get("input")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let requested_profile_id = object
        .get("model_profile_id")
        .and_then(Value::as_str)
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| {
            ApiError::Unprocessable("schedule model_profile_id must be a UUID string".to_owned())
        })?;
    let expected_profile_binding = if matches!(target, ExecutorTarget::PersonalDevice { .. }) {
        "per_device"
    } else {
        "server"
    };
    let profile_defaults = if let Some(profile_id) = requested_profile_id {
        sqlx::query_scalar::<_, Value>(
            "SELECT model_defaults FROM provider_profiles WHERE id = $1 AND owner_user_id = $2 AND deleted_at IS NULL",
        )
        .bind(profile_id)
        .bind(user_id)
        .fetch_optional(&mut **tx)
        .await?
    } else {
        None
    };
    let profile_binding = profile_defaults
        .as_ref()
        .and_then(|defaults| defaults.get("endpoint_binding"))
        .and_then(Value::as_str);
    let model_profile_id = requested_profile_id.filter(|_| {
        profile_defaults.is_some() && profile_binding == Some(expected_profile_binding)
    });
    let enabled = object
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let target_json = serde_json::to_value(&target)?;
    if let Some(row) = &current {
        let unchanged = row.try_get::<Uuid, _>("task_id")? == task_id
            && row.try_get::<Uuid, _>("project_id")? == project_id
            && row.try_get::<Uuid, _>("thread_id")? == thread_id
            && row.try_get::<String, _>("cron_expression")? == cron
            && row.try_get::<String, _>("timezone")? == timezone.name()
            && row.try_get::<Value, _>("executor_target")? == target_json
            && row.try_get::<Value, _>("input")? == input
            && row.try_get::<Option<Uuid>, _>("model_profile_id")? == model_profile_id
            && row.try_get::<bool, _>("enabled")? == enabled
            && row
                .try_get::<Option<DateTime<Utc>>, _>("deleted_at")?
                .is_none();
        if unchanged {
            let stored_revision = row.try_get::<i64, _>("revision")?;
            crate::organisation_mfa::bind_schedule(tx, schedule_id, user_id, stored_revision)
                .await?;
            remember_materialization(tx, user_id, "schedule", schedule_id).await?;
            return Ok(());
        }
    }
    let revision = current
        .as_ref()
        .map(|row| row.try_get::<i64, _>("revision"))
        .transpose()?
        .unwrap_or(0)
        + 1;
    let blocked_reason = requested_profile_id.and_then(|_| {
        if profile_defaults.is_none() {
            Some("waiting for model profile metadata")
        } else if model_profile_id.is_none() {
            Some("model profile is bound to a different executor class")
        } else {
            None
        }
    });
    let next_run_at = if enabled && blocked_reason.is_none() {
        Some(crate::workflow::next_occurrence_normalized(
            &cron,
            timezone,
            Utc::now(),
        )?)
    } else {
        None
    };
    if current.is_some() {
        sqlx::query(
            r#"
            UPDATE schedules
            SET revision = $2, etag = $3, task_id = $4, project_id = $5,
                thread_id = $6, cron_expression = $7, timezone = $8,
                executor_target = $9, input = $10, model_profile_id = $11,
                enabled = $12, next_run_at = $13, blocked_reason = $14,
                deleted_at = NULL, updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(schedule_id)
        .bind(revision)
        .bind(format!("W/\"{schedule_id}:{revision}\""))
        .bind(task_id)
        .bind(project_id)
        .bind(thread_id)
        .bind(cron)
        .bind(timezone.name())
        .bind(target_json)
        .bind(input)
        .bind(model_profile_id)
        .bind(enabled)
        .bind(next_run_at)
        .bind(blocked_reason)
        .execute(&mut **tx)
        .await?;
    } else {
        sqlx::query(
            r#"
            INSERT INTO schedules (
                id, revision, etag, task_id, project_id, thread_id,
                cron_expression, timezone, executor_target, input,
                model_profile_id, enabled, next_run_at, blocked_reason, created_by
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
            "#,
        )
        .bind(schedule_id)
        .bind(revision)
        .bind(format!("W/\"{schedule_id}:{revision}\""))
        .bind(task_id)
        .bind(project_id)
        .bind(thread_id)
        .bind(cron)
        .bind(timezone.name())
        .bind(target_json)
        .bind(input)
        .bind(model_profile_id)
        .bind(enabled)
        .bind(next_run_at)
        .bind(blocked_reason)
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    }
    remember_materialization(tx, user_id, "schedule", schedule_id).await?;
    Ok(())
}
