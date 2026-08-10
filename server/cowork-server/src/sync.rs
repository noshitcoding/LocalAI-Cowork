use std::{convert::Infallible, time::Duration as StdDuration};

use axum::{
    extract::{Extension, Query, State},
    http::HeaderMap,
    response::{sse::Event, sse::KeepAlive, Sse},
    Json,
};
use chrono::{DateTime, Duration, Utc};
use cowork_contracts::{
    ensure_compatible, ExecutorTarget, PullSyncChangesResponse, PushSyncChangesRequest,
    PushSyncChangesResponse, ServerSyncChange, SyncApplyResult, SyncApplyStatus, SyncChange,
    SyncOperation, SyncedEntity, SyncedEntityPage, SCHEMA_VERSION,
};
use serde::Deserialize;
use serde_json::Value;
use sqlx::{postgres::PgRow, PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{
    auth::{ExecutorPrincipal, Principal},
    error::ApiError,
    governance, AppState,
};

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

#[derive(Debug, Deserialize)]
pub struct EntityPageQuery {
    after: Option<Uuid>,
    limit: Option<i64>,
}

pub async fn push_changes(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<PushSyncChangesRequest>,
) -> Result<Json<PushSyncChangesResponse>, ApiError> {
    let device_id = session_device_id(&state.pool, &principal).await?;
    Ok(Json(
        push_changes_for_device(&state.pool, principal.user_id, device_id, request).await?,
    ))
}

pub async fn agent_push_changes(
    State(state): State<AppState>,
    axum::extract::Path(executor_id): axum::extract::Path<Uuid>,
    Extension(principal): Extension<ExecutorPrincipal>,
    Json(request): Json<PushSyncChangesRequest>,
) -> Result<Json<PushSyncChangesResponse>, ApiError> {
    let user_id = personal_executor_owner(&state.pool, executor_id, &principal).await?;
    Ok(Json(
        push_changes_for_device(&state.pool, user_id, executor_id, request).await?,
    ))
}

async fn push_changes_for_device(
    pool: &PgPool,
    user_id: Uuid,
    device_id: Uuid,
    request: PushSyncChangesRequest,
) -> Result<PushSyncChangesResponse, ApiError> {
    if request.changes.is_empty() || request.changes.len() > MAX_SYNC_BATCH {
        return Err(ApiError::Unprocessable(format!(
            "sync batch must contain 1 to {MAX_SYNC_BATCH} changes"
        )));
    }
    for change in &request.changes {
        validate_change(change, device_id)?;
    }
    let mut results = Vec::with_capacity(request.changes.len());
    for change in request.changes {
        results.push(apply_change(pool, user_id, change).await?);
    }
    Ok(PushSyncChangesResponse {
        schema_version: SCHEMA_VERSION,
        results,
    })
}

pub async fn pull_changes(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<PullQuery>,
) -> Result<Json<PullSyncChangesResponse>, ApiError> {
    let device_id = session_device_id(&state.pool, &principal).await?;
    Ok(Json(
        pull_changes_for_device(&state.pool, principal.user_id, device_id, query).await?,
    ))
}

pub async fn agent_pull_changes(
    State(state): State<AppState>,
    axum::extract::Path(executor_id): axum::extract::Path<Uuid>,
    Extension(principal): Extension<ExecutorPrincipal>,
    Query(query): Query<PullQuery>,
) -> Result<Json<PullSyncChangesResponse>, ApiError> {
    let user_id = personal_executor_owner(&state.pool, executor_id, &principal).await?;
    Ok(Json(
        pull_changes_for_device(&state.pool, user_id, executor_id, query).await?,
    ))
}

async fn pull_changes_for_device(
    pool: &PgPool,
    user_id: Uuid,
    device_id: Uuid,
    query: PullQuery,
) -> Result<PullSyncChangesResponse, ApiError> {
    let after = query.after.unwrap_or(0);
    if after < 0 {
        return Err(ApiError::Unprocessable(
            "sync cursor must not be negative".to_owned(),
        ));
    }
    let limit = query.limit.unwrap_or(200).clamp(1, 1_000);
    let mut tx = pool.begin().await?;
    let rows = sqlx::query(
        r#"
        SELECT cursor, entity_type, entity_id, revision, operation, payload, created_at
        FROM sync_changes
        WHERE user_id = $1 AND cursor > $2
        ORDER BY cursor
        LIMIT $3
        "#,
    )
    .bind(user_id)
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
    if existing_user.is_some_and(|existing| existing != user_id) {
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
    .bind(user_id)
    .bind(next_cursor)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(PullSyncChangesResponse {
        schema_version: SCHEMA_VERSION,
        changes,
        next_cursor,
    })
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

pub async fn list_entities(
    State(state): State<AppState>,
    axum::extract::Path(entity_type): axum::extract::Path<String>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<EntityPageQuery>,
) -> Result<Json<SyncedEntityPage>, ApiError> {
    session_device_id(&state.pool, &principal).await?;
    Ok(Json(
        list_entities_for_user(&state.pool, principal.user_id, entity_type, query).await?,
    ))
}

pub async fn agent_list_entities(
    State(state): State<AppState>,
    axum::extract::Path((executor_id, entity_type)): axum::extract::Path<(Uuid, String)>,
    Extension(principal): Extension<ExecutorPrincipal>,
    Query(query): Query<EntityPageQuery>,
) -> Result<Json<SyncedEntityPage>, ApiError> {
    let user_id = personal_executor_owner(&state.pool, executor_id, &principal).await?;
    Ok(Json(
        list_entities_for_user(&state.pool, user_id, entity_type, query).await?,
    ))
}

async fn list_entities_for_user(
    pool: &PgPool,
    user_id: Uuid,
    entity_type: String,
    query: EntityPageQuery,
) -> Result<SyncedEntityPage, ApiError> {
    validate_entity_type(&entity_type)?;
    let limit = query.limit.unwrap_or(200).clamp(1, 1_000);
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await?;
    let rows = sqlx::query(
        r#"
        SELECT * FROM sync_entities
        WHERE user_id = $1 AND entity_type = $2
          AND ($3::uuid IS NULL OR entity_id > $3)
        ORDER BY entity_id
        LIMIT $4
        "#,
    )
    .bind(user_id)
    .bind(&entity_type)
    .bind(query.after)
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    let items = rows
        .iter()
        .map(row_to_synced_entity)
        .collect::<Result<Vec<_>, _>>()?;
    let watermark_cursor = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(max(cursor), 0)::bigint FROM sync_changes WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    let next_after = (items.len() == usize::try_from(limit).unwrap_or(usize::MAX))
        .then(|| items.last().map(|item| item.entity_id))
        .flatten();
    Ok(SyncedEntityPage {
        schema_version: SCHEMA_VERSION,
        items,
        next_after,
        watermark_cursor,
    })
}

async fn personal_executor_owner(
    pool: &PgPool,
    executor_id: Uuid,
    principal: &ExecutorPrincipal,
) -> Result<Uuid, ApiError> {
    if principal.executor_id != executor_id {
        return Err(ApiError::Unauthorized(
            "executor credential does not match the requested executor".to_owned(),
        ));
    }
    sqlx::query_scalar::<_, Uuid>(
        "SELECT owner_user_id FROM executors WHERE id = $1 AND kind = 'personal_device' AND owner_user_id IS NOT NULL",
    )
    .bind(executor_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| {
        ApiError::Unauthorized(
            "metadata sync is only available to owner-bound personal executors".to_owned(),
        )
    })
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
    validate_entity_type(&change.entity_type)?;
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

fn validate_entity_type(entity_type: &str) -> Result<(), ApiError> {
    if ALLOWED_ENTITY_TYPES.contains(&entity_type) {
        Ok(())
    } else {
        Err(ApiError::Unprocessable(format!(
            "unsupported sync entity type {entity_type}"
        )))
    }
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

async fn materialize_canonical_entity(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    entity_type: &str,
    entity_id: Uuid,
    operation: SyncOperation,
    payload: Option<&Value>,
) -> Result<(), ApiError> {
    match entity_type {
        "project" => {
            materialize_project(tx, user_id, entity_id, operation, payload).await?;
        }
        "thread" => {
            materialize_thread(tx, user_id, entity_id, operation, payload).await?;
        }
        "message" => {
            materialize_message(tx, user_id, entity_id, operation, payload).await?;
        }
        _ => {}
    }
    Ok(())
}

async fn is_materialized(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    entity_type: &str,
    entity_id: Uuid,
) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM sync_materializations
            WHERE user_id = $1 AND entity_type = $2 AND entity_id = $3
        )
        "#,
    )
    .bind(user_id)
    .bind(entity_type)
    .bind(entity_id)
    .fetch_one(&mut **tx)
    .await?)
}

async fn remember_materialization(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    entity_type: &str,
    entity_id: Uuid,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO sync_materializations (user_id, entity_type, entity_id)
        VALUES ($1, $2, $3)
        ON CONFLICT (user_id, entity_type, entity_id)
        DO UPDATE SET updated_at = now()
        "#,
    )
    .bind(user_id)
    .bind(entity_type)
    .bind(entity_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn object_payload<'a>(
    payload: Option<&'a Value>,
    entity_type: &str,
) -> Result<&'a serde_json::Map<String, Value>, ApiError> {
    payload.and_then(Value::as_object).ok_or_else(|| {
        ApiError::Unprocessable(format!("{entity_type} sync payload must be a JSON object"))
    })
}

fn required_text(
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
    label: &str,
    maximum: usize,
) -> Result<String, ApiError> {
    let value = keys
        .iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
        .unwrap_or_default()
        .trim();
    if value.is_empty() || value.len() > maximum {
        return Err(ApiError::Unprocessable(format!(
            "{label} must contain 1 to {maximum} characters"
        )));
    }
    Ok(value.to_owned())
}

fn optional_text(
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
    maximum: usize,
) -> Result<String, ApiError> {
    let value = keys
        .iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
        .unwrap_or_default();
    if value.len() > maximum {
        return Err(ApiError::Unprocessable(format!(
            "metadata text must not exceed {maximum} characters"
        )));
    }
    Ok(value.to_owned())
}

async fn materialize_project(
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
            reconcile_project_threads(tx, user_id, project_id).await?;
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

async fn soft_delete_thread(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    thread_id: Uuid,
) -> Result<(), ApiError> {
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

async fn materialize_thread(
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
            soft_delete_thread(tx, user_id, thread_id).await?;
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
    Ok(())
}

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

async fn materialize_message(
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
