//! Projects task configuration, executor targets and dependent schedule references.

use crate::{
    error::ApiError,
    sync::{
        materialization::{
            is_materialized, remember_materialization, schedules::reconcile_schedules_for_reference,
        },
        publication::publish_canonical_schedule_tx,
        validation::{object_payload, required_text},
    },
    workflow,
};
use chrono::{DateTime, Utc};
use cowork_contracts::{Capability, ExecutorTarget, SyncOperation};
use serde_json::Value;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

async fn private_task_project(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    object: &serde_json::Map<String, Value>,
) -> Result<Option<Uuid>, ApiError> {
    let explicit_project_id = object
        .get("project_id")
        .and_then(Value::as_str)
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| ApiError::Unprocessable("task project_id must be a UUID string".to_owned()))?;
    let thread_id = object
        .get("thread_id")
        .and_then(Value::as_str)
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| ApiError::Unprocessable("task thread_id must be a UUID string".to_owned()))?;
    let thread_project_id = if let Some(thread_id) = thread_id {
        sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT thread.project_id FROM threads thread
            JOIN projects project ON project.id = thread.project_id
            WHERE thread.id = $1 AND thread.deleted_at IS NULL
              AND project.owner_user_id = $2 AND project.privacy = 'private_local'
              AND project.deleted_at IS NULL
            "#,
        )
        .bind(thread_id)
        .bind(user_id)
        .fetch_optional(&mut **tx)
        .await?
    } else {
        None
    };
    if explicit_project_id.is_some()
        && thread_project_id.is_some()
        && explicit_project_id != thread_project_id
    {
        return Err(ApiError::Conflict(
            "task project_id and thread_id refer to different projects".to_owned(),
        ));
    }
    let Some(project_id) = explicit_project_id.or(thread_project_id) else {
        return Ok(None);
    };
    let allowed = sqlx::query_scalar::<_, bool>(
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
    Ok(allowed.then_some(project_id))
}

fn task_projection_config(object: &serde_json::Map<String, Value>) -> Result<Value, ApiError> {
    let task_kind = object
        .get("task_kind")
        .and_then(Value::as_str)
        .unwrap_or("work");
    if !matches!(task_kind, "work" | "plan") {
        return Err(ApiError::Unprocessable(
            "task_kind must be work or plan".to_owned(),
        ));
    }
    let runner = object
        .get("runner")
        .and_then(Value::as_str)
        .unwrap_or("model");
    if !matches!(runner, "model" | "crew") {
        return Err(ApiError::Unprocessable(
            "task runner must be model or crew".to_owned(),
        ));
    }
    Ok(serde_json::json!({
        "sync_metadata": {
            "task_kind": task_kind,
            "expected_output": object.get("expected_output").cloned().unwrap_or(Value::Null),
            "thread_id": object.get("thread_id").cloned().unwrap_or(Value::Null),
            "runner": runner,
            "crew_id": object.get("crew_id").cloned().unwrap_or(Value::Null),
            "model": object.get("model").cloned().unwrap_or(Value::Null),
            "backend_selection": object.get("backend_selection").cloned().unwrap_or(Value::Null),
            "schedule_expression": object.get("schedule_expression").cloned().unwrap_or(Value::Null),
            "schedule_enabled": object.get("schedule_enabled").cloned().unwrap_or(Value::Bool(false)),
            "status": object.get("status").cloned().unwrap_or(Value::Null),
            "note": object.get("note").cloned().unwrap_or(Value::Null)
        }
    }))
}

