//! Snapshot manifests, scoped chunk persistence, reservations and reference accounting.

use crate::{
    error::ApiError,
    organization,
    storage::{
        envelope::KeyScope,
        object_store::ObjectStore,
        snapshot_validation::{validate_snapshot_files, MAX_CHUNK_BYTES},
    },
};
use chrono::{Duration, Utc};
use cowork_contracts::{
    ProjectRole, SnapshotChunk, SnapshotFile, SnapshotManifest, SnapshotUploadChunk,
    SnapshotUploadFile, SCHEMA_VERSION,
};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

pub(super) async fn decrypt_manifest_chunk(
    pool: &PgPool,
    store: &ObjectStore,
    manifest: &sqlx::postgres::PgRow,
    manifest_id: Uuid,
    digest_hex: &str,
) -> Result<Vec<u8>, ApiError> {
    let scope = row_scope(manifest)?;
    let digest = hex::decode(digest_hex)
        .map_err(|_| ApiError::Unprocessable("invalid chunk digest".to_owned()))?;
    let row = sqlx::query(
        r#"
        SELECT chunk.* FROM snapshot_chunks chunk
        JOIN snapshot_manifest_chunks mapping
          ON mapping.key_scope_type = chunk.key_scope_type
         AND mapping.key_scope_id = chunk.key_scope_id
         AND mapping.plaintext_digest = chunk.plaintext_digest
        WHERE mapping.manifest_id = $1 AND chunk.plaintext_digest = $2
          AND chunk.status = 'ready' LIMIT 1
        "#,
    )
    .bind(manifest_id)
    .bind(digest)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("chunk {digest_hex} was not found")))?;
    let wrap_nonce: Option<Vec<u8>> = row.try_get("wrap_nonce")?;
    let plaintext = store
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
    if hex::encode(Sha256::digest(&plaintext)) != digest_hex {
        return Err(ApiError::Internal(anyhow::anyhow!(
            "decrypted chunk digest verification failed"
        )));
    }
    Ok(plaintext)
}

pub(super) async fn upload_file_from_bytes(
    pool: &PgPool,
    store: &ObjectStore,
    scope: KeyScope,
    template: &SnapshotFile,
    bytes: &[u8],
) -> Result<SnapshotFile, ApiError> {
    let mut chunks = Vec::new();
    for part in bytes.chunks(MAX_CHUNK_BYTES) {
        let digest = hex::encode(Sha256::digest(part));
        let ciphertext_size = persist_plaintext_chunk(pool, store, scope, &digest, part).await?;
        chunks.push(SnapshotChunk {
            digest,
            plaintext_size: part.len() as u64,
            ciphertext_size,
        });
    }
    Ok(SnapshotFile {
        path: template.path.clone(),
        size: bytes.len() as u64,
        mode: template.mode,
        modified_at: Utc::now(),
        chunks,
    })
}

async fn persist_plaintext_chunk(
    pool: &PgPool,
    store: &ObjectStore,
    scope: KeyScope,
    digest_hex: &str,
    bytes: &[u8],
) -> Result<u64, ApiError> {
    let digest = hex::decode(digest_hex)
        .map_err(|_| ApiError::Internal(anyhow::anyhow!("invalid generated digest")))?;
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("{}:{}:{digest_hex}", scope.kind(), scope.id()))
        .execute(&mut *tx)
        .await?;
    if let Some(size) = sqlx::query_scalar::<_, i64>(
        "SELECT ciphertext_size FROM snapshot_chunks WHERE key_scope_type = $1 AND key_scope_id = $2 AND plaintext_digest = $3 AND status = 'ready'",
    )
    .bind(scope.kind())
    .bind(scope.id())
    .bind(&digest)
    .fetch_optional(&mut *tx)
    .await?
    {
        tx.commit().await?;
        return u64::try_from(size).map_err(|error| ApiError::Internal(error.into()));
    }
    let (object_key, encrypted) = store.put_encrypted(scope, digest_hex, bytes).await?;
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
    .bind(digest)
    .bind(object_key)
    .bind(i64::try_from(bytes.len()).map_err(|error| ApiError::Internal(error.into()))?)
    .bind(i64::try_from(ciphertext_size).map_err(|error| ApiError::Internal(error.into()))?)
    .bind(encrypted.wrapped_data_key)
    .bind(encrypted.nonce.as_slice())
    .bind(encrypted.wrap_nonce.as_slice())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(ciphertext_size)
}

