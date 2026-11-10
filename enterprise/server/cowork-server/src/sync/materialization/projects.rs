//! Projects private project changes and reconciles their existing thread projections.

use crate::{
    error::ApiError,
    sync::{
        materialization::{
            is_materialized, remember_materialization,
            threads::{materialize_thread, soft_delete_thread},
        },
        publication::publish_server_tombstone_tx,
        records::parse_operation,
        validation::{object_payload, optional_text, required_text},
    },
};
use cowork_contracts::{ExecutorTarget, SyncOperation};
use serde_json::Value;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

pub(in crate::sync) async fn materialize_project(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    project_id: Uuid,
    operation: SyncOperation,
    payload: Option<&Value>,
) -> Result<(), ApiError> {
    let tracked = is_materialized(tx, user_id, "project", project_id).await?;
    let current =
        sqlx::query("SELECT owner_user_id, privacy FROM projects WHERE id = $1 FOR UPDATE")
            .bind(project_id)
            .fetch_optional(&mut **tx)
            .await?;
    if current.is_some() && !tracked {
        return Err(ApiError::Conflict(
            "synced project ID collides with an independent server project".to_owned(),
        ));
    }
    if let Some(row) = &current {
        if row.try_get::<Uuid, _>("owner_user_id")? != user_id
            || row.try_get::<&str, _>("privacy")? != "private_local"
        {
            return Err(ApiError::Conflict(
                "synced projects may only materialize into their owner's private project"
                    .to_owned(),
            ));
        }
    }
    if operation == SyncOperation::Delete {
        if tracked {
            let thread_ids = sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM threads WHERE project_id = $1 AND deleted_at IS NULL ORDER BY id FOR UPDATE",
            )
            .bind(project_id)
            .fetch_all(&mut **tx)
            .await?;
            let message_ids = sqlx::query_scalar::<_, Uuid>(
                r#"
                SELECT message.id FROM messages message
                JOIN threads thread ON thread.id = message.thread_id
                WHERE thread.project_id = $1 AND message.deleted_at IS NULL
                ORDER BY message.id FOR UPDATE OF message
                "#,
            )
            .bind(project_id)
            .fetch_all(&mut **tx)
            .await?;
            let mut task_ids = sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM task_definitions WHERE project_id = $1 AND deleted_at IS NULL ORDER BY id, revision FOR UPDATE",
            )
            .bind(project_id)
            .fetch_all(&mut **tx)
            .await?;
            task_ids.dedup();
            let schedule_ids = sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM schedules WHERE project_id = $1 AND deleted_at IS NULL ORDER BY id FOR UPDATE",
            )
            .bind(project_id)
            .fetch_all(&mut **tx)
            .await?;
            sqlx::query(
                r#"
                UPDATE messages AS message
                SET revision = message.revision + 1,
                    etag = 'W/"' || message.id::text || ':' || (message.revision + 1)::text || '"',
                    deleted_at = now(), updated_at = now()
                FROM threads thread
                WHERE message.thread_id = thread.id AND thread.project_id = $1
                  AND message.deleted_at IS NULL
                "#,
            )
            .bind(project_id)
            .execute(&mut **tx)
            .await?;
            sqlx::query(
                r#"
                UPDATE schedules
                SET revision = revision + 1,
                    etag = 'W/"' || id::text || ':' || (revision + 1)::text || '"',
                    enabled = FALSE, next_run_at = NULL,
                    blocked_reason = 'project deleted', deleted_at = now(), updated_at = now()
                WHERE project_id = $1 AND deleted_at IS NULL
                "#,
            )
            .bind(project_id)
            .execute(&mut **tx)
            .await?;
            sqlx::query(
                "UPDATE task_definitions SET released = FALSE, deleted_at = now() WHERE project_id = $1 AND deleted_at IS NULL",
            )
            .bind(project_id)
            .execute(&mut **tx)
            .await?;
            sqlx::query(
                r#"
                UPDATE threads
                SET revision = revision + 1,
                    etag = 'W/"' || id::text || ':' || (revision + 1)::text || '"',
                    deleted_at = now(), updated_at = now()
                WHERE project_id = $1 AND deleted_at IS NULL
                "#,
            )
            .bind(project_id)
            .execute(&mut **tx)
            .await?;
            sqlx::query(
                r#"
                UPDATE projects
                SET revision = revision + 1,
                    etag = 'W/"' || id::text || ':' || (revision + 1)::text || '"',
                    deleted_at = now(), updated_at = now()
                WHERE id = $1 AND owner_user_id = $2 AND privacy = 'private_local'
                "#,
            )
            .bind(project_id)
            .bind(user_id)
            .execute(&mut **tx)
            .await?;
            for message_id in message_ids {
                publish_server_tombstone_tx(tx, user_id, "message", message_id).await?;
            }
            for thread_id in thread_ids {
                publish_server_tombstone_tx(tx, user_id, "thread", thread_id).await?;
            }
            for schedule_id in schedule_ids {
                publish_server_tombstone_tx(tx, user_id, "schedule", schedule_id).await?;
            }
            for task_id in task_ids {
                publish_server_tombstone_tx(tx, user_id, "task", task_id).await?;
            }
        }
        return Ok(());
    }

    let object = object_payload(payload, "project")?;
    if object
        .get("project_kind")
        .and_then(Value::as_str)
        .is_some_and(|kind| !matches!(kind, "private" | "private_local"))
    {
        return Err(ApiError::Unprocessable(
            "personal metadata sync cannot create team projects".to_owned(),
        ));
    }
    let name = required_text(object, &["title", "name"], "project name", 200)?;
    let description = optional_text(object, &["instructions", "description"], 200_000)?;
    let policy = object
        .get("policy")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    if !policy.is_object() {
        return Err(ApiError::Unprocessable(
            "project policy must be a JSON object".to_owned(),
        ));
    }
    let preferred_target = object.get("preferred_executor_target").cloned();
    if let Some(target) = &preferred_target {
        serde_json::from_value::<ExecutorTarget>(target.clone()).map_err(|error| {
            ApiError::Unprocessable(format!("invalid preferred executor target: {error}"))
        })?;
    }
    if current.is_some() {
        sqlx::query(
            r#"
            UPDATE projects
            SET revision = revision + 1,
                etag = 'W/"' || id::text || ':' || (revision + 1)::text || '"',
                name = $2, description = $3, preferred_executor_target = $4,
                policy = $5, deleted_at = NULL, updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(project_id)
        .bind(name)
        .bind(description)
        .bind(preferred_target)
        .bind(policy)
        .execute(&mut **tx)
        .await?;
    } else {
        let etag = format!("W/\"{project_id}:1\"");
        sqlx::query(
            r#"
            INSERT INTO projects (
                id, revision, etag, owner_user_id, team_id, privacy, name,
                description, preferred_executor_target, policy
            ) VALUES ($1, 1, $2, $3, NULL, 'private_local', $4, $5, $6, $7)
            "#,
        )
        .bind(project_id)
        .bind(etag)
        .bind(user_id)
        .bind(name)
        .bind(description)
        .bind(preferred_target)
        .bind(policy)
        .execute(&mut **tx)
        .await?;
    }
    remember_materialization(tx, user_id, "project", project_id).await?;
    reconcile_project_threads(tx, user_id, project_id).await?;
    Ok(())
}

