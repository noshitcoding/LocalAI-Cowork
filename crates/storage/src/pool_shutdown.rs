use sqlx::AnyPool;

pub(super) async fn close(pool: &AnyPool) {
    loop {
        pool.close().await;
        // A returning connection can enter the idle queue after the close pass
        // has drained it. Closed pools cannot create new connections; drain
        // again until all counted connections have completed their shutdown.
        if pool.size() == 0 {
            return;
        }
        tokio::task::yield_now().await;
    }
}

#[cfg(test)]
mod tests {
    use sqlx::any::AnyPoolOptions;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use tokio::sync::Notify;
    use tokio::time::{Duration, timeout};

    #[tokio::test]
    async fn store_close_waits_for_a_connection_returning_during_close() {
        sqlx::any::install_default_drivers();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("close-race.sqlite");
        let encoded = percent_encoding::utf8_percent_encode(
            path.to_str().unwrap(),
            percent_encoding::NON_ALPHANUMERIC,
        );
        let armed = Arc::new(AtomicBool::new(false));
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let hook = (armed.clone(), entered.clone(), release.clone());
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .after_release(move |_, _| {
                let (armed, entered, release) = hook.clone();
                Box::pin(async move {
                    if armed.swap(false, Ordering::SeqCst) {
                        entered.notify_one();
                        release.notified().await;
                    }
                    Ok(true)
                })
            })
            .connect(&format!("sqlite:{encoded}?mode=rwc"))
            .await
            .unwrap();
        let store = crate::Store {
            pool: pool.clone(),
            sqlite: true,
            workflow_clock: crate::workflow_clocks::WorkflowClock::system(),
        };
        let connection = pool.acquire().await.unwrap();
        armed.store(true, Ordering::SeqCst);
        drop(connection);
        timeout(Duration::from_secs(5), entered.notified())
            .await
            .unwrap();
        let closing = store.clone();
        let close = tokio::spawn(async move { closing.close().await });
        timeout(Duration::from_secs(5), async {
            while !pool.is_closed() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        release.notify_one();
        timeout(Duration::from_secs(5), close)
            .await
            .unwrap()
            .unwrap();
        let observed = (pool.size(), pool.num_idle());
        eprintln!(
            "Pool after Store.close: size={}, idle={}",
            observed.0, observed.1
        );
        // Preserve the assertion evidence while still cleaning the RED fixture.
        pool.close().await;
        drop(store);
        drop(pool);
        directory.close().unwrap();
        // SQLx num_idle can remain stale after close; size is the drain gate.
        assert_eq!(observed.0, 0, "Store.close left a counted connection open");
    }
}
