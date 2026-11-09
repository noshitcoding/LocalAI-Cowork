//! Projects thread changes, dependent tasks and messages, including soft deletion.

use crate::{
    error::ApiError,
    sync::{
        materialization::{
            is_materialized, messages::materialize_message, remember_materialization,
            schedules::reconcile_schedules_for_reference, tasks::materialize_task,
        },
        publication::publish_server_tombstone_tx,
        validation::{object_payload, required_text},
    },
};
use cowork_contracts::SyncOperation;
use serde_json::Value;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

async fn project_for_thread(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    thread_id: Uuid,
) -> Result<Option<Uuid>, ApiError> {
    let rows = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT entity_id FROM sync_entities
        WHERE user_id = $1 AND entity_type = 'project' AND NOT tombstone
          AND jsonb_typeof(payload -> 'thread_ids') = 'array'
          AND (payload -> 'thread_ids') ? $2
        ORDER BY entity_id
        LIMIT 2
        "#,
    )
    .bind(user_id)
    .bind(thread_id.to_string())
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() > 1 {
        return Err(ApiError::Conflict(format!(
            "thread {thread_id} is assigned to more than one synced project"
        )));
    }
    Ok(rows.into_iter().next())
}

pub(in crate::sync) async fn soft_delete_thread(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    thread_id: Uuid,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        UPDATE messages AS message
        SET revision = message.revision + 1,
            etag = 'W/"' || message.id::text || ':' || (message.revision + 1)::text || '"',
            deleted_at = now(), updated_at = now()
        FROM threads thread
        JOIN projects project ON project.id = thread.project_id
        WHERE message.thread_id = thread.id AND thread.id = $1
          AND project.owner_user_id = $2 AND project.privacy = 'private_local'
          AND message.deleted_at IS NULL
        "#,
    )
    .bind(thread_id)
    .bind(user_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE schedules AS schedule
        SET revision = schedule.revision + 1,
            etag = 'W/"' || schedule.id::text || ':' || (schedule.revision + 1)::text || '"',
            enabled = FALSE, next_run_at = NULL,
            blocked_reason = 'thread unavailable', deleted_at = now(), updated_at = now()
        FROM threads thread
        JOIN projects project ON project.id = thread.project_id
        WHERE schedule.thread_id = thread.id AND thread.id = $1
          AND project.owner_user_id = $2 AND project.privacy = 'private_local'
          AND schedule.deleted_at IS NULL
        "#,
    )
    .bind(thread_id)
    .bind(user_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE threads AS thread
        SET revision = thread.revision + 1,
            etag = 'W/"' || thread.id::text || ':' || (thread.revision + 1)::text || '"',
            deleted_at = now(), updated_at = now()
        FROM projects project
        WHERE thread.id = $1 AND thread.project_id = project.id
          AND project.owner_user_id = $2 AND project.privacy = 'private_local'
        "#,
    )
    .bind(thread_id)
    .bind(user_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(in crate::sync) async fn materialize_thread(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    thread_id: Uuid,
    operation: SyncOperation,
    payload: Option<&Value>,
) -> Result<(), ApiError> {
    let tracked = is_materialized(tx, user_id, "thread", thread_id).await?;
    let current = sqlx::query("SELECT id FROM threads WHERE id = $1 FOR UPDATE")
        .bind(thread_id)
        .fetch_optional(&mut **tx)
        .await?;
    if current.is_some() && !tracked {
        return Err(ApiError::Conflict(
            "synced thread ID collides with an independent server thread".to_owned(),
        ));
    }
    if operation == SyncOperation::Delete {
        if tracked {
            let message_ids = sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM messages WHERE thread_id = $1 AND deleted_at IS NULL ORDER BY id FOR UPDATE",
            )
            .bind(thread_id)
            .fetch_all(&mut **tx)
            .await?;
            let schedule_ids = sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM schedules WHERE thread_id = $1 AND deleted_at IS NULL ORDER BY id FOR UPDATE",
            )
            .bind(thread_id)
            .fetch_all(&mut **tx)
            .await?;
            soft_delete_thread(tx, user_id, thread_id).await?;
            for message_id in message_ids {
                publish_server_tombstone_tx(tx, user_id, "message", message_id).await?;
            }
            for schedule_id in schedule_ids {
                publish_server_tombstone_tx(tx, user_id, "schedule", schedule_id).await?;
            }
        }
        return Ok(());
    }
    let object = object_payload(payload, "thread")?;
    let title = required_text(object, &["title"], "thread title", 200)?;
    let Some(project_id) = project_for_thread(tx, user_id, thread_id).await? else {
        if tracked {
            soft_delete_thread(tx, user_id, thread_id).await?;
        }
        return Ok(());
    };
    let project_exists = sqlx::query_scalar::<_, bool>(
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
    if !project_exists {
        return Ok(());
    }
    if current.is_some() {
        sqlx::query(
            r#"
            UPDATE threads
            SET revision = revision + 1,
                etag = 'W/"' || id::text || ':' || (revision + 1)::text || '"',
                project_id = $2, title = $3, deleted_at = NULL, updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(thread_id)
        .bind(project_id)
        .bind(title)
        .execute(&mut **tx)
        .await?;
    } else {
        let etag = format!("W/\"{thread_id}:1\"");
        sqlx::query(
            r#"
            INSERT INTO threads (
                id, revision, etag, project_id, created_by, title
            ) VALUES ($1, 1, $2, $3, $4, $5)
            "#,
        )
        .bind(thread_id)
        .bind(etag)
        .bind(project_id)
        .bind(user_id)
        .bind(title)
        .execute(&mut **tx)
        .await?;
    }
    remember_materialization(tx, user_id, "thread", thread_id).await?;
    let messages = sqlx::query(
        r#"
        SELECT entity_id, tombstone, payload FROM sync_entities
        WHERE user_id = $1 AND entity_type = 'message'
          AND payload ->> 'thread_id' = $2
        ORDER BY entity_id
        "#,
    )
    .bind(user_id)
    .bind(thread_id.to_string())
    .fetch_all(&mut **tx)
    .await?;
    for message in messages {
        let id = message.try_get("entity_id")?;
        let operation = if message.try_get("tombstone")? {
            SyncOperation::Delete
        } else {
            SyncOperation::Upsert
        };
        let payload = message.try_get::<Option<Value>, _>("payload")?;
        materialize_message(tx, user_id, id, operation, payload.as_ref()).await?;
    }
    reconcile_thread_tasks(tx, user_id, thread_id).await?;
    reconcile_schedules_for_reference(tx, user_id, "thread_id", thread_id).await?;
    Ok(())
}

async fn reconcile_thread_tasks(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    thread_id: Uuid,
) -> Result<(), ApiError> {
    let tasks = sqlx::query(
        r#"
        SELECT entity_id, tombstone, payload FROM sync_entities
        WHERE user_id = $1 AND entity_type = 'task'
          AND payload ->> 'thread_id' = $2
        ORDER BY entity_id
        "#,
    )
    .bind(user_id)
    .bind(thread_id.to_string())
    .fetch_all(&mut **tx)
    .await?;
    for task in tasks {
        let task_id = task.try_get("entity_id")?;
        let operation = if task.try_get("tombstone")? {
            SyncOperation::Delete
        } else {
            SyncOperation::Upsert
        };
        let payload = task.try_get::<Option<Value>, _>("payload")?;
        materialize_task(tx, user_id, task_id, operation, payload.as_ref()).await?;
    }
    Ok(())
}
