//! Transactional sync operation replay, optimistic revisions and durable change journaling.

use crate::{
    error::ApiError,
    governance,
    sync::{
        materialization::materialize_canonical_entity,
        records::{operation_name, row_to_synced_entity},
    },
};
use chrono::{DateTime, Utc};
use cowork_contracts::{
    SyncApplyResult, SyncApplyStatus, SyncChange, SyncOperation, SCHEMA_VERSION,
};
use serde_json::Value;
use sqlx::{postgres::PgRow, PgPool, Row};
use uuid::Uuid;

pub(in crate::sync) async fn apply_change(
    pool: &PgPool,
    user_id: Uuid,
    change: SyncChange,
) -> Result<SyncApplyResult, ApiError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("sync:operation:{}", change.operation_id))
        .execute(&mut *tx)
        .await?;
    if let Some(row) = sqlx::query("SELECT * FROM sync_operations WHERE operation_id = $1")
        .bind(change.operation_id)
        .fetch_optional(&mut *tx)
        .await?
    {
        ensure_replayed_operation_matches(&row, user_id, &change)?;
        let result = serde_json::from_value(row.try_get("result")?)?;
        tx.commit().await?;
        return Ok(result);
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "sync:entity:{user_id}:{}:{}",
            change.entity_type, change.entity_id
        ))
        .execute(&mut *tx)
        .await?;
    let current = sqlx::query(
        "SELECT * FROM sync_entities WHERE user_id = $1 AND entity_type = $2 AND entity_id = $3 FOR UPDATE",
    )
    .bind(user_id)
    .bind(&change.entity_type)
    .bind(change.entity_id)
    .fetch_optional(&mut *tx)
    .await?;
    let current_revision = current
        .as_ref()
        .map(|row| row.try_get::<i64, _>("revision"))
        .transpose()?
        .unwrap_or(0);
    let result = if current_revision != change.base_revision {
        SyncApplyResult {
            schema_version: SCHEMA_VERSION,
            operation_id: change.operation_id,
            status: SyncApplyStatus::Conflict,
            entity: current.as_ref().map(row_to_synced_entity).transpose()?,
        }
    } else {
        let revision = current_revision + 1;
        let etag = format!("W/\"{}:{revision}\"", change.entity_id);
        let tombstone = change.operation == SyncOperation::Delete;
        let current_bytes = current
            .as_ref()
            .and_then(|row| row.try_get::<Option<Value>, _>("payload").ok().flatten())
            .map(|payload| serde_json::to_vec(&payload).map(|bytes| bytes.len()))
            .transpose()?
            .unwrap_or(0);
        let next_bytes = change
            .payload
            .as_ref()
            .map(|payload| serde_json::to_vec(payload).map(|bytes| bytes.len()))
            .transpose()?
            .unwrap_or(0);
        governance::enforce_storage_quota_tx(
            &mut tx,
            "user",
            user_id,
            u64::try_from(next_bytes.saturating_sub(current_bytes))
                .map_err(|error| ApiError::Internal(error.into()))?,
        )
        .await?;
        let row = sqlx::query(
            r#"
            INSERT INTO sync_entities (
                user_id, entity_type, entity_id, revision, etag, payload, tombstone
            ) VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (user_id, entity_type, entity_id) DO UPDATE
            SET revision = EXCLUDED.revision, etag = EXCLUDED.etag,
                payload = EXCLUDED.payload, tombstone = EXCLUDED.tombstone,
                updated_at = now()
            RETURNING *
            "#,
        )
        .bind(user_id)
        .bind(&change.entity_type)
        .bind(change.entity_id)
        .bind(revision)
        .bind(etag)
        .bind(change.payload.clone())
        .bind(tombstone)
        .fetch_one(&mut *tx)
        .await?;
        materialize_canonical_entity(
            &mut tx,
            user_id,
            &change.entity_type,
            change.entity_id,
            change.operation,
            change.payload.as_ref(),
        )
        .await?;
        sqlx::query(
            r#"
            INSERT INTO sync_changes (
                user_id, entity_type, entity_id, revision, operation, payload
            ) VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(user_id)
        .bind(&change.entity_type)
        .bind(change.entity_id)
        .bind(revision)
        .bind(operation_name(change.operation))
        .bind(change.payload.clone())
        .execute(&mut *tx)
        .await?;
        SyncApplyResult {
            schema_version: SCHEMA_VERSION,
            operation_id: change.operation_id,
            status: SyncApplyStatus::Applied,
            entity: Some(row_to_synced_entity(&row)?),
        }
    };
    sqlx::query(
        r#"
        INSERT INTO sync_operations (
            operation_id, device_id, user_id, entity_type, entity_id,
            base_revision, operation, payload, client_timestamp,
            server_revision, result
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        "#,
    )
    .bind(change.operation_id)
    .bind(change.device_id)
    .bind(user_id)
    .bind(&change.entity_type)
    .bind(change.entity_id)
    .bind(change.base_revision)
    .bind(operation_name(change.operation))
    .bind(change.payload)
    .bind(change.client_timestamp)
    .bind(result.entity.as_ref().map(|entity| entity.revision))
    .bind(serde_json::to_value(&result)?)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(result)
}

fn ensure_replayed_operation_matches(
    row: &PgRow,
    user_id: Uuid,
    change: &SyncChange,
) -> Result<(), ApiError> {
    let matches = row.try_get::<Uuid, _>("user_id")? == user_id
        && row.try_get::<Uuid, _>("device_id")? == change.device_id
        && row.try_get::<&str, _>("entity_type")? == change.entity_type
        && row.try_get::<Uuid, _>("entity_id")? == change.entity_id
        && row.try_get::<i64, _>("base_revision")? == change.base_revision
        && row.try_get::<&str, _>("operation")? == operation_name(change.operation)
        && row.try_get::<Option<Value>, _>("payload")? == change.payload
        && row.try_get::<DateTime<Utc>, _>("client_timestamp")? == change.client_timestamp;
    if matches {
        Ok(())
    } else {
        Err(ApiError::Conflict(
            "operation_id was already used for a different sync change".to_owned(),
        ))
    }
}
