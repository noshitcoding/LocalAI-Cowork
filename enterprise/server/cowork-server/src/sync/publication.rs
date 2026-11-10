//! Publishes canonical server changes and tombstones into the user sync journal.

use crate::{
    error::ApiError,
    governance,
    sync::{
        materialization::remember_materialization,
        validation::{validate_entity_type, MAX_SYNC_PAYLOAD_BYTES},
    },
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{postgres::PgRow, Postgres, Row, Transaction};
use uuid::Uuid;

pub(crate) async fn publish_canonical_project_tx(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
) -> Result<bool, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT id, owner_user_id, privacy, name, description,
               preferred_executor_target, policy, created_at, updated_at
        FROM projects WHERE id = $1 AND deleted_at IS NULL
        "#,
    )
    .bind(project_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else { return Ok(false) };
    if row.try_get::<&str, _>("privacy")? != "private_local" {
        return Ok(false);
    }
    let user_id: Uuid = row.try_get("owner_user_id")?;
    let thread_ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM threads WHERE project_id = $1 AND deleted_at IS NULL ORDER BY created_at, id",
    )
    .bind(project_id)
    .fetch_all(&mut **tx)
    .await?;
    let payload = serde_json::json!({
        "title": row.try_get::<String, _>("name")?,
        "instructions": row.try_get::<String, _>("description")?,
        "thread_ids": thread_ids,
        "project_kind": "private",
        "files_location": "personal_device",
        "preferred_executor_target": row.try_get::<Option<Value>, _>("preferred_executor_target")?,
        "policy": row.try_get::<Value, _>("policy")?,
        "created_at": row.try_get::<DateTime<Utc>, _>("created_at")?,
        "updated_at": row.try_get::<DateTime<Utc>, _>("updated_at")?,
        "source": "server",
    });
    publish_server_entity_tx(tx, user_id, "project", project_id, payload).await?;
    remember_materialization(tx, user_id, "project", project_id).await?;
    Ok(true)
}

pub(crate) async fn publish_canonical_thread_tx(
    tx: &mut Transaction<'_, Postgres>,
    thread_id: Uuid,
) -> Result<bool, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT thread.id, thread.project_id, thread.title, thread.created_at,
               thread.updated_at, project.owner_user_id, project.privacy
        FROM threads thread
        JOIN projects project ON project.id = thread.project_id
        WHERE thread.id = $1 AND thread.deleted_at IS NULL AND project.deleted_at IS NULL
        "#,
    )
    .bind(thread_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else { return Ok(false) };
    if row.try_get::<&str, _>("privacy")? != "private_local" {
        return Ok(false);
    }
    let project_id: Uuid = row.try_get("project_id")?;
    publish_canonical_project_tx(tx, project_id).await?;
    let user_id: Uuid = row.try_get("owner_user_id")?;
    let payload = serde_json::json!({
        "title": row.try_get::<String, _>("title")?,
        "provider_settings": {},
        "runner": "model",
        "crew_id": null,
        "created_at": row.try_get::<DateTime<Utc>, _>("created_at")?,
        "updated_at": row.try_get::<DateTime<Utc>, _>("updated_at")?,
        "source": "server",
    });
    publish_server_entity_tx(tx, user_id, "thread", thread_id, payload).await?;
    remember_materialization(tx, user_id, "thread", thread_id).await?;
    Ok(true)
}