pub(super) async fn create_ready_manifest(
    pool: &PgPool,
    project_id: Uuid,
    user_id: Uuid,
    scope: KeyScope,
    files: &[SnapshotUploadFile],
) -> Result<Uuid, ApiError> {
    let total_bytes = files.iter().try_fold(0_u64, |sum, file| {
        sum.checked_add(file.size)
            .ok_or_else(|| ApiError::Unprocessable("merged snapshot size overflow".to_owned()))
    })?;
    validate_snapshot_files(total_bytes, files)?;
    let id = Uuid::new_v4();
    let mut tx = pool.begin().await?;
    sqlx::query(
        r#"
        INSERT INTO snapshot_manifests (
            id, project_id, created_by, key_scope_type, key_scope_id,
            encryption_key_id, total_bytes, file_count, manifest, status,
            expires_at, committed_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'ready', $10, now())
        "#,
    )
    .bind(id)
    .bind(project_id)
    .bind(user_id)
    .bind(scope.kind())
    .bind(scope.id())
    .bind(scope.key_id())
    .bind(i64::try_from(total_bytes).map_err(|error| ApiError::Internal(error.into()))?)
    .bind(i64::try_from(files.len()).map_err(|error| ApiError::Internal(error.into()))?)
    .bind(serde_json::to_value(files)?)
    // If the final project transaction loses an optimistic-concurrency race,
    // this unattached prepared manifest becomes collectible after one day.
    .bind(Utc::now() + Duration::days(1))
    .execute(&mut *tx)
    .await?;
    let mut mappings = Vec::new();
    for file in files {
        for (index, chunk) in file.chunks.iter().enumerate() {
            mappings.push((
                file.path.clone(),
                i32::try_from(index).map_err(|error| ApiError::Internal(error.into()))?,
                hex::decode(&chunk.digest)
                    .map_err(|_| ApiError::Internal(anyhow::anyhow!("invalid merged digest")))?,
            ));
        }
    }
    if !mappings.is_empty() {
        let mut query = QueryBuilder::<Postgres>::new(
            "INSERT INTO snapshot_manifest_chunks (manifest_id, path, chunk_index, key_scope_type, key_scope_id, plaintext_digest) ",
        );
        query.push_values(mappings, |mut values, (path, index, digest)| {
            values
                .push_bind(id)
                .push_bind(path)
                .push_bind(index)
                .push_bind(scope.kind())
                .push_bind(scope.id())
                .push_bind(digest);
        });
        query.build().execute(&mut *tx).await?;
        sqlx::query(
            r#"
            UPDATE snapshot_chunks chunk SET ref_count = chunk.ref_count + refs.count,
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
        .bind(id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(id)
}

pub(super) fn snapshot_to_upload_file(file: SnapshotFile) -> SnapshotUploadFile {
    SnapshotUploadFile {
        path: file.path,
        size: file.size,
        mode: file.mode,
        modified_at: file.modified_at,
        chunks: file
            .chunks
            .into_iter()
            .map(|chunk| SnapshotUploadChunk {
                digest: chunk.digest,
                plaintext_size: chunk.plaintext_size,
            })
            .collect(),
    }
}

pub(super) async fn release_manifest_references(
    pool: &PgPool,
    manifest_id: Uuid,
) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE snapshot_chunks chunk
        SET ref_count = GREATEST(chunk.ref_count - refs.count, 0),
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
    sqlx::query("DELETE FROM snapshot_manifest_chunks WHERE manifest_id = $1")
        .bind(manifest_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub(super) async fn accessible_manifest(
    pool: &PgPool,
    user_id: Uuid,
    manifest_id: Uuid,
    role: ProjectRole,
) -> Result<sqlx::postgres::PgRow, ApiError> {
    let row = sqlx::query("SELECT * FROM snapshot_manifests WHERE id = $1")
        .bind(manifest_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("snapshot {manifest_id} was not found")))?;
    organization::ensure_project_role(pool, user_id, row.try_get("project_id")?, role).await?;
    Ok(row)
}

pub(super) async fn project_scope(
    pool: &PgPool,
    project_id: Uuid,
    user_id: Uuid,
) -> Result<KeyScope, ApiError> {
    let row = sqlx::query(
        "SELECT privacy, owner_user_id, team_id FROM projects WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("project {project_id} was not found")))?;
    match row.try_get::<String, _>("privacy")?.as_str() {
        "private_local" => {
            let owner: Uuid = row.try_get("owner_user_id")?;
            if owner != user_id {
                return Err(ApiError::Unauthorized(
                    "private snapshots can only be uploaded by the project owner".to_owned(),
                ));
            }
            Ok(KeyScope::User(owner))
        }
        "team_managed" => Ok(KeyScope::Team(
            row.try_get::<Option<Uuid>, _>("team_id")?
                .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("team project has no team")))?,
        )),
        other => Err(ApiError::Internal(anyhow::anyhow!(
            "invalid project privacy in database: {other}"
        ))),
    }
}

pub(super) fn row_scope(row: &sqlx::postgres::PgRow) -> Result<KeyScope, ApiError> {
    let id: Uuid = row.try_get("key_scope_id")?;
    match row.try_get::<String, _>("key_scope_type")?.as_str() {
        "user" => Ok(KeyScope::User(id)),
        "team" => Ok(KeyScope::Team(id)),
        other => Err(ApiError::Internal(anyhow::anyhow!(
            "invalid snapshot key scope in database: {other}"
        ))),
    }
}

pub(super) async fn existing_digests(
    pool: &PgPool,
    scope: KeyScope,
    digests: &HashMap<String, u64>,
) -> Result<HashSet<String>, ApiError> {
    if digests.is_empty() {
        return Ok(HashSet::new());
    }
    let mut builder = QueryBuilder::<Postgres>::new(
        "SELECT encode(plaintext_digest, 'hex') AS digest FROM snapshot_chunks WHERE key_scope_type = ",
    );
    builder
        .push_bind(scope.kind())
        .push(" AND key_scope_id = ")
        .push_bind(scope.id())
        .push(" AND status = 'ready' AND plaintext_digest IN (");
    let mut separated = builder.separated(", ");
    for digest in digests.keys() {
        separated.push_bind(hex::decode(digest).map_err(|_| {
            ApiError::Internal(anyhow::anyhow!("validated digest failed to decode"))
        })?);
    }
    separated.push_unseparated(")");
    let rows = builder.build().fetch_all(pool).await?;
    rows.iter()
        .map(|row| row.try_get("digest").map_err(ApiError::from))
        .collect()
}

pub(super) async fn reserve_existing_chunks<'a>(
    tx: &mut Transaction<'_, Postgres>,
    manifest_id: Uuid,
    scope: KeyScope,
    digests: impl Iterator<Item = &'a String>,
) -> Result<HashSet<String>, ApiError> {
    let digests: Vec<&String> = digests.collect();
    if digests.is_empty() {
        return Ok(HashSet::new());
    }
    let mut builder = QueryBuilder::<Postgres>::new(
        "INSERT INTO snapshot_chunk_reservations (manifest_id, key_scope_type, key_scope_id, plaintext_digest) SELECT ",
    );
    builder
        .push_bind(manifest_id)
        .push(", chunk.key_scope_type, chunk.key_scope_id, chunk.plaintext_digest FROM snapshot_chunks chunk WHERE chunk.key_scope_type = ")
        .push_bind(scope.kind())
        .push(" AND chunk.key_scope_id = ")
        .push_bind(scope.id())
        .push(" AND chunk.status = 'ready' AND chunk.plaintext_digest IN (");
    let mut separated = builder.separated(", ");
    for digest in digests {
        separated.push_bind(hex::decode(digest).map_err(|_| {
            ApiError::Internal(anyhow::anyhow!("validated digest failed to decode"))
        })?);
    }
    separated.push_unseparated(
        ") ON CONFLICT DO NOTHING RETURNING encode(plaintext_digest, 'hex') AS digest",
    );
    let rows = builder.build().fetch_all(&mut **tx).await?;
    rows.iter()
        .map(|row| row.try_get("digest").map_err(ApiError::from))
        .collect()
}

pub(super) async fn insert_chunk_reservation(
    tx: &mut Transaction<'_, Postgres>,
    manifest_id: Uuid,
    scope: KeyScope,
    digest: &[u8],
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO snapshot_chunk_reservations (
            manifest_id, key_scope_type, key_scope_id, plaintext_digest
        ) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING
        "#,
    )
    .bind(manifest_id)
    .bind(scope.kind())
    .bind(scope.id())
    .bind(digest)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(super) struct StoredChunk {
    pub(super) plaintext_size: u64,
    pub(super) ciphertext_size: u64,
}

pub(super) async fn fetch_chunk_rows<'a>(
    tx: &mut Transaction<'_, Postgres>,
    scope: KeyScope,
    digests: impl Iterator<Item = &'a String>,
) -> Result<HashMap<String, StoredChunk>, ApiError> {
    let digests: Vec<&String> = digests.collect();
    if digests.is_empty() {
        return Ok(HashMap::new());
    }
    let mut builder = QueryBuilder::<Postgres>::new(
        "SELECT encode(plaintext_digest, 'hex') AS digest, plaintext_size, ciphertext_size FROM snapshot_chunks WHERE key_scope_type = ",
    );
    builder
        .push_bind(scope.kind())
        .push(" AND key_scope_id = ")
        .push_bind(scope.id())
        .push(" AND status = 'ready' AND plaintext_digest IN (");
    let mut separated = builder.separated(", ");
    for digest in digests {
        separated.push_bind(hex::decode(digest).map_err(|_| {
            ApiError::Internal(anyhow::anyhow!("validated digest failed to decode"))
        })?);
    }
    separated.push_unseparated(")");
    let rows = builder.build().fetch_all(&mut **tx).await?;
    rows.into_iter()
        .map(|row| {
            Ok((
                row.try_get("digest")?,
                StoredChunk {
                    plaintext_size: u64::try_from(row.try_get::<i64, _>("plaintext_size")?)
                        .map_err(|error| ApiError::Internal(error.into()))?,
                    ciphertext_size: u64::try_from(row.try_get::<i64, _>("ciphertext_size")?)
                        .map_err(|error| ApiError::Internal(error.into()))?,
                },
            ))
        })
        .collect()
}

