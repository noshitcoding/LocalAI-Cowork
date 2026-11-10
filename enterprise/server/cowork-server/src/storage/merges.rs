//! Project merge review, file analysis and resolved snapshot materialization.

use crate::{
    auth::Principal,
    error::ApiError,
    organization,
    storage::{
        object_store::{require_store, ObjectStore},
        snapshot_repository::{
            create_ready_manifest, row_scope, snapshot_from_row, snapshot_to_upload_file,
            upload_file_from_bytes,
        },
        versions::row_to_project_version,
    },
    AppState,
};
use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    Json,
};
use cowork_contracts::{
    ApplyProjectMergeRequest, MergeFileReview, MergeFileStatus, MergeResolutionChoice,
    ProjectMergeReview, ProjectRole, ProjectVersion, SnapshotFile, SCHEMA_VERSION,
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

const MAX_TEXT_MERGE_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct MergeReviewQuery {
    base_version_id: Uuid,
    current_version_id: Uuid,
    result_version_id: Uuid,
}

#[derive(Clone)]
struct VersionFiles {
    version: ProjectVersion,
    manifest_id: Uuid,
    files: HashMap<String, SnapshotFile>,
}

struct FileAnalysis {
    review: MergeFileReview,
    auto_merged: Option<Vec<u8>>,
}

pub async fn review_project_merge(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<MergeReviewQuery>,
) -> Result<Json<ProjectMergeReview>, ApiError> {
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Viewer,
    )
    .await?;
    let store = require_store(&state)?;
    let (base, current, result) = load_merge_versions(
        &state.pool,
        project_id,
        query.base_version_id,
        query.current_version_id,
        query.result_version_id,
    )
    .await?;
    let analysis = analyze_merge(&state.pool, &store, &base, &current, &result).await?;
    Ok(Json(ProjectMergeReview {
        schema_version: SCHEMA_VERSION,
        project_id,
        base_version_id: base.version.id,
        current_version_id: current.version.id,
        result_version_id: result.version.id,
        files: analysis.into_iter().map(|item| item.review).collect(),
    }))
}

