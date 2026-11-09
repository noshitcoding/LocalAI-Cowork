//! Dispatches canonical entity projections and records their sync ownership.

mod messages;
mod projects;
mod providers;
mod schedules;
mod tasks;
mod threads;

use crate::{
    error::ApiError,
    sync::materialization::{
        messages::materialize_message, projects::materialize_project,
        providers::materialize_provider_profile, schedules::materialize_schedule,
        tasks::materialize_task, threads::materialize_thread,
    },
};
use cowork_contracts::SyncOperation;
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

pub(in crate::sync) async fn materialize_canonical_entity(
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
        "task" => {
            materialize_task(tx, user_id, entity_id, operation, payload).await?;
        }
        "schedule" => {
            materialize_schedule(tx, user_id, entity_id, operation, payload).await?;
        }
        "provider_profile" => {
            materialize_provider_profile(tx, user_id, entity_id, operation, payload).await?;
        }
        _ => {}
    }
    Ok(())
}

pub(in crate::sync) async fn is_materialized(
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

pub(in crate::sync) async fn remember_materialization(
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