pub(crate) async fn publish_canonical_message_tx(
    tx: &mut Transaction<'_, Postgres>,
    message_id: Uuid,
) -> Result<bool, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT message.id, message.thread_id, message.role, message.content,
               message.run_id, message.created_at, project.owner_user_id, project.privacy
        FROM messages message
        JOIN threads thread ON thread.id = message.thread_id
        JOIN projects project ON project.id = thread.project_id
        WHERE message.id = $1 AND message.deleted_at IS NULL
          AND thread.deleted_at IS NULL AND project.deleted_at IS NULL
        "#,
    )
    .bind(message_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else { return Ok(false) };
    if row.try_get::<&str, _>("privacy")? != "private_local" {
        return Ok(false);
    }
    let thread_id: Uuid = row.try_get("thread_id")?;
    publish_canonical_thread_tx(tx, thread_id).await?;
    let user_id: Uuid = row.try_get("owner_user_id")?;
    let content: Value = row.try_get("content")?;
    let (text, truncated) = bounded_message_text(&content, 200_000);
    let payload = serde_json::json!({
        "thread_id": thread_id,
        "role": row.try_get::<String, _>("role")?,
        "content": text,
        "content_truncated": truncated,
        "timestamp": row.try_get::<DateTime<Utc>, _>("created_at")?.timestamp_millis(),
        "attachment_descriptors": [],
        "durable_run_id": row.try_get::<Option<Uuid>, _>("run_id")?,
        "source": "server",
    });
    publish_server_entity_tx(tx, user_id, "message", message_id, payload).await?;
    remember_materialization(tx, user_id, "message", message_id).await?;
    Ok(true)
}

pub(crate) async fn publish_canonical_task_tx(
    tx: &mut Transaction<'_, Postgres>,
    task_id: Uuid,
) -> Result<bool, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT task.*, project.owner_user_id, project.privacy
        FROM task_definitions task
        JOIN projects project ON project.id = task.project_id
        WHERE task.id = $1 AND task.deleted_at IS NULL
          AND project.deleted_at IS NULL
        ORDER BY task.revision DESC
        LIMIT 1
        "#,
    )
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(false);
    };
    if row.try_get::<&str, _>("privacy")? != "private_local" {
        return Ok(false);
    }
    let user_id: Uuid = row.try_get("owner_user_id")?;
    let config: Value = row.try_get("config")?;
    let metadata = config.get("sync_metadata").and_then(Value::as_object);
    let metadata_value = |key: &str| {
        metadata
            .and_then(|object| object.get(key))
            .cloned()
            .unwrap_or(Value::Null)
    };
    let task_kind = metadata
        .and_then(|object| object.get("task_kind"))
        .and_then(Value::as_str)
        .unwrap_or("work");
    let runner = metadata
        .and_then(|object| object.get("runner"))
        .and_then(Value::as_str)
        .unwrap_or("model");
    let payload = serde_json::json!({
        "task_kind": task_kind,
        "title": row.try_get::<String, _>("name")?,
        "description": row.try_get::<String, _>("instructions")?,
        "expected_output": metadata_value("expected_output"),
        "project_id": row.try_get::<Uuid, _>("project_id")?,
        "thread_id": metadata_value("thread_id"),
        "runner": runner,
        "crew_id": metadata_value("crew_id"),
        "model": metadata_value("model"),
        "backend_selection": metadata_value("backend_selection"),
        "schedule_expression": metadata_value("schedule_expression"),
        "schedule_enabled": metadata_value("schedule_enabled"),
        "status": metadata_value("status"),
        "note": metadata_value("note"),
        "required_capabilities": row.try_get::<Value, _>("required_capabilities")?,
        "default_executor_target": row.try_get::<Option<Value>, _>("default_executor_target")?,
        "released": row.try_get::<bool, _>("released")?,
        "canonical_revision": row.try_get::<i64, _>("revision")?,
        "created_at": row.try_get::<DateTime<Utc>, _>("created_at")?,
        "source": "server"
    });
    publish_server_entity_tx(tx, user_id, "task", task_id, payload).await?;
    remember_materialization(tx, user_id, "task", task_id).await?;
    Ok(true)
}

