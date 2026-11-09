//! Executor route identity and existing lease checks for storage requests.

use crate::{auth::ExecutorPrincipal, db, error::ApiError};
use axum::http::HeaderMap;
use sqlx::PgPool;
use uuid::Uuid;

pub(super) fn ensure_executor_path(
    principal: &ExecutorPrincipal,
    executor_id: Uuid,
) -> Result<(), ApiError> {
    if principal.executor_id != executor_id {
        return Err(ApiError::Unauthorized(
            "executor credential does not match the route".to_owned(),
        ));
    }
    Ok(())
}

fn executor_lease_token(headers: &HeaderMap) -> Result<Uuid, ApiError> {
    headers
        .get("x-cowork-lease-token")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| ApiError::Unauthorized("missing executor lease token".to_owned()))?
        .parse()
        .map_err(|_| ApiError::Unauthorized("invalid executor lease token".to_owned()))
}

pub(super) async fn verify_executor_lease(
    pool: &PgPool,
    run_id: Uuid,
    executor_id: Uuid,
    headers: &HeaderMap,
) -> Result<cowork_contracts::RunRecord, ApiError> {
    let mut tx = pool.begin().await?;
    let run =
        db::verify_lease(&mut tx, run_id, executor_id, executor_lease_token(headers)?).await?;
    tx.commit().await?;
    Ok(run)
}
