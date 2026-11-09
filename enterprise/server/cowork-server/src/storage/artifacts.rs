//! Encrypted run artifacts, attachments and their authenticated download endpoints.

use crate::{
    auth::{ExecutorPrincipal, Principal},
    db,
    error::ApiError,
    organization,
    storage::{
        executor_access::{ensure_executor_path, verify_executor_lease},
        object_store::{require_store, ObjectStore, AWS_ENCODE_SET},
        snapshot_repository::{project_scope, row_scope},
    },
    AppState,
};
use axum::{
    body::{Body, Bytes},
    extract::{Extension, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::Response,
    Json,
};
use cowork_contracts::{ProjectRole, RunArtifact, RunEventKind, SCHEMA_VERSION};
use percent_encoding::utf8_percent_encode;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::path::{Component, Path as FsPath};
use uuid::Uuid;

pub const MAX_AGENT_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct ExecutorArtifactQuery {
    path: String,
    source: String,
    source_event_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct UserAttachmentQuery {
    name: String,
}

#[allow(clippy::too_many_arguments)]
pub async fn persist_run_artifact(
    pool: &PgPool,
    store: &ObjectStore,
    run_id: Uuid,
    project_id: Uuid,
    creator_user_id: Uuid,
    workspace_path: &str,
    source: &str,
    source_event_id: Option<Uuid>,
    plaintext: &[u8],
) -> Result<serde_json::Value, ApiError> {
    let scope = project_scope(pool, project_id, creator_user_id).await?;
    let artifact_id = Uuid::new_v4();
    let name = workspace_path
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::Unprocessable("artifact path has no file name".to_owned()))?;
    let digest = Sha256::digest(plaintext).to_vec();
    if let Some(source_event_id) = source_event_id {
        if let Some(row) =
            sqlx::query("SELECT * FROM run_artifacts WHERE run_id = $1 AND source_event_id = $2")
                .bind(run_id)
                .bind(source_event_id)
                .fetch_optional(pool)
                .await?
        {
            let metadata: Value = row.try_get("metadata")?;
            if row.try_get::<Vec<u8>, _>("digest")? != digest
                || row.try_get::<i64, _>("size_bytes")? != plaintext.len() as i64
                || metadata.get("workspace_path").and_then(Value::as_str) != Some(workspace_path)
                || metadata.get("source").and_then(Value::as_str) != Some(source)
            {
                return Err(ApiError::Conflict(
                    "source_event_id was already used for different artifact content".to_owned(),
                ));
            }
            return Ok(json!({
                "schema_version": SCHEMA_VERSION,
                "id": row.try_get::<Uuid, _>("id")?,
                "run_id": run_id,
                "revision": row.try_get::<i64, _>("revision")?,
                "kind": row.try_get::<String, _>("kind")?,
                "media_type": row.try_get::<String, _>("media_type")?,
                "name": row.try_get::<String, _>("name")?,
                "digest": hex::encode(&digest),
                "size_bytes": plaintext.len(),
                "workspace_path": workspace_path,
                "storage": "object_store_encrypted",
            }));
        }
    }
    let encrypted = store.encrypt(scope, plaintext)?;
    let object_key = format!(
        "artifacts/{}/{}/{}/{}",
        scope.kind(),
        scope.id(),
        run_id,
        artifact_id
    );
    store
        .put_ciphertext(&object_key, encrypted.ciphertext.clone())
        .await?;
    let revision = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(revision), 0) + 1 FROM run_artifacts WHERE run_id = $1 AND name = $2",
    )
    .bind(run_id)
    .bind(name)
    .fetch_one(pool)
    .await?;
    let (kind, media_type) = artifact_media(name);
    let inserted = sqlx::query(
        "INSERT INTO run_artifacts (id, run_id, revision, kind, media_type, name, object_key, digest, size_bytes, metadata, key_scope_type, key_scope_id, encrypted_data_key, nonce, wrap_nonce, source_event_id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
    )
    .bind(artifact_id)
    .bind(run_id)
    .bind(revision)
    .bind(kind)
    .bind(media_type)
    .bind(name)
    .bind(&object_key)
    .bind(&digest)
    .bind(plaintext.len() as i64)
    .bind(json!({"workspace_path": workspace_path, "source": source}))
    .bind(scope.kind())
    .bind(scope.id())
    .bind(&encrypted.wrapped_data_key)
    .bind(encrypted.nonce.as_slice())
    .bind(encrypted.wrap_nonce.as_slice())
    .bind(source_event_id)
    .execute(pool)
    .await;
    if let Err(error) = inserted {
        let _ = store.delete(&object_key).await;
        return Err(error.into());
    }
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "id": artifact_id,
        "run_id": run_id,
        "revision": revision,
        "kind": kind,
        "media_type": media_type,
        "name": name,
        "digest": hex::encode(digest),
        "size_bytes": plaintext.len(),
        "workspace_path": workspace_path,
        "storage": "object_store_encrypted",
    }))
}