pub(crate) async fn publish_canonical_schedule_tx(
    tx: &mut Transaction<'_, Postgres>,
    schedule_id: Uuid,
) -> Result<bool, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT schedule.*, project.owner_user_id, project.privacy
        FROM schedules schedule
        JOIN projects project ON project.id = schedule.project_id
        WHERE schedule.id = $1 AND schedule.deleted_at IS NULL
          AND project.deleted_at IS NULL
        "#,
    )
    .bind(schedule_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(false);
    };
    if row.try_get::<&str, _>("privacy")? != "private_local" {
        return Ok(false);
    }
    let user_id: Uuid = row.try_get("owner_user_id")?;
    let payload = serde_json::json!({
        "task_id": row.try_get::<Uuid, _>("task_id")?,
        "project_id": row.try_get::<Uuid, _>("project_id")?,
        "thread_id": row.try_get::<Uuid, _>("thread_id")?,
        "cron": row.try_get::<String, _>("cron_expression")?,
        "timezone": row.try_get::<String, _>("timezone")?,
        "executor_target": row.try_get::<Value, _>("executor_target")?,
        "input": row.try_get::<Value, _>("input")?,
        "model_profile_id": row.try_get::<Option<Uuid>, _>("model_profile_id")?,
        "enabled": row.try_get::<bool, _>("enabled")?,
        "next_run_at": row.try_get::<Option<DateTime<Utc>>, _>("next_run_at")?,
        "last_triggered_at": row.try_get::<Option<DateTime<Utc>>, _>("last_triggered_at")?,
        "blocked_reason": row.try_get::<Option<String>, _>("blocked_reason")?,
        "canonical_revision": row.try_get::<i64, _>("revision")?,
        "created_at": row.try_get::<DateTime<Utc>, _>("created_at")?,
        "updated_at": row.try_get::<DateTime<Utc>, _>("updated_at")?,
        "source": "server"
    });
    publish_server_entity_tx(tx, user_id, "schedule", schedule_id, payload).await?;
    remember_materialization(tx, user_id, "schedule", schedule_id).await?;
    Ok(true)
}

pub(crate) async fn publish_canonical_provider_profile_tx(
    tx: &mut Transaction<'_, Postgres>,
    profile_id: Uuid,
) -> Result<bool, ApiError> {
    let row = sqlx::query("SELECT * FROM provider_profiles WHERE id = $1 AND deleted_at IS NULL")
        .bind(profile_id)
        .fetch_optional(&mut **tx)
        .await?;
    let Some(row) = row else {
        return Ok(false);
    };
    let payload = provider_profile_sync_payload(&row)?;
    if let Some(user_id) = row.try_get::<Option<Uuid>, _>("owner_user_id")? {
        publish_server_entity_tx(tx, user_id, "provider_profile", profile_id, payload).await?;
        remember_materialization(tx, user_id, "provider_profile", profile_id).await?;
    } else if let Some(team_id) = row.try_get::<Option<Uuid>, _>("team_id")? {
        let members = sqlx::query_scalar::<_, Uuid>(
            "SELECT user_id FROM team_members WHERE team_id = $1 ORDER BY user_id",
        )
        .bind(team_id)
        .fetch_all(&mut **tx)
        .await?;
        for user_id in members {
            publish_server_entity_tx(tx, user_id, "provider_profile", profile_id, payload.clone())
                .await?;
        }
    }
    Ok(true)
}

pub(crate) async fn publish_team_provider_profiles_for_user_tx(
    tx: &mut Transaction<'_, Postgres>,
    team_id: Uuid,
    user_id: Uuid,
) -> Result<(), ApiError> {
    let rows = sqlx::query(
        "SELECT * FROM provider_profiles WHERE team_id = $1 AND deleted_at IS NULL ORDER BY id",
    )
    .bind(team_id)
    .fetch_all(&mut **tx)
    .await?;
    for row in rows {
        let profile_id: Uuid = row.try_get("id")?;
        publish_server_entity_tx(
            tx,
            user_id,
            "provider_profile",
            profile_id,
            provider_profile_sync_payload(&row)?,
        )
        .await?;
    }
    Ok(())
}

