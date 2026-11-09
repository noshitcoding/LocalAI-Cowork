//! PostgreSQL notifications are hints. The durable run_events log remains the
//! only source of event contents, ordering, and replay cursors.

use std::{sync::Arc, time::Duration};

use serde::Serialize;
use sqlx::postgres::PgListener;
use tokio::{sync::broadcast, task::JoinHandle};
use uuid::Uuid;

pub const CHANNEL: &str = "cowork_run_events";
// Authorization remains frequent; durable replay reads need not poll as often.
pub const AUTHORIZATION_INTERVAL: Duration = Duration::from_millis(750);
pub const REPLAY_FALLBACK_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct EventNotifications {
    inner: Arc<Inner>,
}

struct Inner {
    hints: broadcast::Sender<Hint>,
    connected: Arc<std::sync::atomic::AtomicBool>,
    listener: JoinHandle<()>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.listener.abort();
    }
}

#[derive(Clone, Copy, Debug)]
enum Hint {
    Run(Uuid),
    ReplayAll,
}

#[derive(Debug, Serialize)]
pub struct EventDeliveryStatus {
    pub durable_replay: &'static str,
    pub wakeups: &'static str,
    pub listener_connected: bool,
    pub auth_recheck_ms: u64,
    pub replay_fallback_ms: u64,
}

impl EventNotifications {
    pub fn start(database_url: String) -> Self {
        let (hints, _) = broadcast::channel(1024);
        let connected = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let listener = tokio::spawn(listen(database_url, hints.clone(), connected.clone()));
        Self {
            inner: Arc::new(Inner {
                hints,
                connected,
                listener,
            }),
        }
    }

    /// Subscribe before the first durable read to close the read/wait race.
    pub fn subscribe(&self, run_id: Uuid) -> EventSubscription {
        EventSubscription {
            run_id,
            receiver: self.inner.hints.subscribe(),
        }
    }

    pub fn status(&self) -> EventDeliveryStatus {
        EventDeliveryStatus {
            durable_replay: "postgresql_run_events",
            wakeups: "postgresql_listen_notify",
            listener_connected: self
                .inner
                .connected
                .load(std::sync::atomic::Ordering::Acquire),
            auth_recheck_ms: AUTHORIZATION_INTERVAL.as_millis() as u64,
            replay_fallback_ms: REPLAY_FALLBACK_INTERVAL.as_millis() as u64,
        }
    }
}

pub struct EventSubscription {
    run_id: Uuid,
    receiver: broadcast::Receiver<Hint>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Wakeup {
    Replay,
    AuthorizationCheck,
}

impl EventSubscription {
    pub async fn wait_until(&mut self, replay_deadline: tokio::time::Instant) -> Wakeup {
        let remaining = replay_deadline.saturating_duration_since(tokio::time::Instant::now());
        let wakeup = self.wait_for(AUTHORIZATION_INTERVAL.min(remaining)).await;
        if wakeup == Wakeup::AuthorizationCheck && tokio::time::Instant::now() >= replay_deadline {
            Wakeup::Replay
        } else {
            wakeup
        }
    }

    #[cfg(test)]
    pub async fn wait(&mut self) -> Wakeup {
        self.wait_for(AUTHORIZATION_INTERVAL).await
    }

