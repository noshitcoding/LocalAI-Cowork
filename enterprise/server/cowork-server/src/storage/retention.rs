//! Collection of expired or unattached manifests and unreferenced encrypted chunks.

use crate::{
    error::ApiError,
    storage::{object_store::ObjectStore, snapshot_repository::release_manifest_references},
};
use sqlx::{PgPool, Row};
use uuid::Uuid;

pub async fn garbage_collect(
    pool: &PgPool,
    store: &ObjectStore,
    limit: i64,
) -> Result<usize, ApiError> {
    let abandoned = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM snapshot_manifests WHERE status = 'uploading' AND upload_expires_at <= now() ORDER BY upload_expires_at LIMIT $1",
    )
    .bind(limit.clamp(1, 100))
    .fetch_all(pool)
    .await?;
    for manifest_id in abandoned {
        let mut tx = pool.begin().await?;
        sqlx::query("DELETE FROM snapshot_chunk_reservations WHERE manifest_id = $1")
            .bind(manifest_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE snapshot_manifests SET status = 'expired' WHERE id = $1 AND status = 'uploading'")
            .bind(manifest_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    }
    let expired = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT manifest.id FROM snapshot_manifests manifest
        WHERE manifest.status = 'ready' AND manifest.expires_at <= now()
          AND NOT EXISTS(
              SELECT 1 FROM project_versions version
              WHERE version.snapshot_manifest_id = manifest.id
          )
        ORDER BY manifest.expires_at LIMIT $1
        "#,
    )
    .bind(limit.clamp(1, 100))
    .fetch_all(pool)
    .await?;
    for manifest_id in expired {
        release_manifest_references(pool, manifest_id).await?;
        sqlx::query("UPDATE snapshot_manifests SET status = 'expired' WHERE id = $1")
            .bind(manifest_id)
            .execute(pool)
            .await?;
    }
    let rows = sqlx::query(
        r#"
        UPDATE snapshot_chunks SET status = 'deleting'
        WHERE (key_scope_type, key_scope_id, plaintext_digest) IN (
            SELECT key_scope_type, key_scope_id, plaintext_digest
            FROM snapshot_chunks chunk
            WHERE ref_count = 0 AND status = 'ready'
              AND NOT EXISTS (
                  SELECT 1 FROM snapshot_chunk_reservations reservation
                  WHERE reservation.key_scope_type = chunk.key_scope_type
                    AND reservation.key_scope_id = chunk.key_scope_id
                    AND reservation.plaintext_digest = chunk.plaintext_digest
              )
            ORDER BY last_referenced_at LIMIT $1 FOR UPDATE SKIP LOCKED
        ) RETURNING key_scope_type, key_scope_id, plaintext_digest, object_key
        "#,
    )
    .bind(limit.clamp(1, 1000))
    .fetch_all(pool)
    .await?;
    let mut deleted = 0;
    for row in rows {
        let object_key: String = row.try_get("object_key")?;
        match store.delete(&object_key).await {
            Ok(()) => {
                sqlx::query(
                    "DELETE FROM snapshot_chunks WHERE key_scope_type = $1 AND key_scope_id = $2 AND plaintext_digest = $3 AND ref_count = 0 AND status = 'deleting'",
                )
                .bind(row.try_get::<String, _>("key_scope_type")?)
                .bind(row.try_get::<Uuid, _>("key_scope_id")?)
                .bind(row.try_get::<Vec<u8>, _>("plaintext_digest")?)
                .execute(pool)
                .await?;
                deleted += 1;
            }
            Err(error) => {
                tracing::warn!(?error, %object_key, "failed to delete unreferenced object-store chunk");
                sqlx::query(
                    "UPDATE snapshot_chunks SET status = 'ready' WHERE object_key = $1 AND status = 'deleting'",
                )
                .bind(object_key)
                .execute(pool)
                .await?;
            }
        }
    }
    Ok(deleted)
}
