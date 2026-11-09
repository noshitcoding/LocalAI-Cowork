//! Project version creation, ancestry validation and application.

use crate::{auth::Principal, error::ApiError, organization, AppState};
use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    Json,
};
use cowork_contracts::{ApplyProjectVersionRequest, ProjectRole, ProjectVersion, SCHEMA_VERSION};
use serde_json::json;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

pub async fn create_project_version(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<cowork_contracts::CreateProjectVersionRequest>,
) -> Result<(StatusCode, Json<ProjectVersion>), ApiError> {
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Editor,
    )
    .await?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT id FROM projects WHERE id = $1 AND deleted_at IS NULL FOR UPDATE")
        .bind(project_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("project {project_id} was not found")))?;
    let snapshot_ready = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM snapshot_manifests WHERE id = $1 AND project_id = $2 AND status = 'ready')",
    )
    .bind(request.snapshot_manifest_id)
    .bind(project_id)
    .fetch_one(&mut *tx)
    .await?;
    if !snapshot_ready {
        return Err(ApiError::Unprocessable(
            "project versions require a ready snapshot from the same project".to_owned(),
        ));
    }
    validate_version_parent(&mut tx, project_id, request.parent_version_id).await?;
    validate_version_parent(&mut tx, project_id, request.merge_base_version_id).await?;
    if let Some(run_id) = request.created_by_run_id {
        let valid_run = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE id = $1 AND project_id = $2 AND state = 'completed')",
        )
        .bind(run_id)
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;
        if !valid_run {
            return Err(ApiError::Unprocessable(
                "created_by_run_id must be a completed run from this project".to_owned(),
            ));
        }
    }
    let revision = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(revision), 0) + 1 FROM project_versions WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_one(&mut *tx)
    .await?;
    let id = Uuid::new_v4();
    let row = sqlx::query(
        r#"
        INSERT INTO project_versions (
            id, project_id, revision, parent_version_id, merge_base_version_id,
            snapshot_manifest_id, created_by_user_id, created_by_run_id,
            diff_summary
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(project_id)
    .bind(revision)
    .bind(request.parent_version_id)
    .bind(request.merge_base_version_id)
    .bind(request.snapshot_manifest_id)
    .bind(principal.user_id)
    .bind(request.created_by_run_id)
    .bind(request.diff_summary)
    .fetch_one(&mut *tx)
    .await?;
    let version = row_to_project_version(&row)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(version)))
}

pub async fn list_project_versions(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<ProjectVersion>>, ApiError> {
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Viewer,
    )
    .await?;
    let rows =
        sqlx::query("SELECT * FROM project_versions WHERE project_id = $1 ORDER BY revision DESC")
            .bind(project_id)
            .fetch_all(&state.pool)
            .await?;
    rows.iter()
        .map(row_to_project_version)
        .collect::<Result<_, _>>()
        .map(Json)
}

pub async fn apply_project_version(
    State(state): State<AppState>,
    Path((project_id, version_id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<ApplyProjectVersionRequest>,
) -> Result<Json<ProjectVersion>, ApiError> {
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Editor,
    )
    .await?;
    let mut tx = state.pool.begin().await?;
    let project = sqlx::query(
        "SELECT revision, current_version_id FROM projects WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(project_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("project {project_id} was not found")))?;
    let revision: i64 = project.try_get("revision")?;
    let current: Option<Uuid> = project.try_get("current_version_id")?;
    if revision != request.expected_project_revision
        || current != request.expected_current_version_id
    {
        return Err(ApiError::Conflict(
            "project changed since the review started".to_owned(),
        ));
    }
    let version = sqlx::query("SELECT * FROM project_versions WHERE id = $1 AND project_id = $2")
        .bind(version_id)
        .bind(project_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("project version {version_id} was not found")))?;
    let next_revision = revision + 1;
    sqlx::query(
        "UPDATE projects SET current_version_id = $2, revision = $3, etag = $4, updated_at = now() WHERE id = $1",
    )
    .bind(project_id)
    .bind(version_id)
    .bind(next_revision)
    .bind(format!("W/\"{project_id}:{next_revision}\""))
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO audit_events (id, actor_user_id, action, target_type, target_id, metadata) VALUES ($1, $2, 'project.version.apply', 'project', $3, $4)",
    )
    .bind(Uuid::new_v4())
    .bind(principal.user_id)
    .bind(project_id)
    .bind(json!({"version_id": version_id, "previous_version_id": current}))
    .execute(&mut *tx)
    .await?;
    let record = row_to_project_version(&version)?;
    tx.commit().await?;
    Ok(Json(record))
}

async fn validate_version_parent(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    version_id: Option<Uuid>,
) -> Result<(), ApiError> {
    if let Some(version_id) = version_id {
        let valid = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM project_versions WHERE id = $1 AND project_id = $2)",
        )
        .bind(version_id)
        .bind(project_id)
        .fetch_one(&mut **tx)
        .await?;
        if !valid {
            return Err(ApiError::Unprocessable(format!(
                "version {version_id} does not belong to this project"
            )));
        }
    }
    Ok(())
}

pub(super) fn row_to_project_version(
    row: &sqlx::postgres::PgRow,
) -> Result<ProjectVersion, ApiError> {
    Ok(ProjectVersion {
        schema_version: SCHEMA_VERSION,
        id: row.try_get("id")?,
        project_id: row.try_get("project_id")?,
        revision: row.try_get("revision")?,
        parent_version_id: row.try_get("parent_version_id")?,
        merge_base_version_id: row.try_get("merge_base_version_id")?,
        snapshot_manifest_id: row.try_get("snapshot_manifest_id")?,
        created_by_run_id: row.try_get("created_by_run_id")?,
        created_at: row.try_get("created_at")?,
    })
}