pub async fn apply_project_merge(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<ApplyProjectMergeRequest>,
) -> Result<(StatusCode, Json<ProjectVersion>), ApiError> {
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Editor,
    )
    .await?;
    let store = require_store(&state)?;
    let (base, current, result) = load_merge_versions(
        &state.pool,
        project_id,
        request.base_version_id,
        request.current_version_id,
        request.result_version_id,
    )
    .await?;
    let analysis = analyze_merge(&state.pool, &store, &base, &current, &result).await?;
    let valid_paths: HashSet<&str> = analysis
        .iter()
        .map(|item| item.review.path.as_str())
        .collect();
    let mut resolutions = HashMap::new();
    for resolution in request.resolutions {
        if !valid_paths.contains(resolution.path.as_str()) {
            return Err(ApiError::Unprocessable(format!(
                "merge resolution references unknown path {}",
                resolution.path
            )));
        }
        if resolutions
            .insert(resolution.path.clone(), resolution.choice)
            .is_some()
        {
            return Err(ApiError::Unprocessable(format!(
                "merge resolution for {} is duplicated",
                resolution.path
            )));
        }
    }
    let scope_row = sqlx::query("SELECT * FROM snapshot_manifests WHERE id = $1")
        .bind(current.manifest_id)
        .fetch_one(&state.pool)
        .await?;
    let scope = row_scope(&scope_row)?;
    let mut output = Vec::new();
    for item in &analysis {
        let path = &item.review.path;
        let explicit = resolutions.get(path).copied();
        let choice = explicit.unwrap_or_else(|| default_resolution(item.review.status));
        let selected = match choice {
            MergeResolutionChoice::Current => current.files.get(path).cloned(),
            MergeResolutionChoice::Result => result.files.get(path).cloned(),
            MergeResolutionChoice::Delete => None,
            MergeResolutionChoice::AutoMerged => {
                let bytes = item.auto_merged.as_ref().ok_or_else(|| {
                    ApiError::Unprocessable(format!("{path} has no conflict-free automatic merge"))
                })?;
                let template = result
                    .files
                    .get(path)
                    .or_else(|| current.files.get(path))
                    .ok_or_else(|| {
                        ApiError::Internal(anyhow::anyhow!("merge template file disappeared"))
                    })?;
                Some(upload_file_from_bytes(&state.pool, &store, scope, template, bytes).await?)
            }
        };
        if matches!(
            item.review.status,
            MergeFileStatus::TextConflict | MergeFileStatus::BinaryConflict
        ) && explicit.is_none()
        {
            return Err(ApiError::Unprocessable(format!(
                "conflict for {path} requires an explicit resolution"
            )));
        }
        if let Some(file) = selected {
            output.push(snapshot_to_upload_file(file));
        }
    }
    output.sort_by(|left, right| left.path.cmp(&right.path));
    let merged_manifest_id =
        create_ready_manifest(&state.pool, project_id, principal.user_id, scope, &output).await?;

    let mut tx = state.pool.begin().await?;
    let project = sqlx::query(
        "SELECT revision, current_version_id FROM projects WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(project_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("project {project_id} was not found")))?;
    let project_revision: i64 = project.try_get("revision")?;
    let current_version_id: Option<Uuid> = project.try_get("current_version_id")?;
    if project_revision != request.expected_project_revision
        || current_version_id != Some(current.version.id)
    {
        return Err(ApiError::Conflict(
            "project changed since the merge review was generated".to_owned(),
        ));
    }
    let version_revision = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(revision), 0) + 1 FROM project_versions WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_one(&mut *tx)
    .await?;
    let version_id = Uuid::new_v4();
    let review_summary = json!({
        "base_version_id": base.version.id,
        "current_version_id": current.version.id,
        "result_version_id": result.version.id,
        "files": analysis.iter().map(|item| &item.review).collect::<Vec<_>>()
    });
    let version_row = sqlx::query(
        r#"
        INSERT INTO project_versions (
            id, project_id, revision, parent_version_id, merge_base_version_id,
            snapshot_manifest_id, created_by_user_id, diff_summary
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING *
        "#,
    )
    .bind(version_id)
    .bind(project_id)
    .bind(version_revision)
    .bind(current.version.id)
    .bind(base.version.id)
    .bind(merged_manifest_id)
    .bind(principal.user_id)
    .bind(review_summary)
    .fetch_one(&mut *tx)
    .await?;
    let next_project_revision = project_revision + 1;
    sqlx::query(
        "UPDATE projects SET current_version_id = $2, revision = $3, etag = $4, updated_at = now() WHERE id = $1",
    )
    .bind(project_id)
    .bind(version_id)
    .bind(next_project_revision)
    .bind(format!("W/\"{project_id}:{next_project_revision}\""))
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO audit_events (id, actor_user_id, action, target_type, target_id, metadata) VALUES ($1, $2, 'project.merge.apply', 'project', $3, $4)",
    )
    .bind(Uuid::new_v4())
    .bind(principal.user_id)
    .bind(project_id)
    .bind(json!({"version_id": version_id, "result_version_id": result.version.id}))
    .execute(&mut *tx)
    .await?;
    let version = row_to_project_version(&version_row)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(version)))
}

async fn load_merge_versions(
    pool: &PgPool,
    project_id: Uuid,
    base_id: Uuid,
    current_id: Uuid,
    result_id: Uuid,
) -> Result<(VersionFiles, VersionFiles, VersionFiles), ApiError> {
    let project_current = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT current_version_id FROM projects WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await?
    .flatten()
    .ok_or_else(|| ApiError::Conflict("project has no current version".to_owned()))?;
    if project_current != current_id {
        return Err(ApiError::Conflict(
            "current_version_id is no longer the project's current version".to_owned(),
        ));
    }
    let base = load_version_files(pool, project_id, base_id).await?;
    let current = load_version_files(pool, project_id, current_id).await?;
    let result = load_version_files(pool, project_id, result_id).await?;
    Ok((base, current, result))
}

async fn load_version_files(
    pool: &PgPool,
    project_id: Uuid,
    version_id: Uuid,
) -> Result<VersionFiles, ApiError> {
    let row = sqlx::query("SELECT * FROM project_versions WHERE id = $1 AND project_id = $2")
        .bind(version_id)
        .bind(project_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("project version {version_id} was not found")))?;
    let version = row_to_project_version(&row)?;
    let manifest_row =
        sqlx::query("SELECT * FROM snapshot_manifests WHERE id = $1 AND status = 'ready'")
            .bind(version.snapshot_manifest_id)
            .fetch_optional(pool)
            .await?
            .ok_or_else(|| {
                ApiError::Conflict(format!("version {version_id} snapshot is not ready"))
            })?;
    let manifest = snapshot_from_row(pool, &manifest_row).await?;
    Ok(VersionFiles {
        version,
        manifest_id: manifest.id,
        files: manifest
            .files
            .into_iter()
            .map(|file| (file.path.clone(), file))
            .collect(),
    })
}

