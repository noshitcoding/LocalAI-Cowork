//! Read-only update admission under the local database writer lock.
use super::*;

impl Store {
    pub async fn native_update_preflight(&self) -> Result<()> {
        if !self.sqlite {
            return Err(Error::Forbidden);
        }
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        // Acquire the SQLite writer lock before examining all execution queues.
        sqlx::query("UPDATE schema_migrations SET checksum=checksum WHERE version=1")
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        for query in [
            "SELECT COUNT(*) FROM runs WHERE state NOT IN ('completed','failed','canceled','expired','interrupted')",
            "SELECT COUNT(*) FROM team_runs WHERE status IN ('queued','running')",
            "SELECT COUNT(*) FROM android_scoped_runs WHERE state IN ('queued','leased','running')",
            "SELECT COUNT(*) FROM offline_local_runs WHERE state IN ('running','awaiting_approval')",
        ] {
            let count: i64 = sqlx::query_scalar(query)
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
            if count != 0 {
                return Err(Error::WorkspaceBusy);
            }
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(())
    }
}