    async fn wait_for(&mut self, fallback: Duration) -> Wakeup {
        let deadline = tokio::time::Instant::now() + fallback;
        loop {
            match tokio::time::timeout_at(deadline, self.receiver.recv()).await {
                Ok(Ok(Hint::Run(run_id))) if run_id != self.run_id => continue,
                Ok(Err(broadcast::error::RecvError::Closed)) => {
                    // A stopped listener must not turn the stream into a busy loop.
                    tokio::time::sleep_until(deadline).await;
                    return Wakeup::AuthorizationCheck;
                }
                // Lag, reconnect and matching hints force durable replay;
                // an idle timer only asks the route to recheck authorization.
                // Never advance the cursor based on a notification.
                Err(_) => return Wakeup::AuthorizationCheck,
                _ => return Wakeup::Replay,
            }
        }
    }
}

async fn listen(
    database_url: String,
    hints: broadcast::Sender<Hint>,
    connected: Arc<std::sync::atomic::AtomicBool>,
) {
    loop {
        let connection = tokio::time::timeout(Duration::from_secs(10), async {
            let mut listener = PgListener::connect(&database_url).await?;
            listener.listen(CHANNEL).await?;
            // Make gaps explicit instead of hiding a reconnect inside recv().
            listener.eager_reconnect(false);
            Ok::<_, sqlx::Error>(listener)
        })
        .await;
        let mut listener = match connection {
            Ok(Ok(listener)) => listener,
            _ => {
                connected.store(false, std::sync::atomic::Ordering::Release);
                // Do not log the connection string or notification payloads.
                tracing::warn!(
                    "run-event notification listener unavailable; durable polling continues"
                );
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };
        connected.store(true, std::sync::atomic::Ordering::Release);
        let _ = hints.send(Hint::ReplayAll);
        while let Ok(Some(notification)) = listener.try_recv().await {
            if notification.channel() == CHANNEL {
                if let Ok(run_id) = Uuid::parse_str(notification.payload()) {
                    let _ = hints.send(Hint::Run(run_id));
                }
            }
        }
        connected.store(false, std::sync::atomic::Ordering::Release);
        let _ = hints.send(Hint::ReplayAll);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subscription(capacity: usize) -> (broadcast::Sender<Hint>, EventSubscription) {
        let (sender, receiver) = broadcast::channel(capacity);
        (
            sender,
            EventSubscription {
                run_id: Uuid::new_v4(),
                receiver,
            },
        )
    }

    #[tokio::test]
    async fn matching_hint_queued_before_wait_wakes_replay() {
        let (sender, mut subscription) = subscription(16);
        sender.send(Hint::Run(subscription.run_id)).unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_millis(100), subscription.wait())
                .await
                .unwrap(),
            Wakeup::Replay
        );
    }

    #[tokio::test]
    async fn dropped_notification_and_unrelated_hints_have_bounded_fallback() {
        let (sender, mut subscription) = subscription(16);
        sender.send(Hint::Run(Uuid::new_v4())).unwrap();
        let started = tokio::time::Instant::now();
        assert_eq!(
            subscription.wait_for(Duration::from_millis(20)).await,
            Wakeup::AuthorizationCheck
        );
        assert!(started.elapsed() >= Duration::from_millis(20));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn lagged_subscriber_wakes_without_trusting_lost_hints() {
        let (sender, mut subscription) = subscription(1);
        for _ in 0..10 {
            sender.send(Hint::Run(Uuid::new_v4())).unwrap();
        }
        assert_eq!(
            tokio::time::timeout(Duration::from_millis(100), subscription.wait())
                .await
                .unwrap(),
            Wakeup::Replay
        );
    }

    #[tokio::test]
    async fn closed_listener_waits_for_fallback_instead_of_spinning() {
        let (sender, mut subscription) = subscription(1);
        drop(sender);
        let started = tokio::time::Instant::now();
        assert_eq!(
            subscription.wait_for(Duration::from_millis(20)).await,
            Wakeup::AuthorizationCheck
        );
        assert!(started.elapsed() >= Duration::from_millis(20));
    }

    #[tokio::test]
    async fn missed_notification_replays_at_deadline_without_a_hint() {
        let (_sender, mut subscription) = subscription(1);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(20);
        assert_eq!(subscription.wait_until(deadline).await, Wakeup::Replay);
        assert!(tokio::time::Instant::now() >= deadline);
    }

    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to an isolated PostgreSQL test database"]
    async fn postgres_trigger_notifies_commit_only_and_contains_no_event_payload() {
        use sqlx::Connection;
        let url = std::env::var("COWORK_TEST_DATABASE_URL").expect("isolated test database URL");
        let mut writer = sqlx::PgConnection::connect(&url).await.unwrap();
        let mut listener = PgListener::connect(&url).await.unwrap();
        listener.listen(CHANNEL).await.unwrap();
        sqlx::query("CREATE TEMP TABLE run_events (run_id uuid, sequence bigint, payload jsonb)")
            .execute(&mut writer)
            .await
            .unwrap();
        let trigger = include_str!("../migrations/0051_run_event_notifications.sql")
            .replace("notify_cowork_run_event", "pg_temp.notify_cowork_run_event");
        sqlx::raw_sql(&trigger).execute(&mut writer).await.unwrap();
        let run_id = Uuid::new_v4();
        let mut transaction = writer.begin().await.unwrap();
        sqlx::query(
            "INSERT INTO run_events VALUES ($1,1,'{\"secret\":\"must-stay-in-durable-log\"}')",
        )
        .bind(run_id)
        .execute(&mut *transaction)
        .await
        .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.recv())
                .await
                .is_err()
        );
        transaction.rollback().await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.recv())
                .await
                .is_err()
        );
        let mut transaction = writer.begin().await.unwrap();
        sqlx::query(
            "INSERT INTO run_events VALUES ($1,1,'{\"secret\":\"must-stay-in-durable-log\"}')",
        )
        .bind(run_id)
        .execute(&mut *transaction)
        .await
        .unwrap();
        transaction.commit().await.unwrap();
        let notification = tokio::time::timeout(Duration::from_secs(2), listener.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(notification.payload(), run_id.to_string());
        assert_eq!(notification.channel(), CHANNEL);
        assert!(!notification.payload().contains("secret"));
    }

    #[tokio::test]
    #[ignore = "requires COWORK_TEST_DATABASE_URL pointing to an isolated PostgreSQL test database"]
    async fn postgres_durable_replay_keeps_cursor_across_batches_without_notification() {
        let url = std::env::var("COWORK_TEST_DATABASE_URL").expect("isolated test database URL");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        // One pooled session preserves this temporary table for the real DB
        // replay function. No listener is running and no trigger is installed.
        sqlx::query("CREATE TEMP TABLE run_events (run_id uuid, sequence bigint, event_id uuid, kind text, payload jsonb, created_at timestamptz)")
            .execute(&pool).await.unwrap();
        let run_id = Uuid::new_v4();
        sqlx::query("INSERT INTO run_events SELECT $1, seq, md5(seq::text)::uuid, 'model_delta', jsonb_build_object('text', seq::text), now() FROM generate_series(1,600) seq")
            .bind(run_id).execute(&pool).await.unwrap();
        let mut cursor = 125; // Reconnect after receiving Last-Event-ID: 125.
        let mut sequences = Vec::new();
        loop {
            let events = crate::db::get_run_events(&pool, run_id, cursor, 250)
                .await
                .unwrap();
            if events.is_empty() {
                break;
            }
            for event in events {
                assert!(event.sequence > cursor);
                cursor = event.sequence;
                sequences.push(event.sequence);
            }
        }
        assert_eq!(sequences, (126..=600).collect::<Vec<_>>());
        assert!(crate::db::get_run_events(&pool, run_id, cursor, 250)
            .await
            .unwrap()
            .is_empty());
        pool.close().await;
    }
}