async fn analyze_merge(
    pool: &PgPool,
    store: &ObjectStore,
    base: &VersionFiles,
    current: &VersionFiles,
    result: &VersionFiles,
) -> Result<Vec<FileAnalysis>, ApiError> {
    let mut paths: HashSet<String> = base.files.keys().cloned().collect();
    paths.extend(current.files.keys().cloned());
    paths.extend(result.files.keys().cloned());
    let mut paths: Vec<String> = paths.into_iter().collect();
    paths.sort();
    let mut analysis = Vec::with_capacity(paths.len());
    for path in paths {
        let base_file = base.files.get(&path);
        let current_file = current.files.get(&path);
        let result_file = result.files.get(&path);
        let base_digest = base_file.map(file_fingerprint);
        let current_digest = current_file.map(file_fingerprint);
        let result_digest = result_file.map(file_fingerprint);
        let mut auto_merged = None;
        let mut conflict_preview = None;
        let status = match (
            base_digest.as_ref(),
            current_digest.as_ref(),
            result_digest.as_ref(),
        ) {
            (None, None, Some(_)) => MergeFileStatus::Added,
            (None, Some(_), None) => MergeFileStatus::CurrentOnly,
            (None, Some(current), Some(result)) if current == result => {
                MergeFileStatus::IdenticalChange
            }
            (None, Some(_), Some(_)) => MergeFileStatus::BinaryConflict,
            (Some(base), Some(current), Some(result)) if base == current && base == result => {
                MergeFileStatus::Unchanged
            }
            (Some(_), None, None) => MergeFileStatus::Deleted,
            (Some(base), Some(current), None) if base == current => MergeFileStatus::Deleted,
            (Some(_), Some(_), None) => MergeFileStatus::BinaryConflict,
            (Some(base), None, Some(result)) if base == result => MergeFileStatus::CurrentOnly,
            (Some(_), None, Some(_)) => MergeFileStatus::BinaryConflict,
            (Some(base), Some(current), Some(_)) if base == current => MergeFileStatus::ResultOnly,
            (Some(base), Some(_), Some(result)) if base == result => MergeFileStatus::CurrentOnly,
            (Some(_), Some(current), Some(result)) if current == result => {
                MergeFileStatus::IdenticalChange
            }
            (Some(_), Some(_), Some(_)) => {
                match attempt_text_merge(
                    pool,
                    store,
                    base.manifest_id,
                    current.manifest_id,
                    result.manifest_id,
                    &path,
                    base_file.expect("matched Some"),
                    current_file.expect("matched Some"),
                    result_file.expect("matched Some"),
                )
                .await?
                {
                    TextMerge::Merged(bytes) => {
                        auto_merged = Some(bytes);
                        MergeFileStatus::AutoMerged
                    }
                    TextMerge::Conflict(preview) => {
                        conflict_preview = Some(preview);
                        MergeFileStatus::TextConflict
                    }
                    TextMerge::Binary => MergeFileStatus::BinaryConflict,
                }
            }
            (None, None, None) => unreachable!(),
        };
        analysis.push(FileAnalysis {
            review: MergeFileReview {
                path,
                renamed_from: None,
                status,
                base_digest,
                current_digest,
                result_digest,
                auto_mergeable: status == MergeFileStatus::AutoMerged,
                conflict_preview,
            },
            auto_merged,
        });
    }
    annotate_renames(&mut analysis, base, result);
    Ok(analysis)
}

enum TextMerge {
    Merged(Vec<u8>),
    Conflict(String),
    Binary,
}

#[allow(clippy::too_many_arguments)]
async fn attempt_text_merge(
    pool: &PgPool,
    store: &ObjectStore,
    base_manifest: Uuid,
    current_manifest: Uuid,
    result_manifest: Uuid,
    path: &str,
    base: &SnapshotFile,
    current: &SnapshotFile,
    result: &SnapshotFile,
) -> Result<TextMerge, ApiError> {
    if [base.size, current.size, result.size]
        .into_iter()
        .any(|size| size > MAX_TEXT_MERGE_BYTES)
    {
        return Ok(TextMerge::Binary);
    }
    let base = materialize_file(pool, store, base_manifest, path).await?;
    let current = materialize_file(pool, store, current_manifest, path).await?;
    let result = materialize_file(pool, store, result_manifest, path).await?;
    let (Ok(base), Ok(current), Ok(result)) = (
        String::from_utf8(base),
        String::from_utf8(current),
        String::from_utf8(result),
    ) else {
        return Ok(TextMerge::Binary);
    };
    match diffy::merge(&base, &current, &result) {
        Ok(merged) => Ok(TextMerge::Merged(merged.into_bytes())),
        Err(conflict) => Ok(TextMerge::Conflict(conflict.chars().take(20_000).collect())),
    }
}

