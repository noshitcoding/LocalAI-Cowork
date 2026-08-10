use std::{convert::Infallible, time::Duration as StdDuration};

use axum::{
    extract::{Extension, Query, State},
    http::HeaderMap,
    response::{sse::Event, sse::KeepAlive, Sse},
    Json,
};
use chrono::{DateTime, Duration, Utc};
use cowork_contracts::{
    ensure_compatible, PullSyncChangesResponse, PushSyncChangesRequest, PushSyncChangesResponse,
    ServerSyncChange, SyncApplyResult, SyncApplyStatus, SyncChange, SyncOperation, SyncedEntity,
    SCHEMA_VERSION,
};
use serde::Deserialize;
use serde_json::Value;
use sqlx::{postgres::PgRow, PgPool, Row};
use uuid::Uuid;

use crate::{auth::Principal, error::ApiError, governance, AppState};

const MAX_SYNC_BATCH: usize = 100;
const MAX_SYNC_PAYLOAD_BYTES: usize = 512 * 1024;
const ALLOWED_ENTITY_TYPES: &[&str] = &[
    "project",
    "thread",
    "message",
    "task",
    "schedule",
    "run",
    "crew",
    "skill",
    "memory",
    "provider_profile",
    "secret_metadata",
    "mcp_metadata",
];

#[derive(Debug, Deserialize)]
pub struct PullQuery {
    after: Option<i64>,
    limit: Option<i64>,
}

pub async fn push_changes(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<PushSyncChangesRequest>,
) -> Result<Json<PushSyncChangesResponse>, ApiError> {
    if request.changes.is_empty() || request.changes.len() > MAX_SYNC_BATCH {
        return Err(ApiError::Unprocessable(format!(
            "sync batch must contain 1 to {MAX_SYNC_BATCH} changes"
        )));
    }
    let device_id = session_device_id(&state.pool, &principal).await?;
    for change in &request.changes {
        validate_change(change, device_id)?;
    }
    let mut results = Vec::with_capacity(request.changes.len());
    for change in request.changes {
        results.push(apply_change(&state.pool, principal.user_id, change).await?);
    }
    Ok(Json(PushSyncChangesResponse {
        schema_version: SCHEMA_VERSION,
        results,
    }))
}

pub async fn pull_changes(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<PullQuery>,
) -> Result<Json<PullSyncChangesResponse>, ApiError> {
    let after = query.after.unwrap_or(0);
    if after < 0 {
        return Err(ApiError::Unprocessable(
            "sync cursor must not be negative".to_owned(),
        ));
    }
    let limit = query.limit.unwrap_or(200).clamp(1, 1_000);
    let device_id = session_device_id(&state.pool, &principal).await?;
    let mut tx = state.pool.begin().await?;
    let rows = sqlx::query(
        r#"
        SELECT cursor, entity_type, entity_id, revision, operation, payload, created_at
        FROM sync_changes
        WHERE user_id = $1 AND cursor > $2
        ORDER BY cursor
        LIMIT $3
        "#,
    )
    .bind(principal.user_id)
    .bind(after)
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    let changes = rows
        .iter()
        .map(row_to_server_change)
        .collect::<Result<Vec<_>, _>>()?;
    let next_cursor = changes.last().map(|change| change.cursor).unwrap_or(after);
    let existing_user = sqlx::query_scalar::<_, Uuid>(
        "SELECT user_id FROM sync_device_cursors WHERE device_id = $1 FOR UPDATE",
    )
    .bind(device_id)
    .fetch_optional(&mut *tx)
    .await?;
    if existing_user.is_some_and(|user_id| user_id != principal.user_id) {
        return Err(ApiError::Conflict(
            "device identifier is already registered to another account".to_owned(),
        ));
    }
    sqlx::query(
        r#"
        INSERT INTO sync_device_cursors (device_id, user_id, last_cursor)
        VALUES ($1, $2, $3)
        ON CONFLICT (device_id) DO UPDATE
        SET last_cursor = GREATEST(sync_device_cursors.last_cursor, EXCLUDED.last_cursor),
            updated_at = now()
        "#,
    )
    .bind(device_id)
    .bind(principal.user_id)
    .bind(next_cursor)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(PullSyncChangesResponse {
        schema_version: SCHEMA_VERSION,
        changes,
        next_cursor,
    }))
}