fn artifact_media(name: &str) -> (&'static str, &'static str) {
    match name
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => ("image", "image/png"),
        "jpg" | "jpeg" => ("image", "image/jpeg"),
        "webp" => ("image", "image/webp"),
        "pdf" => ("document", "application/pdf"),
        "zip" => ("trace", "application/zip"),
        "webm" => ("video", "video/webm"),
        "json" => ("log", "application/json"),
        "txt" | "log" => ("log", "text/plain; charset=utf-8"),
        _ => ("file", "application/octet-stream"),
    }
}

pub async fn list_run_artifacts(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<RunArtifact>>, ApiError> {
    let run = sqlx::query("SELECT project_id, thread_id FROM runs WHERE id = $1")
        .bind(run_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("run {run_id} was not found")))?;
    organization::ensure_thread_role(
        &state.pool,
        principal.user_id,
        run.try_get("project_id")?,
        run.try_get("thread_id")?,
        ProjectRole::Viewer,
    )
    .await?;
    let rows = sqlx::query(
        "SELECT * FROM run_artifacts WHERE run_id = $1 AND deleted_at IS NULL ORDER BY created_at, id",
    )
    .bind(run_id)
    .fetch_all(&state.pool)
    .await?;
    let artifacts = rows
        .iter()
        .map(row_to_artifact)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(artifacts))
}

pub async fn download_run_artifact(
    State(state): State<AppState>,
    Path((run_id, artifact_id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<Principal>,
) -> Result<Response, ApiError> {
    let row = sqlx::query(
        "SELECT artifact.*, run.project_id, run.thread_id FROM run_artifacts artifact JOIN runs run ON run.id = artifact.run_id WHERE artifact.id = $1 AND artifact.run_id = $2 AND artifact.deleted_at IS NULL",
    )
    .bind(artifact_id)
    .bind(run_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("artifact {artifact_id} was not found")))?;
    organization::ensure_thread_role(
        &state.pool,
        principal.user_id,
        row.try_get("project_id")?,
        row.try_get("thread_id")?,
        ProjectRole::Viewer,
    )
    .await?;
    let store = state
        .object_store
        .as_ref()
        .ok_or_else(|| ApiError::Conflict("object storage is not configured".to_owned()))?;
    let scope = row_scope(&row)?;
    let wrapped_data_key = row
        .try_get::<Option<Vec<u8>>, _>("encrypted_data_key")?
        .ok_or_else(|| ApiError::Conflict("artifact has no encrypted data key".to_owned()))?;
    let nonce = row
        .try_get::<Option<Vec<u8>>, _>("nonce")?
        .ok_or_else(|| ApiError::Conflict("artifact has no encryption nonce".to_owned()))?;
    let wrap_nonce = row
        .try_get::<Option<Vec<u8>>, _>("wrap_nonce")?
        .ok_or_else(|| ApiError::Conflict("artifact has no wrapped-key nonce".to_owned()))?;
    let object_key: String = row.try_get("object_key")?;
    let bytes = store
        .get_decrypted(scope, &object_key, &wrapped_data_key, &nonce, &wrap_nonce)
        .await?;
    let expected_digest: Vec<u8> = row.try_get("digest")?;
    if Sha256::digest(&bytes).as_slice() != expected_digest.as_slice() {
        return Err(ApiError::Internal(anyhow::anyhow!(
            "artifact digest verification failed"
        )));
    }
    let name: String = row.try_get("name")?;
    let media_type: String = row.try_get("media_type")?;
    let disposition_name = utf8_percent_encode(&name, AWS_ENCODE_SET).to_string();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, media_type)
        .header(header::CONTENT_LENGTH, bytes.len())
        .header(
            header::CONTENT_DISPOSITION,
            format!("inline; filename*=UTF-8''{disposition_name}"),
        )
        .body(Body::from(bytes))
        .map_err(|error| ApiError::Internal(error.into()))
}