pub(crate) async fn publish_team_provider_profile_tombstones_for_user_tx(
    tx: &mut Transaction<'_, Postgres>,
    team_id: Uuid,
    user_id: Uuid,
) -> Result<(), ApiError> {
    let profile_ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM provider_profiles WHERE team_id = $1 AND deleted_at IS NULL ORDER BY id",
    )
    .bind(team_id)
    .fetch_all(&mut **tx)
    .await?;
    for profile_id in profile_ids {
        publish_server_tombstone_tx(tx, user_id, "provider_profile", profile_id).await?;
    }
    Ok(())
}

fn provider_profile_sync_payload(row: &PgRow) -> Result<Value, ApiError> {
    let defaults: Value = row.try_get("model_defaults")?;
    let value = |key: &str| defaults.get(key).cloned().unwrap_or(Value::Null);
    Ok(serde_json::json!({
        "name": row.try_get::<String, _>("name")?,
        "provider": "openai-compatible",
        "provider_kind": row.try_get::<String, _>("provider_kind")?,
        "preset": value("preset"),
        "auth_mode": value("auth_mode"),
        "model": value("model"),
        "timeout_ms": value("timeout_ms"),
        "verify_tls_certificates": value("verify_tls_certificates"),
        "context_window": value("context_window"),
        "temperature": value("temperature"),
        "endpoint_binding": value("endpoint_binding"),
        "has_api_key": row.try_get::<Option<Vec<u8>>, _>("encrypted_secret")?.is_some(),
        "canonical_revision": row.try_get::<i64, _>("revision")?,
        "source": "server"
    }))
}

pub(crate) async fn publish_provider_profile_tombstones_tx(
    tx: &mut Transaction<'_, Postgres>,
    profile_id: Uuid,
    owner_user_id: Option<Uuid>,
    team_id: Option<Uuid>,
) -> Result<(), ApiError> {
    if let Some(user_id) = owner_user_id {
        publish_server_tombstone_tx(tx, user_id, "provider_profile", profile_id).await?;
    } else if let Some(team_id) = team_id {
        let members = sqlx::query_scalar::<_, Uuid>(
            "SELECT user_id FROM team_members WHERE team_id = $1 ORDER BY user_id",
        )
        .bind(team_id)
        .fetch_all(&mut **tx)
        .await?;
        for user_id in members {
            publish_server_tombstone_tx(tx, user_id, "provider_profile", profile_id).await?;
        }
    }
    Ok(())
}

