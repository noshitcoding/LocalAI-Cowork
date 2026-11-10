//! Instance-local chat workspace identities. Public inputs never select a volume.
use cowork_contracts::{ExecutorTarget, RunSpec};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::error::ApiError;

pub(crate) async fn assign(
    tx: &mut Transaction<'_, Postgres>,
    spec: &RunSpec,
) -> Result<Option<Uuid>, ApiError> {
    use_contract(tx).await?;
    if !matches!(spec.executor_target, ExecutorTarget::ServerLinux { .. }) {
        return Ok(None);
    }
    // Once a chat has a workspace, subsequent tasks keep it even if creation of
    // new workspaces is disabled. Changing an env flag must not hide user files.
    let existing = sqlx::query_scalar::<_, Uuid>(
        "SELECT workspace_id FROM chat_workspaces WHERE thread_id=$1 AND project_id=$2",
    )
    .bind(spec.thread_id)
    .bind(spec.project_id)
    .fetch_optional(&mut **tx)
    .await?;
    if existing.is_some() {
        return Ok(existing);
    }
    if std::env::var("COWORK_CHAT_WORKSPACES_ENABLED").as_deref() != Ok("true") {
        return Ok(None);
    }
    // Serialize first allocation across users/API replicas, after the per-user
    // quota lock. No two chats are allowed to name a shared writable workspace.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("chat-workspace:{}", spec.thread_id))
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "INSERT INTO chat_workspaces(thread_id,project_id,workspace_id) \
         SELECT id,project_id,$3 FROM threads WHERE id=$1 AND project_id=$2 AND deleted_at IS NULL \
         ON CONFLICT(thread_id) DO NOTHING",
    )
    .bind(spec.thread_id)
    .bind(spec.project_id)
    .bind(Uuid::new_v4())
    .execute(&mut **tx)
    .await?;
    let workspace = sqlx::query_scalar::<_, Uuid>(
        "SELECT workspace_id FROM chat_workspaces WHERE thread_id=$1 AND project_id=$2",
    )
    .bind(spec.thread_id)
    .bind(spec.project_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::BadRequest("chat does not belong to this project".to_owned()))?;
    Ok(Some(workspace))
}

pub(crate) async fn use_contract(tx: &mut Transaction<'_, Postgres>) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT set_config('cowork.chat_workspace_version','1',true)")
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub(crate) async fn claimable(
    tx: &mut Transaction<'_, Postgres>,
    run: &RunSpec,
) -> Result<bool, sqlx::Error> {
    let locked: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("chat-workspace:{}", run.thread_id))
            .fetch_one(&mut **tx)
            .await?;
    if !locked {
        return Ok(false);
    }
    // Recheck after locking: concurrent API transactions may commit a run with
    // an earlier creation timestamp after a later one was already dispatched.
    sqlx::query_scalar(
        "SELECT NOT EXISTS(SELECT 1 FROM runs WHERE thread_id=$1 AND id<>$2 \
        AND state IN ('running','waiting_approval','waiting_input','interrupted'))",
    )
    .bind(run.thread_id)
    .bind(run.id)
    .fetch_one(&mut **tx)
    .await
}

pub(crate) async fn for_run(pool: &PgPool, run: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar("SELECT sandbox_workspace_id FROM runs WHERE id=$1")
        .bind(run)
        .fetch_one(pool)
        .await
}

pub(crate) fn jobs_path(workspace: Option<Uuid>) -> &'static str {
    if workspace.is_some() {
        "/v2/jobs"
    } else {
        "/v1/jobs"
    }
}

pub(crate) fn run_path(run: Uuid, resource: &str, query: &str, workspace: Option<Uuid>) -> String {
    match workspace {
        Some(id) => format!("/v2/runs/{run}/{resource}?{query}&workspace_id={id}"),
        None => format!("/v1/runs/{run}/{resource}?{query}"),
    }
}