pub async fn change_events(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    session_device_id(&state.pool, &principal).await?;
    let mut cursor = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0)
        .max(0);
    let pool = state.pool.clone();
    let user_id = principal.user_id;
    let stream = async_stream::stream! {
        loop {
            match sqlx::query(
                r#"
                SELECT cursor, entity_type, entity_id, revision, operation, payload, created_at
                FROM sync_changes
                WHERE user_id = $1 AND cursor > $2
                ORDER BY cursor
                LIMIT 250
                "#,
            )
            .bind(user_id)
            .bind(cursor)
            .fetch_all(&pool)
            .await
            {
                Ok(rows) => {
                    for row in rows {
                        match row_to_server_change(&row) {
                            Ok(change) => {
                                cursor = change.cursor;
                                match serde_json::to_string(&change) {
                                    Ok(data) => yield Ok(Event::default()
                                        .id(change.cursor.to_string())
                                        .event("sync_change")
                                        .data(data)),
                                    Err(error) => {
                                        tracing::error!(?error, %user_id, "failed to encode sync SSE event");
                                        break;
                                    }
                                }
                            }
                            Err(error) => {
                                tracing::error!(?error, %user_id, "failed to decode sync SSE row");
                                break;
                            }
                        }
                    }
                }
                Err(error) => tracing::warn!(?error, %user_id, "sync event stream database read failed"),
            }
            tokio::time::sleep(StdDuration::from_millis(750)).await;
        }
    };
    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(StdDuration::from_secs(15))
            .text("keep-alive"),
    ))
}

async fn session_device_id(pool: &PgPool, principal: &Principal) -> Result<Uuid, ApiError> {
    let session_id = principal.session_id.ok_or_else(|| {
        ApiError::Unauthorized("metadata sync requires an authenticated device session".to_owned())
    })?;
    sqlx::query_scalar::<_, Uuid>(
        "SELECT device_id FROM auth_sessions WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL AND expires_at > now()",
    )
    .bind(session_id)
    .bind(principal.user_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::Unauthorized("device session is no longer active".to_owned()))
}

fn validate_change(change: &SyncChange, session_device_id: Uuid) -> Result<(), ApiError> {
    ensure_compatible(change.schema_version)
        .map_err(|error| ApiError::Unprocessable(error.to_string()))?;
    if change.device_id != session_device_id {
        return Err(ApiError::Unauthorized(
            "sync change device_id does not match the authenticated session".to_owned(),
        ));
    }
    if !ALLOWED_ENTITY_TYPES.contains(&change.entity_type.as_str()) {
        return Err(ApiError::Unprocessable(format!(
            "unsupported sync entity type {}",
            change.entity_type
        )));
    }
    if change.base_revision < 0 {
        return Err(ApiError::Unprocessable(
            "base_revision must not be negative".to_owned(),
        ));
    }
    match change.operation {
        SyncOperation::Upsert if change.payload.is_none() => {
            return Err(ApiError::Unprocessable(
                "upsert changes require a payload".to_owned(),
            ))
        }
        SyncOperation::Delete if change.payload.is_some() => {
            return Err(ApiError::Unprocessable(
                "delete changes must not contain a payload".to_owned(),
            ))
        }
        _ => {}
    }
    if change.payload.as_ref().is_some_and(|payload| {
        serde_json::to_vec(payload).is_ok_and(|bytes| bytes.len() > MAX_SYNC_PAYLOAD_BYTES)
    }) {
        return Err(ApiError::Unprocessable(format!(
            "sync payload must not exceed {MAX_SYNC_PAYLOAD_BYTES} bytes"
        )));
    }
    if matches!(
        change.entity_type.as_str(),
        "provider_profile" | "secret_metadata" | "mcp_metadata"
    ) && change
        .payload
        .as_ref()
        .is_some_and(contains_cleartext_secret_field)
    {
        return Err(ApiError::Unprocessable(
            "metadata sync must not contain cleartext secret fields".to_owned(),
        ));
    }
    if change.client_timestamp > Utc::now() + Duration::minutes(5) {
        return Err(ApiError::Unprocessable(
            "client_timestamp is too far in the future".to_owned(),
        ));
    }
    Ok(())
}