pub(super) async fn snapshot_from_row(
    pool: &PgPool,
    row: &sqlx::postgres::PgRow,
) -> Result<SnapshotManifest, ApiError> {
    let manifest_id: Uuid = row.try_get("id")?;
    let upload_files: Vec<SnapshotUploadFile> = serde_json::from_value(row.try_get("manifest")?)?;
    let mappings = sqlx::query(
        r#"
        SELECT mapping.path, mapping.chunk_index,
               encode(chunk.plaintext_digest, 'hex') AS digest,
               chunk.plaintext_size, chunk.ciphertext_size
        FROM snapshot_manifest_chunks mapping
        JOIN snapshot_chunks chunk
          ON chunk.key_scope_type = mapping.key_scope_type
         AND chunk.key_scope_id = mapping.key_scope_id
         AND chunk.plaintext_digest = mapping.plaintext_digest
        WHERE mapping.manifest_id = $1
        ORDER BY mapping.path, mapping.chunk_index
        "#,
    )
    .bind(manifest_id)
    .fetch_all(pool)
    .await?;
    let mut by_path: HashMap<String, Vec<SnapshotChunk>> = HashMap::new();
    for mapping in mappings {
        by_path
            .entry(mapping.try_get("path")?)
            .or_default()
            .push(SnapshotChunk {
                digest: mapping.try_get("digest")?,
                plaintext_size: u64::try_from(mapping.try_get::<i64, _>("plaintext_size")?)
                    .map_err(|error| ApiError::Internal(error.into()))?,
                ciphertext_size: u64::try_from(mapping.try_get::<i64, _>("ciphertext_size")?)
                    .map_err(|error| ApiError::Internal(error.into()))?,
            });
    }
    let files = upload_files
        .into_iter()
        .map(|file| SnapshotFile {
            chunks: by_path.remove(&file.path).unwrap_or_default(),
            path: file.path,
            size: file.size,
            mode: file.mode,
            modified_at: file.modified_at,
        })
        .collect();
    Ok(SnapshotManifest {
        schema_version: SCHEMA_VERSION,
        id: manifest_id,
        project_id: row.try_get("project_id")?,
        total_bytes: u64::try_from(row.try_get::<i64, _>("total_bytes")?)
            .map_err(|error| ApiError::Internal(error.into()))?,
        files,
        encryption_key_id: row.try_get("encryption_key_id")?,
        created_at: row.try_get("created_at")?,
        expires_at: row.try_get("expires_at")?,
    })
}
