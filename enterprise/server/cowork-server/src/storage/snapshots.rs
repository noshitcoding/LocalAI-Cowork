//! Authenticated snapshot upload, commit and download lifecycle, including executor result snapshots.

use crate::{
    auth::{ExecutorPrincipal, Principal},
    db,
    error::ApiError,
    governance, organization,
    storage::{
        executor_access::{ensure_executor_path, verify_executor_lease},
        object_store::require_store,
        snapshot_repository::{
            accessible_manifest, decrypt_manifest_chunk, existing_digests, fetch_chunk_rows,
            insert_chunk_reservation, project_scope, release_manifest_references,
            reserve_existing_chunks, row_scope, snapshot_from_row,
        },
        snapshot_validation::{
            detect_secret_warnings, unique_digests, validate_digest, validate_snapshot_files,
            validated_snapshot_expiry, MAX_CHUNK_BYTES,
        },
    },
    AppState,
};
use axum::{
    body::Bytes,
    extract::{Extension, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use cowork_contracts::{
    BeginSnapshotUploadRequest, ExecutorTarget, ProjectRole, SnapshotChunk, SnapshotChunkReceipt,
    SnapshotFile, SnapshotManifest, SnapshotUploadFile, SnapshotUploadSession, SCHEMA_VERSION,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use std::collections::HashSet;
use uuid::Uuid;

fn ensure_upload_unexpired(row: &sqlx::postgres::PgRow) -> Result<(), ApiError> {
    let expires_at: Option<chrono::DateTime<chrono::Utc>> = row.try_get("expires_at")?;
    if expires_at.is_some_and(|expiry| expiry <= chrono::Utc::now()) {
        return Err(ApiError::Conflict(
            "the original snapshot upload has expired; explicitly prepare a new upload".to_owned(),
        ));
    }
    Ok(())
}

pub async fn begin_snapshot_upload(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<BeginSnapshotUploadRequest>,
) -> Result<(StatusCode, Json<SnapshotUploadSession>), ApiError> {
    begin_snapshot_upload_for(&state, principal.user_id, request, None).await
}

async fn begin_snapshot_upload_for(
    state: &AppState,
    user_id: Uuid,
    request: BeginSnapshotUploadRequest,
    source_run_id: Option<Uuid>,
) -> Result<(StatusCode, Json<SnapshotUploadSession>), ApiError> {
    require_store(state)?;
    organization::ensure_project_role(
        &state.pool,
        user_id,
        request.project_id,
        ProjectRole::Runner,
    )
    .await?;
    validate_snapshot_files(request.total_bytes, &request.files)?;
    let scope = project_scope(&state.pool, request.project_id, user_id).await?;
    let expires_at = validated_snapshot_expiry(scope, request.expires_at)?;
    let unique_digests = unique_digests(&request.files)?;
    if let Some(source_run_id) = source_run_id {
        if let Some(row) = sqlx::query("SELECT * FROM snapshot_manifests WHERE source_run_id = $1")
            .bind(source_run_id)
            .fetch_optional(&state.pool)
            .await?
        {
            if row.try_get::<Uuid, _>("project_id")? != request.project_id
                || row.try_get::<Uuid, _>("created_by")? != user_id
            {
                return Err(ApiError::Conflict(
                    "the run result snapshot belongs to a different project or user".to_owned(),
                ));
            }
            let existing_total = u64::try_from(row.try_get::<i64, _>("total_bytes")?)
                .map_err(|error| ApiError::Internal(error.into()))?;
            let existing_manifest: Value = row.try_get("manifest")?;
            if existing_total != request.total_bytes
                || existing_manifest != serde_json::to_value(&request.files)?
            {
                return Err(ApiError::Conflict(
                    "the retried result snapshot manifest differs from its original declaration"
                        .to_owned(),
                ));
            }
            return Ok((
                StatusCode::OK,
                Json(snapshot_upload_session_from_row(state, &row).await?),
            ));
        }
    }
    let request_sha256 = Sha256::digest(serde_json::to_vec(&request)?).to_vec();
    let manifest_id = Uuid::new_v4();
    let mut tx = state.pool.begin().await?;
    if let Some(key) = request.idempotency_key {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("snapshot-upload:{user_id}:{key}"))
            .execute(&mut *tx)
            .await?;
        if let Some(intent) = sqlx::query("SELECT manifest_id, request_sha256 FROM snapshot_upload_intents WHERE user_id=$1 AND idempotency_key=$2")
            .bind(user_id).bind(key).fetch_optional(&mut *tx).await? {
            if intent.try_get::<Vec<u8>, _>("request_sha256")? != request_sha256 {
                return Err(ApiError::Conflict("the original snapshot upload has different files; explicitly prepare a new upload".to_owned()));
            }
            let original:Uuid=intent.try_get("manifest_id")?;
            let row=sqlx::query("SELECT * FROM snapshot_manifests WHERE id=$1 AND project_id=$2")
                .bind(original).bind(request.project_id).fetch_one(&mut *tx).await?;
            ensure_upload_unexpired(&row)?;
            if !matches!(row.try_get::<String,_>("status")?.as_str(), "uploading"|"ready") {
                return Err(ApiError::Conflict("the original snapshot upload has expired; explicitly prepare a new upload".to_owned()));
            }
            tx.commit().await?;
            return Ok((StatusCode::OK,Json(snapshot_upload_session_from_row(state,&row).await?)));
        }
    }
    governance::enforce_storage_quota_tx(&mut tx, scope.kind(), scope.id(), request.total_bytes)
        .await?;
    sqlx::query(
        r#"
        INSERT INTO snapshot_manifests (
            id, project_id, created_by, key_scope_type, key_scope_id,
            encryption_key_id, total_bytes, file_count, manifest, status,
            expires_at, source_run_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'uploading', $10, $11)
        "#,
    )
    .bind(manifest_id)
    .bind(request.project_id)
    .bind(user_id)
    .bind(scope.kind())
    .bind(scope.id())
    .bind(scope.key_id())
    .bind(
        i64::try_from(request.total_bytes)
            .map_err(|_| ApiError::Unprocessable("snapshot total_bytes is too large".to_owned()))?,
    )
    .bind(
        i64::try_from(request.files.len())
            .map_err(|_| ApiError::Unprocessable("snapshot contains too many files".to_owned()))?,
    )
    .bind(serde_json::to_value(&request.files)?)
    .bind(expires_at)
    .bind(source_run_id)
    .execute(&mut *tx)
    .await?;
    if let Some(key) = request.idempotency_key {
        sqlx::query("INSERT INTO snapshot_upload_intents(user_id,idempotency_key,manifest_id,request_sha256) VALUES($1,$2,$3,$4)")
            .bind(user_id).bind(key).bind(manifest_id).bind(&request_sha256)
            .execute(&mut *tx).await?;
    }
    let reserved =
        reserve_existing_chunks(&mut tx, manifest_id, scope, unique_digests.keys()).await?;
    tx.commit().await?;
    let missing_chunks = unique_digests
        .keys()
        .filter(|digest| !reserved.contains(*digest))
        .cloned()
        .collect();
    Ok((
        StatusCode::CREATED,
        Json(SnapshotUploadSession {
            schema_version: SCHEMA_VERSION,
            manifest_id,
            missing_chunks,
            max_chunk_bytes: MAX_CHUNK_BYTES as u64,
            expires_at,
            warnings: Vec::new(),
        }),
    ))
}

pub async fn snapshot_upload_status(
    State(state): State<AppState>,
    Path(manifest_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<SnapshotUploadSession>, ApiError> {
    let row = accessible_manifest(
        &state.pool,
        principal.user_id,
        manifest_id,
        ProjectRole::Runner,
    )
    .await?;
    Ok(Json(snapshot_upload_session_from_row(&state, &row).await?))
}

async fn snapshot_upload_session_from_row(
    state: &AppState,
    row: &sqlx::postgres::PgRow,
) -> Result<SnapshotUploadSession, ApiError> {
    ensure_upload_unexpired(row)?;
    let files: Vec<SnapshotUploadFile> = serde_json::from_value(row.try_get("manifest")?)?;
    let scope = row_scope(row)?;
    let unique = unique_digests(&files)?;
    let existing = existing_digests(&state.pool, scope, &unique).await?;
    let missing_chunks = unique
        .keys()
        .filter(|digest| !existing.contains(*digest))
        .cloned()
        .collect();
    Ok(SnapshotUploadSession {
        schema_version: SCHEMA_VERSION,
        manifest_id: row.try_get("id")?,
        missing_chunks,
        max_chunk_bytes: MAX_CHUNK_BYTES as u64,
        expires_at: row.try_get("expires_at")?,
        warnings: serde_json::from_value(row.try_get("warnings")?)?,
    })
}

pub async fn upload_snapshot_chunk(
    State(state): State<AppState>,
    Path((manifest_id, digest_hex)): Path<(Uuid, String)>,
    Extension(principal): Extension<Principal>,
    body: Bytes,
) -> Result<(StatusCode, Json<SnapshotChunkReceipt>), ApiError> {
    let store = require_store(&state)?;
    validate_digest(&digest_hex)?;
    if body.len() > MAX_CHUNK_BYTES {
        return Err(ApiError::Unprocessable(format!(
            "chunk exceeds maximum of {MAX_CHUNK_BYTES} bytes"
        )));
    }
    let manifest = accessible_manifest(
        &state.pool,
        principal.user_id,
        manifest_id,
        ProjectRole::Runner,
    )
    .await?;
    ensure_upload_unexpired(&manifest)?;
    let status: String = manifest.try_get("status")?;
    if status != "uploading" {
        return Err(ApiError::Conflict(
            "chunks can only be uploaded while the snapshot is uploading".to_owned(),
        ));
    }
    let files: Vec<SnapshotUploadFile> = serde_json::from_value(manifest.try_get("manifest")?)?;
    let expected = unique_digests(&files)?
        .get(&digest_hex)
        .copied()
        .ok_or_else(|| ApiError::Unprocessable("chunk is not part of this manifest".to_owned()))?;
    if expected != body.len() as u64 {
        return Err(ApiError::Unprocessable(format!(
            "chunk size is {}, expected {expected}",
            body.len()
        )));
    }
    let actual_digest = hex::encode(Sha256::digest(&body));
    if actual_digest != digest_hex {
        return Err(ApiError::Unprocessable(
            "chunk SHA-256 digest does not match its URL".to_owned(),
        ));
    }
    let scope = row_scope(&manifest)?;
    let digest = hex::decode(&digest_hex)
        .map_err(|_| ApiError::Unprocessable("invalid chunk digest".to_owned()))?;
    let mut tx = state.pool.begin().await?;
    // Serialize concurrent uploads of the same scope/digest without relying on
    // a process-local mutex (API replicas remain safe).
    let lock_name = format!("{}:{}:{digest_hex}", scope.kind(), scope.id());
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(lock_name)
        .execute(&mut *tx)
        .await?;
    if let Some(row) = sqlx::query(
        r#"
        SELECT plaintext_size, ciphertext_size FROM snapshot_chunks
        WHERE key_scope_type = $1 AND key_scope_id = $2
          AND plaintext_digest = $3 AND status = 'ready'
        "#,
    )
    .bind(scope.kind())
    .bind(scope.id())
    .bind(&digest)
    .fetch_optional(&mut *tx)
    .await?
    {
        insert_chunk_reservation(&mut tx, manifest_id, scope, &digest).await?;
        tx.commit().await?;
        return Ok((
            StatusCode::OK,
            Json(SnapshotChunkReceipt {
                schema_version: SCHEMA_VERSION,
                manifest_id,
                digest: digest_hex,
                plaintext_size: u64::try_from(row.try_get::<i64, _>("plaintext_size")?)
                    .map_err(|error| ApiError::Internal(error.into()))?,
                ciphertext_size: u64::try_from(row.try_get::<i64, _>("ciphertext_size")?)
                    .map_err(|error| ApiError::Internal(error.into()))?,
                deduplicated: true,
                warnings: Vec::new(),
            }),
        ));
    }
    let warnings = detect_secret_warnings(&body);
    let (object_key, encrypted) = store.put_encrypted(scope, &digest_hex, &body).await?;
    let ciphertext_size = encrypted.ciphertext.len() as u64;
    sqlx::query(
        r#"
        INSERT INTO snapshot_chunks (
            key_scope_type, key_scope_id, plaintext_digest, object_key,
            plaintext_size, ciphertext_size, wrapped_data_key, nonce,
            wrap_nonce, status
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'ready')
        "#,
    )
    .bind(scope.kind())
    .bind(scope.id())
    .bind(&digest)
    .bind(object_key)
    .bind(i64::try_from(body.len()).map_err(|error| ApiError::Internal(error.into()))?)
    .bind(i64::try_from(ciphertext_size).map_err(|error| ApiError::Internal(error.into()))?)
    .bind(encrypted.wrapped_data_key)
    .bind(encrypted.nonce.as_slice())
    .bind(encrypted.wrap_nonce.as_slice())
    .execute(&mut *tx)
    .await?;
    insert_chunk_reservation(&mut tx, manifest_id, scope, &digest).await?;
    if !warnings.is_empty() {
        sqlx::query("UPDATE snapshot_manifests SET warnings = warnings || $2::jsonb WHERE id = $1")
            .bind(manifest_id)
            .bind(serde_json::to_value(&warnings)?)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(SnapshotChunkReceipt {
            schema_version: SCHEMA_VERSION,
            manifest_id,
            digest: digest_hex,
            plaintext_size: body.len() as u64,
            ciphertext_size,
            deduplicated: false,
            warnings,
        }),
    ))
}

pub async fn commit_snapshot(
    State(state): State<AppState>,
    Path(manifest_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<SnapshotManifest>, ApiError> {
    require_store(&state)?;
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query("SELECT * FROM snapshot_manifests WHERE id = $1 FOR UPDATE")
        .bind(manifest_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("snapshot {manifest_id} was not found")))?;
    let project_id: Uuid = row.try_get("project_id")?;
    organization::ensure_project_role(
        &state.pool,
        principal.user_id,
        project_id,
        ProjectRole::Runner,
    )
    .await?;
    ensure_upload_unexpired(&row)?;
    let status: String = row.try_get("status")?;
    if status == "ready" {
        return Ok(Json(snapshot_from_row(&state.pool, &row).await?));
    }
    if status != "uploading" {
        return Err(ApiError::Conflict(
            "snapshot is not in an uploadable state".to_owned(),
        ));
    }
    let scope = row_scope(&row)?;
    let upload_files: Vec<SnapshotUploadFile> = serde_json::from_value(row.try_get("manifest")?)?;
    let unique = unique_digests(&upload_files)?;
    let chunk_rows = fetch_chunk_rows(&mut tx, scope, unique.keys()).await?;
    if chunk_rows.len() != unique.len() {
        let present: HashSet<&str> = chunk_rows.keys().map(String::as_str).collect();
        let missing: Vec<&str> = unique
            .keys()
            .map(String::as_str)
            .filter(|digest| !present.contains(digest))
            .take(20)
            .collect();
        return Err(ApiError::Conflict(format!(
            "snapshot still has missing chunks: {}",
            missing.join(", ")
        )));
    }

    let mut mappings = Vec::new();
    let mut files = Vec::with_capacity(upload_files.len());
    for file in upload_files {
        let mut chunks = Vec::with_capacity(file.chunks.len());
        for (index, requested) in file.chunks.into_iter().enumerate() {
            let stored = chunk_rows.get(&requested.digest).ok_or_else(|| {
                ApiError::Internal(anyhow::anyhow!("validated snapshot chunk disappeared"))
            })?;
            if stored.plaintext_size != requested.plaintext_size {
                return Err(ApiError::Conflict(format!(
                    "stored chunk {} has a different plaintext size",
                    requested.digest
                )));
            }
            mappings.push((
                file.path.clone(),
                i32::try_from(index).map_err(|error| ApiError::Internal(error.into()))?,
                hex::decode(&requested.digest)
                    .map_err(|_| ApiError::Internal(anyhow::anyhow!("invalid validated digest")))?,
            ));
            chunks.push(SnapshotChunk {
                digest: requested.digest,
                plaintext_size: requested.plaintext_size,
                ciphertext_size: stored.ciphertext_size,
            });
        }
        files.push(SnapshotFile {
            path: file.path,
            size: file.size,
            mode: file.mode,
            modified_at: file.modified_at,
            chunks,
        });
    }
    if !mappings.is_empty() {
        let mut query = QueryBuilder::<Postgres>::new(
            "INSERT INTO snapshot_manifest_chunks (manifest_id, path, chunk_index, key_scope_type, key_scope_id, plaintext_digest) ",
        );
        query.push_values(mappings, |mut values, (path, index, digest)| {
            values
                .push_bind(manifest_id)
                .push_bind(path)
                .push_bind(index)
                .push_bind(scope.kind())
                .push_bind(scope.id())
                .push_bind(digest);
        });
        query.push(" ON CONFLICT (manifest_id, path, chunk_index) DO NOTHING");
        query.build().execute(&mut *tx).await?;
        sqlx::query(
            r#"
            UPDATE snapshot_chunks chunk
            SET ref_count = chunk.ref_count + refs.count,
                last_referenced_at = now()
            FROM (
                SELECT key_scope_type, key_scope_id, plaintext_digest,
                       count(*)::bigint AS count
                FROM snapshot_manifest_chunks WHERE manifest_id = $1
                GROUP BY key_scope_type, key_scope_id, plaintext_digest
            ) refs
            WHERE chunk.key_scope_type = refs.key_scope_type
              AND chunk.key_scope_id = refs.key_scope_id
              AND chunk.plaintext_digest = refs.plaintext_digest
            "#,
        )
        .bind(manifest_id)
        .execute(&mut *tx)
        .await?;
    }
    let row = sqlx::query(
        "UPDATE snapshot_manifests SET status = 'ready', committed_at = now() WHERE id = $1 RETURNING *",
    )
    .bind(manifest_id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM snapshot_chunk_reservations WHERE manifest_id = $1")
        .bind(manifest_id)
        .execute(&mut *tx)
        .await?;
    let manifest = SnapshotManifest {
        schema_version: SCHEMA_VERSION,
        id: manifest_id,
        project_id,
        total_bytes: u64::try_from(row.try_get::<i64, _>("total_bytes")?)
            .map_err(|error| ApiError::Internal(error.into()))?,
        files,
        encryption_key_id: row.try_get("encryption_key_id")?,
        created_at: row.try_get("created_at")?,
        expires_at: row.try_get("expires_at")?,
    };
    tx.commit().await?;
    resume_snapshot_runs(&state, manifest_id).await?;
    Ok(Json(manifest))
}

pub async fn get_snapshot(
    State(state): State<AppState>,
    Path(manifest_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<SnapshotManifest>, ApiError> {
    let row = accessible_manifest(
        &state.pool,
        principal.user_id,
        manifest_id,
        ProjectRole::Viewer,
    )
    .await?;
    if row.try_get::<String, _>("status")? != "ready" {
        return Err(ApiError::Conflict("snapshot is not ready".to_owned()));
    }
    Ok(Json(snapshot_from_row(&state.pool, &row).await?))
}

pub async fn download_snapshot_chunk(
    State(state): State<AppState>,
    Path((manifest_id, digest_hex)): Path<(Uuid, String)>,
    Extension(principal): Extension<Principal>,
) -> Result<impl IntoResponse, ApiError> {
    let store = require_store(&state)?;
    validate_digest(&digest_hex)?;
    let manifest = accessible_manifest(
        &state.pool,
        principal.user_id,
        manifest_id,
        ProjectRole::Viewer,
    )
    .await?;
    if manifest.try_get::<String, _>("status")? != "ready" {
        return Err(ApiError::Conflict("snapshot is not ready".to_owned()));
    }
    let plaintext =
        decrypt_manifest_chunk(&state.pool, &store, &manifest, manifest_id, &digest_hex).await?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CACHE_CONTROL, "private, no-store"),
        ],
        plaintext,
    ))
}

pub async fn get_executor_run_snapshot(
    State(state): State<AppState>,
    Path((executor_id, run_id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<ExecutorPrincipal>,
    headers: HeaderMap,
) -> Result<Json<SnapshotManifest>, ApiError> {
    ensure_executor_path(&principal, executor_id)?;
    let run = verify_executor_lease(&state.pool, run_id, executor_id, &headers).await?;
    let manifest_id = run
        .spec
        .snapshot_id
        .ok_or_else(|| ApiError::NotFound("the run has no snapshot".to_owned()))?;
    let row = sqlx::query(
        "SELECT * FROM snapshot_manifests WHERE id = $1 AND project_id = $2 AND status = 'ready' AND (expires_at IS NULL OR expires_at > now())",
    )
    .bind(manifest_id)
    .bind(run.spec.project_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("snapshot {manifest_id} was not found")))?;
    Ok(Json(snapshot_from_row(&state.pool, &row).await?))
}

pub async fn begin_executor_run_result_snapshot(
    State(state): State<AppState>,
    Path((executor_id, run_id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<ExecutorPrincipal>,
    headers: HeaderMap,
    Json(request): Json<BeginSnapshotUploadRequest>,
) -> Result<(StatusCode, Json<SnapshotUploadSession>), ApiError> {
    ensure_executor_path(&principal, executor_id)?;
    let run = verify_executor_lease(&state.pool, run_id, executor_id, &headers).await?;
    if request.project_id != run.spec.project_id {
        return Err(ApiError::Unprocessable(
            "a result snapshot must belong to the leased run project".to_owned(),
        ));
    }
    begin_snapshot_upload_for(&state, run.spec.creator_user_id, request, Some(run_id)).await
}

pub async fn upload_executor_run_result_chunk(
    State(state): State<AppState>,
    Path((executor_id, run_id, manifest_id, digest_hex)): Path<(Uuid, Uuid, Uuid, String)>,
    Extension(principal): Extension<ExecutorPrincipal>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<SnapshotChunkReceipt>), ApiError> {
    ensure_executor_path(&principal, executor_id)?;
    let run = verify_executor_lease(&state.pool, run_id, executor_id, &headers).await?;
    ensure_run_result_manifest(&state.pool, manifest_id, &run).await?;
    upload_snapshot_chunk(
        State(state),
        Path((manifest_id, digest_hex)),
        Extension(Principal {
            user_id: run.spec.creator_user_id,
            session_id: None,
            bootstrap: false,
        }),
        body,
    )
    .await
}

pub async fn get_executor_run_result_snapshot(
    State(state): State<AppState>,
    Path((executor_id, run_id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<ExecutorPrincipal>,
    headers: HeaderMap,
) -> Result<Json<Option<SnapshotManifest>>, ApiError> {
    ensure_executor_path(&principal, executor_id)?;
    let run = verify_executor_lease(&state.pool, run_id, executor_id, &headers).await?;
    let row = sqlx::query(
        r#"
        SELECT * FROM snapshot_manifests
        WHERE source_run_id = $1 AND project_id = $2 AND created_by = $3
          AND status = 'ready' AND (expires_at IS NULL OR expires_at > now())
        "#,
    )
    .bind(run_id)
    .bind(run.spec.project_id)
    .bind(run.spec.creator_user_id)
    .fetch_optional(&state.pool)
    .await?;
    match row {
        Some(row) => Ok(Json(Some(snapshot_from_row(&state.pool, &row).await?))),
        None => Ok(Json(None)),
    }
}

pub async fn commit_executor_run_result_snapshot(
    State(state): State<AppState>,
    Path((executor_id, run_id, manifest_id)): Path<(Uuid, Uuid, Uuid)>,
    Extension(principal): Extension<ExecutorPrincipal>,
    headers: HeaderMap,
) -> Result<Json<SnapshotManifest>, ApiError> {
    ensure_executor_path(&principal, executor_id)?;
    let run = verify_executor_lease(&state.pool, run_id, executor_id, &headers).await?;
    ensure_run_result_manifest(&state.pool, manifest_id, &run).await?;
    commit_snapshot(
        State(state),
        Path(manifest_id),
        Extension(Principal {
            user_id: run.spec.creator_user_id,
            session_id: None,
            bootstrap: false,
        }),
    )
    .await
}

pub async fn download_executor_run_chunk(
    State(state): State<AppState>,
    Path((executor_id, run_id, digest_hex)): Path<(Uuid, Uuid, String)>,
    Extension(principal): Extension<ExecutorPrincipal>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    ensure_executor_path(&principal, executor_id)?;
    validate_digest(&digest_hex)?;
    let run = verify_executor_lease(&state.pool, run_id, executor_id, &headers).await?;
    let manifest_id = run
        .spec
        .snapshot_id
        .ok_or_else(|| ApiError::NotFound("the run has no snapshot".to_owned()))?;
    let manifest = sqlx::query(
        "SELECT * FROM snapshot_manifests WHERE id = $1 AND project_id = $2 AND status = 'ready' AND (expires_at IS NULL OR expires_at > now())",
    )
    .bind(manifest_id)
    .bind(run.spec.project_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("snapshot {manifest_id} was not found")))?;
    let store = require_store(&state)?;
    let plaintext =
        decrypt_manifest_chunk(&state.pool, &store, &manifest, manifest_id, &digest_hex).await?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CACHE_CONTROL, "private, no-store"),
        ],
        plaintext,
    ))
}

async fn ensure_run_result_manifest(
    pool: &PgPool,
    manifest_id: Uuid,
    run: &cowork_contracts::RunRecord,
) -> Result<(), ApiError> {
    let matches = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM snapshot_manifests
            WHERE id = $1 AND source_run_id = $2 AND project_id = $3
              AND created_by = $4
        )
        "#,
    )
    .bind(manifest_id)
    .bind(run.spec.id)
    .bind(run.spec.project_id)
    .bind(run.spec.creator_user_id)
    .fetch_one(pool)
    .await?;
    if !matches {
        return Err(ApiError::NotFound(
            "the result snapshot does not belong to this leased run".to_owned(),
        ));
    }
    Ok(())
}

pub async fn delete_snapshot(
    State(state): State<AppState>,
    Path(manifest_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
) -> Result<StatusCode, ApiError> {
    let row = accessible_manifest(
        &state.pool,
        principal.user_id,
        manifest_id,
        ProjectRole::Editor,
    )
    .await?;
    let used = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM project_versions WHERE snapshot_manifest_id = $1)",
    )
    .bind(manifest_id)
    .fetch_one(&state.pool)
    .await?;
    if used {
        return Err(ApiError::Conflict(
            "snapshot is retained by a project version".to_owned(),
        ));
    }
    if row.try_get::<String, _>("status")? == "ready" {
        release_manifest_references(&state.pool, manifest_id).await?;
    }
    sqlx::query("DELETE FROM snapshot_chunk_reservations WHERE manifest_id = $1")
        .bind(manifest_id)
        .execute(&state.pool)
        .await?;
    sqlx::query("UPDATE snapshot_manifests SET status = 'expired' WHERE id = $1")
        .bind(manifest_id)
        .execute(&state.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn resume_snapshot_runs(state: &AppState, manifest_id: Uuid) -> Result<(), ApiError> {
    let run_ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM runs WHERE snapshot_id = $1 AND state = 'waiting_for_snapshot'",
    )
    .bind(manifest_id)
    .fetch_all(&state.pool)
    .await?;
    for run_id in run_ids {
        let run = db::get_run(&state.pool, run_id).await?;
        let available = match &run.spec.executor_target {
            ExecutorTarget::ServerLinux { .. } => {
                let server: HashSet<&str> = state
                    .server_capabilities
                    .iter()
                    .map(|capability| capability.0.as_str())
                    .collect();
                run.spec
                    .required_capabilities
                    .iter()
                    .all(|required| server.contains(required.0.as_str()))
            }
            target => {
                db::target_has_executor(
                    &state.pool,
                    target,
                    &run.spec.required_capabilities,
                    &run.spec.input,
                )
                .await?
            }
        };
        db::transition_run(
            &state.pool,
            run_id,
            if available {
                cowork_contracts::RunState::Queued
            } else {
                cowork_contracts::RunState::WaitingForExecutor
            },
            None,
            None,
        )
        .await?;
    }
    Ok(())
}