fn annotate_renames(analysis: &mut [FileAnalysis], base: &VersionFiles, result: &VersionFiles) {
    let deleted: Vec<(&String, String)> = base
        .files
        .iter()
        .filter(|(path, _)| !result.files.contains_key(*path))
        .map(|(path, file)| (path, file_fingerprint(file)))
        .collect();
    for item in analysis {
        if item.review.status != MergeFileStatus::Added {
            continue;
        }
        let Some(result_file) = result.files.get(&item.review.path) else {
            continue;
        };
        let fingerprint = file_fingerprint(result_file);
        if let Some((old_path, _)) = deleted.iter().find(|(_, old)| *old == fingerprint) {
            item.review.status = MergeFileStatus::Renamed;
            item.review.renamed_from = Some((*old_path).to_owned());
        }
    }
}

fn default_resolution(status: MergeFileStatus) -> MergeResolutionChoice {
    match status {
        MergeFileStatus::Unchanged | MergeFileStatus::CurrentOnly => MergeResolutionChoice::Current,
        MergeFileStatus::AutoMerged => MergeResolutionChoice::AutoMerged,
        MergeFileStatus::Deleted => MergeResolutionChoice::Result,
        MergeFileStatus::Added
        | MergeFileStatus::ResultOnly
        | MergeFileStatus::IdenticalChange
        | MergeFileStatus::Renamed => MergeResolutionChoice::Result,
        MergeFileStatus::TextConflict | MergeFileStatus::BinaryConflict => {
            MergeResolutionChoice::Current
        }
    }
}

async fn materialize_file(
    pool: &PgPool,
    store: &ObjectStore,
    manifest_id: Uuid,
    path: &str,
) -> Result<Vec<u8>, ApiError> {
    let manifest =
        sqlx::query("SELECT * FROM snapshot_manifests WHERE id = $1 AND status = 'ready'")
            .bind(manifest_id)
            .fetch_optional(pool)
            .await?
            .ok_or_else(|| ApiError::Conflict("snapshot is not ready".to_owned()))?;
    let scope = row_scope(&manifest)?;
    let rows = sqlx::query(
        r#"
        SELECT chunk.* FROM snapshot_manifest_chunks mapping
        JOIN snapshot_chunks chunk
          ON chunk.key_scope_type = mapping.key_scope_type
         AND chunk.key_scope_id = mapping.key_scope_id
         AND chunk.plaintext_digest = mapping.plaintext_digest
        WHERE mapping.manifest_id = $1 AND mapping.path = $2
        ORDER BY mapping.chunk_index
        "#,
    )
    .bind(manifest_id)
    .bind(path)
    .fetch_all(pool)
    .await?;
    let mut output = Vec::new();
    for row in rows {
        let wrap_nonce: Option<Vec<u8>> = row.try_get("wrap_nonce")?;
        let chunk = store
            .get_decrypted(
                scope,
                row.try_get("object_key")?,
                row.try_get("wrapped_data_key")?,
                row.try_get("nonce")?,
                wrap_nonce.as_deref().ok_or_else(|| {
                    ApiError::Internal(anyhow::anyhow!("chunk has no wrapped-key nonce"))
                })?,
            )
            .await?;
        output.extend_from_slice(&chunk);
        if output.len() as u64 > MAX_TEXT_MERGE_BYTES {
            return Err(ApiError::Unprocessable(
                "text merge materialization exceeded its size limit".to_owned(),
            ));
        }
    }
    Ok(output)
}

fn file_fingerprint(file: &SnapshotFile) -> String {
    let mut hash = Sha256::new();
    hash.update(file.size.to_be_bytes());
    for chunk in &file.chunks {
        hash.update(chunk.digest.as_bytes());
        hash.update(chunk.plaintext_size.to_be_bytes());
    }
    hex::encode(hash.finalize())
}
