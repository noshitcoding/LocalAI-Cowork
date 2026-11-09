//! Projects messages and touches their materialized parent threads.

use crate::{
    error::ApiError,
    sync::{
        materialization::{is_materialized, remember_materialization},
        validation::object_payload,
    },
};
use chrono::{DateTime, Utc};
use cowork_contracts::SyncOperation;
use serde_json::Value;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

async fn touch_materialized_thread(
    tx: &mut Transaction<'_, Postgres>,
    thread_id: Uuid,
) -> Result<(), ApiError> {
    sqlx::query(
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
    Ok(())
}

pub(in crate::sync) async fn materialize_message(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    message_id: Uuid,
    operation: SyncOperation,
    payload: Option<&Value>,
) -> Result<(), ApiError> {
    let tracked = is_materialized(tx, user_id, "message", message_id).await?;
    let current = sqlx::query("SELECT thread_id FROM messages WHERE id = $1 FOR UPDATE")
        .bind(message_id)
        .fetch_optional(&mut **tx)
        .await?;
    if current.is_some() && !tracked {
        return Err(ApiError::Conflict(
            "synced message ID collides with an independent server message".to_owned(),
        ));
    }
    if operation == SyncOperation::Delete {
        if tracked {
            if let Some(row) = current {
                let thread_id: Uuid = row.try_get("thread_id")?;
                sqlx::query(
                    r#"
                    UPDATE messages
                    SET revision = revision + 1,
                        etag = 'W/"' || id::text || ':' || (revision + 1)::text || '"',
                        deleted_at = now(), updated_at = now()
                    WHERE id = $1
                    "#,
                )
                .bind(message_id)
                .execute(&mut **tx)
                .await?;
                touch_materialized_thread(tx, thread_id).await?;
            }
        }
        return Ok(());
    }
    let object = object_payload(payload, "message")?;
    let thread_id = object
        .get("thread_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| {
            ApiError::Unprocessable("message thread_id must be a UUID string".to_owned())
        })?;
    let role = object
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("assistant");
    if !matches!(role, "user" | "assistant" | "system" | "tool") {
        return Err(ApiError::Unprocessable(
            "message role is not supported".to_owned(),
        ));
    }
    let thread_exists = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM threads thread
            JOIN projects project ON project.id = thread.project_id
            WHERE thread.id = $1 AND thread.deleted_at IS NULL
              AND project.owner_user_id = $2 AND project.privacy = 'private_local'
              AND project.deleted_at IS NULL
        )
        "#,
    )
    .bind(thread_id)
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await?;
    if !thread_exists {
        return Ok(());
    }
    let content = payload.cloned().unwrap_or_else(|| serde_json::json!({}));
    let author_user_id = (role == "user").then_some(user_id);
    let created_at = object
        .get("timestamp")
        .and_then(Value::as_i64)
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .unwrap_or_else(Utc::now);
    if current.is_some() {
        sqlx::query(
            r#"
            UPDATE messages
            SET revision = revision + 1,
                etag = 'W/"' || id::text || ':' || (revision + 1)::text || '"',
                thread_id = $2, author_user_id = $3, role = $4, content = $5,
                run_id = NULL, deleted_at = NULL, updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(message_id)
        .bind(thread_id)
        .bind(author_user_id)
        .bind(role)
        .bind(content)
        .execute(&mut **tx)
        .await?;
    } else {
        let etag = format!("W/\"{message_id}:1\"");
        sqlx::query(
            r#"
            INSERT INTO messages (
                id, revision, etag, thread_id, author_user_id, role, content,
                run_id, created_at, updated_at
            ) VALUES ($1, 1, $2, $3, $4, $5, $6, NULL, $7, $7)
            "#,
        )
        .bind(message_id)
        .bind(etag)
        .bind(thread_id)
        .bind(author_user_id)
        .bind(role)
        .bind(content)
        .bind(created_at)
        .execute(&mut **tx)
        .await?;
    }
    remember_materialization(tx, user_id, "message", message_id).await?;
    touch_materialized_thread(tx, thread_id).await?;
    Ok(())
}
