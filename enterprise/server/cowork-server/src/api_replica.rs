//! Fail closed while executor commands and reverse desktop channels are local
//! to one API process. Workers and the management service do not take this lock.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::{bail, Context, Result};
use serde::Serialize;
use sqlx::{Connection, PgConnection};
use tokio::sync::{watch, Mutex};

// A session advisory lock is scoped to the PostgreSQL database. It must use a
// direct/session-pooled connection, never a transaction-pooling connection.
const LOCK_KEY: i64 = 0x4357_4b41_5049_0001;
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct ApiReplicaGuard {
    inner: Arc<Inner>,
}

struct Inner {
    connection: Mutex<Option<PgConnection>>,
    backend_pid: i32,
    active: AtomicBool,
    lost: watch::Sender<bool>,
}

#[derive(Debug, Serialize)]
pub struct ApiReplicaStatus {
    pub mode: &'static str,
    pub multiple_api_replicas_supported: bool,
    pub ownership_healthy: bool,
    pub executor_connection_routing: &'static str,
    pub desktop_connection_routing: &'static str,
}

impl ApiReplicaGuard {
    pub async fn acquire(database_url: &str) -> Result<Self> {
        let mut connection =
            tokio::time::timeout(Duration::from_secs(10), PgConnection::connect(database_url))
                .await
                .context("API ownership connection timed out")?
                .context("API ownership connection failed")?;
        let (backend_pid, acquired) = tokio::time::timeout(
            PROBE_TIMEOUT,
            sqlx::query_as::<_, (i32, bool)>("SELECT pg_backend_pid(), pg_try_advisory_lock($1)")
                .bind(LOCK_KEY)
                .fetch_one(&mut connection),
        )
        .await
        .context("API ownership lock timed out")?
        .context("API ownership lock failed")?;
        if !acquired {
            bail!("another API process already owns this PostgreSQL database; executor and desktop connections currently require exactly one API process (workers and management remain independent)");
        }
        let (lost, _) = watch::channel(false);
        let guard = Self {
            inner: Arc::new(Inner {
                connection: Mutex::new(Some(connection)),
                backend_pid,
                active: AtomicBool::new(true),
                lost,
            }),
        };
        // Also rejects transaction pooling that changes backend/session ownership.
        guard.ensure_active().await?;
        let weak = Arc::downgrade(&guard.inner);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let Some(inner) = weak.upgrade() else { return };
                if (ApiReplicaGuard { inner }).ensure_active().await.is_err() {
                    tracing::error!(
                        "API process lost exclusive database ownership; serving must stop"
                    );
                    return;
                }
            }
        });
        Ok(guard)
    }

    /// Probe the original session, never reconnect or silently reclaim a lost
    /// lock. Every executor operation and API readiness use this proof.
    pub async fn ensure_active(&self) -> Result<()> {
        if !self.inner.active.load(Ordering::Acquire) {
            bail!("API process no longer owns exclusive executor/desktop connection routing");
        }
        let mut connection = self.inner.connection.lock().await;
        if !self.inner.active.load(Ordering::Acquire) {
            bail!("API process no longer owns exclusive executor/desktop connection routing");
        }
        let owns = match connection.as_mut() {
            Some(connection) => tokio::time::timeout(PROBE_TIMEOUT, sqlx::query_scalar::<_, bool>(
                "SELECT pg_backend_pid() = $1 AND EXISTS (SELECT 1 FROM pg_locks WHERE locktype = 'advisory' AND pid = pg_backend_pid() AND classid::bigint = $2 AND objid::bigint = $3 AND objsubid = 1 AND granted)"
            ).bind(self.inner.backend_pid).bind(LOCK_KEY >> 32).bind(LOCK_KEY & 0xffff_ffff)
                .fetch_one(connection)).await.ok().and_then(|result| result.ok()).unwrap_or(false),
            None => false,
        };
        if !owns {
            self.inner.active.store(false, Ordering::Release);
            self.inner.lost.send_replace(true);
            // Drop the failed connection. It must not be used or reconnected.
            connection.take();
            bail!("API process lost exclusive PostgreSQL ownership; restart after the current owner stops");
        }
        Ok(())
    }

    pub async fn wait_until_lost(&self) {
        let mut lost = self.inner.lost.subscribe();
        loop {
            if *lost.borrow_and_update() {
                return;
            }
            if lost.changed().await.is_err() {
                return;
            }
        }
    }

    /// Stop interactive admission and close relays during graceful shutdown.
    /// Retain the lock until all guard owners (including relay cleanup) drop;
    /// another API must not acquire ownership while old relays drain.
    pub fn begin_draining(&self) {
        self.inner.active.store(false, Ordering::Release);
        self.inner.lost.send_replace(true);
    }

    pub fn status(&self) -> ApiReplicaStatus {
        ApiReplicaStatus {
            mode: "single_api_process",
            multiple_api_replicas_supported: false,
            ownership_healthy: self.inner.active.load(Ordering::Acquire),
            executor_connection_routing: "process_local",
            desktop_connection_routing: "process_local",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lost_guard_is_permanently_fenced_without_database_access() {
        let (lost, _) = watch::channel(true);
        let guard = ApiReplicaGuard {
            inner: Arc::new(Inner {
                connection: Mutex::new(None),
                backend_pid: 0,
                active: AtomicBool::new(false),
                lost,
            }),
        };
        assert!(guard.ensure_active().await.is_err());
        tokio::time::timeout(Duration::from_millis(50), guard.wait_until_lost())
            .await
            .unwrap();
        assert!(!guard.status().multiple_api_replicas_supported);
        assert!(!guard.status().ownership_healthy);
    }

    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to an isolated PostgreSQL test database"]
    async fn postgres_refuses_second_api_and_fences_lost_owner() {
        let url = std::env::var("COWORK_TEST_DATABASE_URL").expect("isolated test database URL");
        let first = ApiReplicaGuard::acquire(&url).await.unwrap();
        let error = ApiReplicaGuard::acquire(&url)
            .await
            .err()
            .expect("second API must refuse startup");
        assert!(error.to_string().contains("another API process"));
        // Independent worker/management database sessions remain usable.
        let mut independent = PgConnection::connect(&url).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i32>("SELECT 1")
                .fetch_one(&mut independent)
                .await
                .unwrap(),
            1
        );
        // Simulate session ownership loss without terminating unrelated backends.
        let mut original = first.inner.connection.lock().await;
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(LOCK_KEY)
            .execute(original.as_mut().unwrap())
            .await
            .unwrap();
        drop(original);
        assert!(first.ensure_active().await.is_err());
        assert!(!first.status().ownership_healthy);
        let replacement = ApiReplicaGuard::acquire(&url).await.unwrap();
        assert!(replacement.status().ownership_healthy);
        assert!(first.ensure_active().await.is_err());
    }
}