async fn reconcile_project_threads(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    project_id: Uuid,
) -> Result<(), ApiError> {
    let mut thread_ids = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT materialized.entity_id
        FROM sync_materializations materialized
        JOIN threads thread ON thread.id = materialized.entity_id
        WHERE materialized.user_id = $1 AND materialized.entity_type = 'thread'
          AND thread.project_id = $2
        "#,
    )
    .bind(user_id)
    .bind(project_id)
    .fetch_all(&mut **tx)
    .await?;
    let desired = sqlx::query_scalar::<_, Value>(
        r#"
        SELECT COALESCE(payload -> 'thread_ids', '[]'::jsonb) FROM sync_entities
        WHERE user_id = $1 AND entity_type = 'project' AND entity_id = $2
          AND NOT tombstone
        "#,
    )
    .bind(user_id)
    .bind(project_id)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(Value::Array(ids)) = desired {
        for value in ids {
            let raw = value.as_str().ok_or_else(|| {
                ApiError::Unprocessable("project thread_ids must contain UUID strings".to_owned())
            })?;
            let id = Uuid::parse_str(raw).map_err(|_| {
                ApiError::Unprocessable("project thread_ids must contain UUID strings".to_owned())
            })?;
            if !thread_ids.contains(&id) {
                thread_ids.push(id);
            }
        }
    }
    for thread_id in thread_ids {
        let entity = sqlx::query(
            r#"
            SELECT operation, payload FROM (
                SELECT CASE WHEN tombstone THEN 'delete' ELSE 'upsert' END AS operation,
                       payload
                FROM sync_entities
                WHERE user_id = $1 AND entity_type = 'thread' AND entity_id = $2
            ) current
            "#,
        )
        .bind(user_id)
        .bind(thread_id)
        .fetch_optional(&mut **tx)
        .await?;
        if let Some(entity) = entity {
            let operation = parse_operation(entity.try_get("operation")?)?;
            let payload = entity.try_get::<Option<Value>, _>("payload")?;
            materialize_thread(tx, user_id, thread_id, operation, payload.as_ref()).await?;
        } else if is_materialized(tx, user_id, "thread", thread_id).await? {
            soft_delete_thread(tx, user_id, thread_id).await?;
        }
    }
    Ok(())
}