async fn publish_server_entity_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    entity_type: &str,
    entity_id: Uuid,
    payload: Value,
) -> Result<(), ApiError> {
    validate_entity_type(entity_type)?;
    let payload_bytes = serde_json::to_vec(&payload)?.len();
    if payload_bytes > MAX_SYNC_PAYLOAD_BYTES {
        return Err(ApiError::Unprocessable(format!(
            "projected sync payload must not exceed {MAX_SYNC_PAYLOAD_BYTES} bytes"
        )));
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("sync:entity:{user_id}:{entity_type}:{entity_id}"))
        .execute(&mut **tx)
        .await?;
    let current = sqlx::query(
        "SELECT revision, payload, tombstone FROM sync_entities WHERE user_id = $1 AND entity_type = $2 AND entity_id = $3 FOR UPDATE",
    )
    .bind(user_id)
    .bind(entity_type)
    .bind(entity_id)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(row) = &current {
        let current_payload: Option<Value> = row.try_get("payload")?;
        if !row.try_get::<bool, _>("tombstone")? && current_payload.as_ref() == Some(&payload) {
            return Ok(());
        }
    }
    let current_revision = current
        .as_ref()
        .map(|row| row.try_get::<i64, _>("revision"))
        .transpose()?
        .unwrap_or(0);
    let current_bytes = current
        .as_ref()
        .and_then(|row| row.try_get::<Option<Value>, _>("payload").ok().flatten())
        .map(|value| serde_json::to_vec(&value).map(|bytes| bytes.len()))
        .transpose()?
        .unwrap_or(0);
    governance::enforce_storage_quota_tx(
        tx,
        "user",
        user_id,
        u64::try_from(payload_bytes.saturating_sub(current_bytes))
            .map_err(|error| ApiError::Internal(error.into()))?,
    )
    .await?;
    let revision = current_revision + 1;
    let etag = format!("W/\"{entity_id}:{revision}\"");
    sqlx::query(
        r#"
        INSERT INTO sync_entities (
            user_id, entity_type, entity_id, revision, etag, payload, tombstone
        ) VALUES ($1, $2, $3, $4, $5, $6, FALSE)
        ON CONFLICT (user_id, entity_type, entity_id) DO UPDATE
        SET revision = EXCLUDED.revision, etag = EXCLUDED.etag,
            payload = EXCLUDED.payload, tombstone = FALSE, updated_at = now()
        "#,
    )
    .bind(user_id)
    .bind(entity_type)
    .bind(entity_id)
    .bind(revision)
    .bind(etag)
    .bind(payload.clone())
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO sync_changes (
            user_id, entity_type, entity_id, revision, operation, payload
        ) VALUES ($1, $2, $3, $4, 'upsert', $5)
        "#,
    )
    .bind(user_id)
    .bind(entity_type)
    .bind(entity_id)
    .bind(revision)
    .bind(payload)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) async fn publish_server_tombstone_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    entity_type: &str,
    entity_id: Uuid,
) -> Result<(), ApiError> {
    validate_entity_type(entity_type)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("sync:entity:{user_id}:{entity_type}:{entity_id}"))
        .execute(&mut **tx)
        .await?;
    let current = sqlx::query(
        "SELECT revision, tombstone FROM sync_entities WHERE user_id = $1 AND entity_type = $2 AND entity_id = $3 FOR UPDATE",
    )
    .bind(user_id)
    .bind(entity_type)
    .bind(entity_id)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(row) = &current {
        if row.try_get::<bool, _>("tombstone")? {
            return Ok(());
        }
    }
    let revision = current
        .as_ref()
        .map(|row| row.try_get::<i64, _>("revision"))
        .transpose()?
        .unwrap_or(0)
        + 1;
    let etag = format!("W/\"{entity_id}:{revision}\"");
    sqlx::query(
        r#"
        INSERT INTO sync_entities (
            user_id, entity_type, entity_id, revision, etag, payload, tombstone
        ) VALUES ($1, $2, $3, $4, $5, NULL, TRUE)
        ON CONFLICT (user_id, entity_type, entity_id) DO UPDATE
        SET revision = EXCLUDED.revision, etag = EXCLUDED.etag,
            payload = NULL, tombstone = TRUE, updated_at = now()
        "#,
    )
    .bind(user_id)
    .bind(entity_type)
    .bind(entity_id)
    .bind(revision)
    .bind(etag)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO sync_changes (
            user_id, entity_type, entity_id, revision, operation, payload
        ) VALUES ($1, $2, $3, $4, 'delete', NULL)
        "#,
    )
    .bind(user_id)
    .bind(entity_type)
    .bind(entity_id)
    .bind(revision)
    .execute(&mut **tx)
    .await?;
    remember_materialization(tx, user_id, entity_type, entity_id).await?;
    Ok(())
}

fn bounded_message_text(content: &Value, maximum_bytes: usize) -> (String, bool) {
    let mut text = match content {
        Value::String(value) => value.clone(),
        Value::Object(object) => ["text", "response", "output", "message"]
            .iter()
            .find_map(|key| object.get(*key).and_then(Value::as_str))
            .map(str::to_owned)
            .unwrap_or_else(|| content.to_string()),
        _ => content.to_string(),
    };
    if text.len() <= maximum_bytes {
        return (text, false);
    }
    let mut boundary = maximum_bytes;
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    text.truncate(boundary);
    (text, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_projected_message_text_on_utf8_boundaries() {
        assert_eq!(
            bounded_message_text(&serde_json::json!({"text": "hello"}), 5),
            ("hello".to_owned(), false)
        );
        assert_eq!(
            bounded_message_text(&Value::String("a€b".to_owned()), 3),
            ("a".to_owned(), true)
        );
    }
}