pub async fn upload_run_attachment(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
    Query(query): Query<UserAttachmentQuery>,
    Extension(principal): Extension<Principal>,
    body: Bytes,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    validate_attachment_name(&query.name)?;
    if body.is_empty() {
        return Err(ApiError::Unprocessable(
            "attachment must not be empty".to_owned(),
        ));
    }
    let project_id = sqlx::query_scalar::<_, Uuid>("SELECT project_id FROM runs WHERE id = $1")
        .bind(run_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("run {run_id} was not found")))?;
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Runner,
    )
    .await?;
    let store = require_store(&state)?;
    let workspace_path = format!("artifacts/user-uploads/{}", query.name);
    let mut payload = persist_run_artifact(
        &state.pool,
        &store,
        run_id,
        project_id,
        principal.user_id,
        &workspace_path,
        "user_attachment",
        None,
        &body,
    )
    .await?;
    if let Some(object) = payload.as_object_mut() {
        object.insert("uploaded_by_user_id".to_owned(), json!(principal.user_id));
    }
    let mut tx = state.pool.begin().await?;
    db::append_event_tx(
        &mut tx,
        run_id,
        RunEventKind::ArtifactCreated,
        payload.clone(),
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(payload)))
}

fn row_to_artifact(row: &sqlx::postgres::PgRow) -> Result<RunArtifact, ApiError> {
    let digest: Vec<u8> = row.try_get("digest")?;
    Ok(RunArtifact {
        schema_version: SCHEMA_VERSION,
        id: row.try_get("id")?,
        run_id: row.try_get("run_id")?,
        revision: row.try_get("revision")?,
        kind: row.try_get("kind")?,
        media_type: row.try_get("media_type")?,
        name: row.try_get("name")?,
        digest: hex::encode(digest),
        size_bytes: row.try_get::<i64, _>("size_bytes")? as u64,
        metadata: row.try_get("metadata")?,
        created_at: row.try_get("created_at")?,
        deleted_at: row.try_get("deleted_at")?,
    })
}

pub async fn upload_executor_run_artifact(
    State(state): State<AppState>,
    Path((executor_id, run_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<ExecutorArtifactQuery>,
    Extension(principal): Extension<ExecutorPrincipal>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    ensure_executor_path(&principal, executor_id)?;
    validate_executor_artifact_path(&query.path)?;
    if query.source.trim().is_empty() || query.source.len() > 100 {
        return Err(ApiError::Unprocessable(
            "artifact source must contain 1 to 100 characters".to_owned(),
        ));
    }
    let run = verify_executor_lease(&state.pool, run_id, executor_id, &headers).await?;
    let store = require_store(&state)?;
    let payload = persist_run_artifact(
        &state.pool,
        &store,
        run_id,
        run.spec.project_id,
        run.spec.creator_user_id,
        &query.path,
        &query.source,
        query.source_event_id,
        &body,
    )
    .await?;
    let mut tx = state.pool.begin().await?;
    match query.source_event_id {
        Some(event_id) => {
            db::append_event_tx_with_id(
                &mut tx,
                run_id,
                event_id,
                RunEventKind::ArtifactCreated,
                payload.clone(),
            )
            .await?;
        }
        None => {
            db::append_event_tx(
                &mut tx,
                run_id,
                RunEventKind::ArtifactCreated,
                payload.clone(),
            )
            .await?;
        }
    }
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(payload)))
}

fn validate_executor_artifact_path(path: &str) -> Result<(), ApiError> {
    if path.is_empty() || path.len() > 1_024 || path.contains('\\') {
        return Err(ApiError::Unprocessable(
            "artifact path is invalid".to_owned(),
        ));
    }
    let mut components = FsPath::new(path).components();
    if !matches!(components.next(), Some(Component::Normal(value)) if value == "artifacts")
        || components.any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ApiError::Unprocessable(
            "artifact path must stay below artifacts/".to_owned(),
        ));
    }
    Ok(())
}

fn validate_attachment_name(name: &str) -> Result<(), ApiError> {
    if name.is_empty()
        || name.len() > 255
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', '\0'])
        || name.chars().any(char::is_control)
    {
        return Err(ApiError::Unprocessable(
            "attachment name must be a single safe file name".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_single_safe_attachment_names() {
        assert!(validate_attachment_name("camera-photo.jpg").is_ok());
        assert!(validate_attachment_name("Bericht 2026.xlsx").is_ok());
        assert!(validate_attachment_name("../secret.txt").is_err());
        assert!(validate_attachment_name("folder/file.txt").is_err());
        assert!(validate_attachment_name("folder\\file.txt").is_err());
        assert!(validate_attachment_name("bad\0name.txt").is_err());
    }
}