pub(in crate::sync) async fn materialize_task(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    task_id: Uuid,
    operation: SyncOperation,
    payload: Option<&Value>,
) -> Result<(), ApiError> {
    let tracked = is_materialized(tx, user_id, "task", task_id).await?;
    let current = sqlx::query(
        "SELECT * FROM task_definitions WHERE id = $1 ORDER BY revision DESC LIMIT 1 FOR UPDATE",
    )
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    if current.is_some() && !tracked {
        return Err(ApiError::Conflict(
            "synced task ID collides with an independent server task".to_owned(),
        ));
    }
    if operation == SyncOperation::Delete {
        if tracked {
            let schedule_ids = sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM schedules WHERE task_id = $1 AND deleted_at IS NULL ORDER BY id FOR UPDATE",
            )
            .bind(task_id)
            .fetch_all(&mut **tx)
            .await?;
            sqlx::query(
                "UPDATE task_definitions SET released = FALSE, deleted_at = COALESCE(deleted_at, now()) WHERE id = $1",
            )
            .bind(task_id)
            .execute(&mut **tx)
            .await?;
            sqlx::query(
                "UPDATE schedules SET enabled = FALSE, next_run_at = NULL, blocked_reason = 'task deleted', updated_at = now() WHERE task_id = $1 AND deleted_at IS NULL",
            )
            .bind(task_id)
            .execute(&mut **tx)
            .await?;
            for schedule_id in schedule_ids {
                publish_canonical_schedule_tx(tx, schedule_id).await?;
            }
        }
        return Ok(());
    }
    let object = object_payload(payload, "task")?;
    let Some(project_id) = private_task_project(tx, user_id, object).await? else {
        return Ok(());
    };
    let name = required_text(object, &["title", "name"], "task name", 200)?;
    let instructions = required_text(
        object,
        &["description", "instructions"],
        "task instructions",
        1_000_000,
    )?;
    let requested_capabilities = object
        .get("required_capabilities")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
    let requested_capabilities = serde_json::from_value::<Vec<Capability>>(requested_capabilities)
        .map_err(|error| {
            ApiError::Unprocessable(format!("invalid task required_capabilities: {error}"))
        })?;
    let default_target = object.get("default_executor_target").cloned();
    if let Some(target) = &default_target {
        serde_json::from_value::<ExecutorTarget>(target.clone()).map_err(|error| {
            ApiError::Unprocessable(format!("invalid task default_executor_target: {error}"))
        })?;
    }
    let config = task_projection_config(object)?;
    let required_capabilities = serde_json::to_value(workflow::effective_task_capabilities(
        &requested_capabilities,
        &config,
    )?)?;
    if let Some(row) = &current {
        let unchanged = row.try_get::<Uuid, _>("project_id")? == project_id
            && row.try_get::<String, _>("name")? == name
            && row.try_get::<String, _>("instructions")? == instructions
            && row.try_get::<Value, _>("required_capabilities")? == required_capabilities
            && row.try_get::<Option<Value>, _>("default_executor_target")? == default_target
            && row.try_get::<Value, _>("config")? == config
            && row
                .try_get::<Option<DateTime<Utc>>, _>("deleted_at")?
                .is_none();
        if unchanged {
            remember_materialization(tx, user_id, "task", task_id).await?;
            reconcile_schedules_for_reference(tx, user_id, "task_id", task_id).await?;
            return Ok(());
        }
    }
    let revision = current
        .as_ref()
        .map(|row| row.try_get::<i64, _>("revision"))
        .transpose()?
        .unwrap_or(0)
        + 1;
    sqlx::query("UPDATE task_definitions SET released = FALSE WHERE id = $1 AND released")
        .bind(task_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        r#"
        INSERT INTO task_definitions (
            id, revision, etag, project_id, name, instructions,
            required_capabilities, default_executor_target, config,
            released, created_by
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, TRUE, $10)
        "#,
    )
    .bind(task_id)
    .bind(revision)
    .bind(format!("W/\"{task_id}:{revision}\""))
    .bind(project_id)
    .bind(name)
    .bind(instructions)
    .bind(required_capabilities)
    .bind(default_target)
    .bind(config)
    .bind(user_id)
    .execute(&mut **tx)
    .await?;
    remember_materialization(tx, user_id, "task", task_id).await?;
    reconcile_schedules_for_reference(tx, user_id, "task_id", task_id).await?;
    Ok(())
}
