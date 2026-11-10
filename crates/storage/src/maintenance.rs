//! Owner maintenance keeps immutable run/audit evidence and actor data untouched.
use super::*;
impl Store {
    pub fn database_backend(&self) -> &'static str {
        if self.sqlite { "sqlite" } else { "postgres" }
    }
    /// SQLite itself produces a transactionally consistent image, including WAL data.
    pub async fn database_backup_sqlite(&self, path: &Path) -> Result<()> {
        if !self.sqlite {
            return Err(Error::Validation(
                "SQLite backup cannot export PostgreSQL".into(),
            ));
        }
        if !path.is_absolute() || path.exists() {
            return Err(Error::Forbidden);
        }
        sqlx::query("VACUUM INTO $1")
            .bind(path.to_str().ok_or(Error::Forbidden)?)
            .execute(&self.pool)
            .await
            .map_err(storage_error)?;
        Ok(())
    }
    /// Retention archives inactive chats. It intentionally preserves history for audit/export.
    pub async fn archive_retained_chats(&self, days: u32, at: i64) -> Result<u64> {
        self.archive_retained_chats_guarded(days, at, |_| Some(()))
            .await
    }
    /// A successful guard is held until commit so host work cannot start while its
    /// chat is archived. Busy/unavailable workspaces are skipped conservatively.
    pub async fn archive_retained_chats_guarded<G: Send>(
        &self,
        days: u32,
        at: i64,
        mut guard: impl FnMut(&str) -> Option<G> + Send,
    ) -> Result<u64> {
        let mut held = Vec::new();
        if days == 0 {
            return Ok(0);
        }
        if days > 36500 {
            return Err(Error::Validation("Invalid retention period".into()));
        }
        let cutoff = at.saturating_sub(i64::from(days) * 86_400_000);
        let mut tx = crate::productivity::begin_productivity(self).await?;
        let mut count = 0;
        let mut cursor = String::new();
        'pages: loop {
            let rows=sqlx::query("SELECT o.id FROM orders o LEFT JOIN native_chat_management m ON m.order_id=o.id WHERE o.id>$1 AND COALESCE(m.archived,0)=0 AND NOT EXISTS(SELECT 1 FROM native_chat_retirements retired WHERE retired.order_id=o.id) AND NOT EXISTS(SELECT 1 FROM runs r WHERE r.order_id=o.id AND r.state NOT IN ('completed','failed','canceled','expired')) ORDER BY o.id LIMIT 256").bind(&cursor).fetch_all(&mut *tx).await.map_err(storage_error)?;
            if rows.is_empty() {
                break;
            }
            for row in rows {
                cursor = row.try_get::<String, _>("id").map_err(storage_error)?;
                sqlx::query("UPDATE orders SET revision=revision WHERE id=$1")
                    .bind(&cursor)
                    .execute(&mut *tx)
                    .await
                    .map_err(storage_error)?;
                let raw: String = sqlx::query_scalar("SELECT payload FROM orders WHERE id=$1")
                    .bind(&cursor)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(storage_error)?;
                let order: Order = parse(&raw)?;
                if !order.draft.trim().is_empty() {
                    continue;
                }
                let active:i64=sqlx::query_scalar("SELECT COUNT(*) FROM runs WHERE order_id=$1 AND state NOT IN ('completed','failed','canceled','expired')").bind(&order.id).fetch_one(&mut *tx).await.map_err(storage_error)?;
                if active > 0 {
                    continue;
                }
                let latest: Option<i64> =
                    sqlx::query_scalar("SELECT MAX(updated_at) FROM runs WHERE order_id=$1")
                        .bind(&order.id)
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(storage_error)?;
                if latest.unwrap_or(order.created_at).max(order.created_at) >= cutoff {
                    continue;
                }
                // A configured schedule or task continues to own its chat; never archive under it.
                let tasks: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM native_workspace_tasks WHERE order_id=$1 AND archived=0",
                )
                .bind(&order.id)
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
                if tasks > 0 {
                    continue;
                }
                let schedules =
                    sqlx::query("SELECT payload FROM productivity_schedules WHERE project_id=$1")
                        .bind(&order.project_id)
                        .fetch_all(&mut *tx)
                        .await
                        .map_err(storage_error)?;
                if schedules.iter().any(|r| {
                    r.try_get::<String, _>("payload")
                        .ok()
                        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                        .is_some_and(|v| v["template"]["order_id"] == order.id)
                }) {
                    continue;
                }
                let Some(lease) = guard(&order.id) else {
                    continue;
                };
                held.push(lease);
                sqlx::query("INSERT INTO native_chat_management(order_id,archived) VALUES($1,1) ON CONFLICT(order_id) DO UPDATE SET archived=1").bind(&order.id).execute(&mut *tx).await.map_err(storage_error)?;
                count += 1;
                if count >= 1000 {
                    break 'pages;
                }
            }
        }
        tx.commit().await.map_err(storage_error)?;
        drop(held);
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn retention_archives_old_inactive_chats_preserving_history_and_drafts() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::sqlite(&temp.path().join("db.sqlite3"))
            .await
            .unwrap();
        let project = store
            .create_project(CreateProject {
                name: "Retention".into(),
            })
            .await
            .unwrap();
        let old = store
            .create_order(CreateOrder {
                project_id: project.id.clone(),
                title: "Old".into(),
            })
            .await
            .unwrap();
        let draft = store
            .create_order(CreateOrder {
                project_id: project.id.clone(),
                title: "Draft".into(),
            })
            .await
            .unwrap();
        for mut order in [old.clone(), draft.clone()] {
            order.created_at = now() - 90 * 86_400_000;
            if order.id == draft.id {
                order.draft = "Unsaved work".into();
            }
            sqlx::query("UPDATE orders SET payload=$1 WHERE id=$2")
                .bind(json_string(&order).unwrap())
                .bind(&order.id)
                .execute(&store.pool)
                .await
                .unwrap();
        }
        assert_eq!(store.archive_retained_chats(0, now()).await.unwrap(), 0);
        assert_eq!(store.archive_retained_chats(30, now()).await.unwrap(), 1);
        assert_eq!(store.archive_retained_chats(30, now()).await.unwrap(), 0);
        assert!(store.order(&old.id).await.is_ok());
        assert_eq!(store.archived_chats(&project.id).await.unwrap().len(), 1);
        assert_eq!(store.order(&draft.id).await.unwrap().draft, "Unsaved work");
        store.close().await;
    }
    #[tokio::test]
    async fn retention_skips_busy_workspace_and_keeps_guard_until_commit() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let project = store
            .create_project(CreateProject {
                name: "Guarded retention".into(),
            })
            .await
            .unwrap();
        let order = store
            .create_order(CreateOrder {
                project_id: project.id,
                title: "Old inactive".into(),
            })
            .await
            .unwrap();
        let later = now() + 3 * 86_400_000;
        assert_eq!(
            store
                .archive_retained_chats_guarded::<()>(1, later, |_| None)
                .await
                .unwrap(),
            0
        );
        assert!(
            store
                .archived_chats(&order.project_id)
                .await
                .unwrap()
                .is_empty()
        );
        let held = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        struct Guard(std::sync::Arc<std::sync::atomic::AtomicBool>);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.store(false, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let state = held.clone();
        assert_eq!(
            store
                .archive_retained_chats_guarded(1, later, move |_| {
                    assert!(!state.swap(true, std::sync::atomic::Ordering::SeqCst));
                    Some(Guard(state.clone()))
                })
                .await
                .unwrap(),
            1
        );
        assert!(!held.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(
            store.archived_chats(&order.project_id).await.unwrap().len(),
            1
        );
    }
}