fn contains_cleartext_secret_field(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            matches!(
                key.to_ascii_lowercase().as_str(),
                "api_key"
                    | "access_token"
                    | "refresh_token"
                    | "password"
                    | "secret"
                    | "client_secret"
                    | "authorization"
            ) || contains_cleartext_secret_field(value)
        }),
        Value::Array(items) => items.iter().any(contains_cleartext_secret_field),
        _ => false,
    }
}

async fn apply_change(
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

fn row_to_synced_entity(row: &PgRow) -> Result<SyncedEntity, ApiError> {
    Ok(SyncedEntity {
        schema_version: SCHEMA_VERSION,
        entity_type: row.try_get("entity_type")?,
        entity_id: row.try_get("entity_id")?,
        revision: row.try_get("revision")?,
        etag: row.try_get("etag")?,
        payload: row.try_get("payload")?,
        tombstone: row.try_get("tombstone")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_server_change(row: &PgRow) -> Result<ServerSyncChange, ApiError> {
    Ok(ServerSyncChange {
        schema_version: SCHEMA_VERSION,
        cursor: row.try_get("cursor")?,
        entity_type: row.try_get("entity_type")?,
        entity_id: row.try_get("entity_id")?,
        revision: row.try_get("revision")?,
        operation: parse_operation(row.try_get("operation")?)?,
        payload: row.try_get("payload")?,
        created_at: row.try_get("created_at")?,
    })
}

fn operation_name(operation: SyncOperation) -> &'static str {
    match operation {
        SyncOperation::Upsert => "upsert",
        SyncOperation::Delete => "delete",
    }
}

fn parse_operation(operation: &str) -> Result<SyncOperation, ApiError> {
    match operation {
        "upsert" => Ok(SyncOperation::Upsert),
        "delete" => Ok(SyncOperation::Delete),
        other => Err(ApiError::Internal(anyhow::anyhow!(
            "unknown sync operation {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(operation: SyncOperation, payload: Option<Value>) -> SyncChange {
        SyncChange {
            schema_version: SCHEMA_VERSION,
            operation_id: Uuid::new_v4(),
            device_id: Uuid::nil(),
            entity_type: "memory".to_owned(),
            entity_id: Uuid::new_v4(),
            base_revision: 0,
            operation,
            payload,
            client_timestamp: Utc::now(),
        }
    }

    #[test]
    fn validates_payload_and_device_boundaries() {
        assert!(validate_change(
            &change(
                SyncOperation::Upsert,
                Some(serde_json::json!({"text": "ok"}))
            ),
            Uuid::nil()
        )
        .is_ok());
        assert!(validate_change(&change(SyncOperation::Upsert, None), Uuid::nil()).is_err());
        assert!(validate_change(
            &change(SyncOperation::Delete, Some(serde_json::json!({}))),
            Uuid::nil()
        )
        .is_err());
        assert!(validate_change(&change(SyncOperation::Delete, None), Uuid::new_v4()).is_err());
    }

    #[test]
    fn rejects_cleartext_provider_secrets_but_allows_presence_metadata() {
        let mut safe = change(
            SyncOperation::Upsert,
            Some(serde_json::json!({"name": "OpenAI", "has_api_key": true})),
        );
        safe.entity_type = "provider_profile".to_owned();
        assert!(validate_change(&safe, Uuid::nil()).is_ok());
        safe.payload = Some(serde_json::json!({"api_key": "must-not-sync"}));
        assert!(validate_change(&safe, Uuid::nil()).is_err());
    }
}
