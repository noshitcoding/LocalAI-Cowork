//! Authenticated sync HTTP endpoints, device cursors and continuously authorized change streams.

use crate::{
    auth::{ExecutorPrincipal, Principal},
    error::ApiError,
    sync::{
        journal::apply_change,
        records::{row_to_server_change, row_to_synced_entity},
        validation::{validate_change, validate_entity_type, MAX_SYNC_BATCH},
    },
    AppState,
};
use axum::{
    extract::{Extension, Query, State},
    http::HeaderMap,
    response::{
        sse::{Event, KeepAlive},
        Sse,
    },
    Json,
};
use cowork_contracts::{
    PullSyncChangesResponse, PushSyncChangesRequest, PushSyncChangesResponse, SyncedEntityPage,
    SCHEMA_VERSION,
};
use futures_util::StreamExt;
use serde::Deserialize;
use sqlx::PgPool;
use std::{convert::Infallible, time::Duration as StdDuration};
use uuid::Uuid;

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
    let api_replica = state.api_ownership();
    let stream = async_stream::stream! {
        loop {
            // A live SSE connection does not extend its session or central
            // access grant. Match the existing 750-ms run-stream recheck bound.
            let session_valid = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM auth_sessions s JOIN users u ON u.id=s.user_id WHERE s.id=$1 AND s.user_id=$2 AND s.revoked_at IS NULL AND s.expires_at>now() AND u.deleted_at IS NULL)"
            ).bind(principal.session_id).bind(user_id).fetch_one(&pool).await.unwrap_or(false);
            if !session_valid || crate::fleet::validate_access(user_id).await.is_err() { break; }
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
    let stream = stream.take_until(async move {
        match api_replica {
            Some(guard) => guard.wait_until_lost().await,
            None => std::future::pending::<()>().await,
        }
    });
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
