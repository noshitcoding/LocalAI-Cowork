use std::{collections::HashMap, future::Future, sync::Arc, time::Duration};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tokio::{
    sync::{mpsc, watch, OwnedSemaphorePermit, Semaphore},
    task::{Id, JoinSet},
    time::{Instant, MissedTickBehavior},
};
use uuid::Uuid;

/// Lease identity includes the token: a later owner of the same run is a
/// different execution and must never be interrupted by this supervisor.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct LeaseKey {
    pub run_id: Uuid,
    pub lease_token: Uuid,
}

#[derive(Clone, Copy)]
pub(super) struct Settings {
    pub poll_interval: Duration,
    pub max_concurrency: usize,
    pub shutdown_grace: Duration,
}

#[async_trait]
pub(super) trait Backend: Send + Sync + 'static {
    type Lease: Clone + Send + Sync + 'static;

    fn key(&self, lease: &Self::Lease) -> LeaseKey;
    async fn claim(&self) -> Result<Option<Self::Lease>>;
    async fn execute(&self, lease: Self::Lease) -> Result<()>;
    async fn interrupt(&self, lease: Self::Lease) -> Result<()>;
    async fn schedule(&self) -> Result<()>;
    async fn maintain(&self, poll_count: u64) -> Result<()>;
}

struct Claimed<L> {
    lease: L,
    permit: OwnedSemaphorePermit,
}

/// Every spawned task belongs to a JoinSet, which aborts its tasks on drop.
/// A signal stops admission and periodic work, then lets existing executions
/// finish. At the deadline executions are canceled and their exact leases are
/// interrupted, never returned to the queue.
pub(super) async fn supervise<B, S>(backend: B, settings: Settings, shutdown: S) -> Result<()>
where
    B: Backend,
    S: Future<Output = ()>,
{
    let backend = Arc::new(backend);
    let (stop_tx, stop_rx) = watch::channel(false);
    let (claimed_tx, mut claimed_rx) = mpsc::channel(settings.max_concurrency);
    let permits = Arc::new(Semaphore::new(settings.max_concurrency));
    let mut loops = JoinSet::new();
    loops.spawn(claim_loop(
        backend.clone(),
        settings.poll_interval,
        permits,
        claimed_tx,
        stop_rx.clone(),
    ));
    loops.spawn(periodic_loop(
        backend.clone(),
        settings.poll_interval,
        false,
        stop_rx.clone(),
    ));
    loops.spawn(periodic_loop(
        backend.clone(),
        settings.poll_interval,
        true,
        stop_rx,
    ));

    let mut executions = JoinSet::new();
    let mut cleanups = JoinSet::new();
    let mut active: HashMap<Id, (LeaseKey, B::Lease, OwnedSemaphorePermit)> = HashMap::new();
    let mut stopping = false;
    let mut deadline = None;
    let mut channel_closed = false;
    let mut supervisor_error = None;
    tokio::pin!(shutdown);

    loop {
        if stopping
            && loops.is_empty()
            && executions.is_empty()
            && cleanups.is_empty()
            && channel_closed
        {
            break;
        }
        tokio::select! {
            biased;
            _ = &mut shutdown, if !stopping => {
                stopping = true;
                deadline = Some(Instant::now() + settings.shutdown_grace);
                let _ = stop_tx.send(true);
                crate::runtime_status::set_draining();
                tracing::info!(active_runs = active.len(), "worker stopped admission; draining runs");
            }
            _ = async {
                if let Some(deadline) = deadline {
                    tokio::time::sleep_until(deadline).await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                tracing::warn!(active_runs = active.len(), "worker drain deadline reached");
                break;
            }
            claimed = claimed_rx.recv(), if !channel_closed => {
                match claimed {
                    Some(Claimed { lease, permit }) if !stopping => {
                        let key = backend.key(&lease);
                        let execution_backend = backend.clone();
                        let execution_lease = lease.clone();
                        let task = executions.spawn(async move {
                            execution_backend.execute(execution_lease).await
                        });
                        active.insert(task.id(), (key, lease, permit));
                        crate::runtime_status::set_active_runs(active.len());
                    }
                    Some(Claimed { lease, permit }) => spawn_cleanup(&mut cleanups, backend.clone(), lease, permit),
                    None => channel_closed = true,
                }
            }
            joined = executions.join_next_with_id(), if !executions.is_empty() => {
                match joined {
                    Some(Ok((id, outcome))) => {
                        if let Some((key, lease, permit)) = active.remove(&id) {
                            crate::runtime_status::set_active_runs(active.len());
                            if let Err(error) = outcome {
                                tracing::error!(?error, run_id = %key.run_id, "server run execution failed");
                                spawn_cleanup(&mut cleanups, backend.clone(), lease, permit);
                            }
                        }
                    }
                    Some(Err(error)) => {
                        if let Some((key, lease, permit)) = active.remove(&error.id()) {
                            crate::runtime_status::set_active_runs(active.len());
                            tracing::error!(?error, run_id = %key.run_id, "server run task failed");
                            spawn_cleanup(&mut cleanups, backend.clone(), lease, permit);
                        }
                    }
                    None => {}
                }
            }
            joined = cleanups.join_next(), if !cleanups.is_empty() => {
                if let Some(Err(error)) = joined {
                    tracing::error!(?error, "worker lease cleanup task failed");
                }
            }
            joined = loops.join_next(), if !loops.is_empty() => {
                if !stopping {
                    supervisor_error = Some(match joined {
                        Some(Err(error)) => anyhow!("worker background task failed: {error}"),
                        _ => anyhow!("worker background task stopped unexpectedly"),
                    });
                    stopping = true;
                    deadline = Some(Instant::now() + settings.shutdown_grace);
                    let _ = stop_tx.send(true);
                    crate::runtime_status::set_draining();
                }
            }
        }
    }

    // Abort and join before cleanup, so canceled executions cannot continue
    // heartbeating or dispatch further tools while their leases are interrupted.
    loops.abort_all();
    executions.abort_all();
    while loops.join_next().await.is_some() {}
    while executions.join_next().await.is_some() {}
    crate::runtime_status::set_active_runs(0);
    for (_, (_, lease, permit)) in active {
        spawn_cleanup(&mut cleanups, backend.clone(), lease, permit);
    }
    while let Ok(Claimed { lease, permit }) = claimed_rx.try_recv() {
        spawn_cleanup(&mut cleanups, backend.clone(), lease, permit);
    }
    // Cleanup runs concurrently under one deadline, rather than multiplying a
    // runner/database timeout by the number of owned runs.
    if tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(joined) = cleanups.join_next().await {
            if let Err(error) = joined {
                tracing::error!(?error, "worker lease cleanup task failed");
            }
        }
    })
    .await
    .is_err()
    {
        tracing::warn!(
            "worker lease cleanup deadline reached; expiry reaper will reconcile remaining leases"
        );
        cleanups.abort_all();
        while cleanups.join_next().await.is_some() {}
    }

    match supervisor_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn spawn_cleanup<B: Backend>(
    cleanups: &mut JoinSet<()>,
    backend: Arc<B>,
    lease: B::Lease,
    permit: OwnedSemaphorePermit,
) {
    cleanups.spawn(async move {
        // Admission capacity remains reserved until lease cleanup has finished,
        // including when an executor panics or fails before finalization.
        let _permit = permit;
        interrupt(backend.as_ref(), lease).await;
    });
}

async fn interrupt<B: Backend>(backend: &B, lease: B::Lease) {
    let key = backend.key(&lease);
    // Database availability must not make shutdown unbounded. If cleanup fails,
    // expiry maintenance will interrupt the lease, with the same no-replay rule.
    match tokio::time::timeout(Duration::from_secs(20), backend.interrupt(lease)).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(?error, run_id = %key.run_id, "could not interrupt owned worker lease")
        }
        Err(_) => tracing::warn!(run_id = %key.run_id, "owned worker lease cleanup timed out"),
    }
}

async fn stopped(stop: &mut watch::Receiver<bool>) {
    while !*stop.borrow_and_update() {
        if stop.changed().await.is_err() {
            break;
        }
    }
}

async fn claim_loop<B: Backend>(
    backend: Arc<B>,
    poll_interval: Duration,
    permits: Arc<Semaphore>,
    claimed: mpsc::Sender<Claimed<B::Lease>>,
    mut stop: watch::Receiver<bool>,
) {
    let mut interval = tokio::time::interval(poll_interval);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _ = stopped(&mut stop) => break,
            _ = interval.tick() => {}
        }
        crate::runtime_status::mark_dispatch_tick();
        // Reserve capacity before touching the database. Queued and executing
        // leases both hold a permit, so admission cannot exceed the limit.
        let Ok(permit) = permits.clone().try_acquire_owned() else {
            continue;
        };
        match backend.claim().await {
            Ok(Some(lease)) => {
                if let Err(error) = claimed.send(Claimed { lease, permit }).await {
                    interrupt(backend.as_ref(), error.0.lease).await;
                    break;
                }
            }
            Ok(None) => {}
            Err(error) => tracing::error!(?error, "failed to claim server run"),
        }
    }
}

async fn periodic_loop<B: Backend>(
    backend: Arc<B>,
    poll_interval: Duration,
    maintenance: bool,
    mut stop: watch::Receiver<bool>,
) {
    let mut interval = tokio::time::interval(poll_interval);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut poll_count = 0_u64;
    loop {
        tokio::select! {
            biased;
            _ = stopped(&mut stop) => break,
            _ = interval.tick() => {}
        }
        poll_count = poll_count.wrapping_add(1);
        let outcome = if maintenance {
            crate::runtime_status::mark_maintenance_tick();
            backend.maintain(poll_count).await
        } else {
            crate::runtime_status::mark_scheduler_tick();
            backend.schedule().await
        };
        if let Err(error) = outcome {
            tracing::error!(?error, maintenance, "worker periodic task failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::VecDeque,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Mutex,
        },
    };

    #[derive(Clone)]
    struct TestBackend(Arc<State>);

    struct State {
        queued: Mutex<VecDeque<LeaseKey>>,
        gate: Semaphore,
        claimed: AtomicUsize,
        started: AtomicUsize,
        active: AtomicUsize,
        maximum_active: AtomicUsize,
        completed: AtomicUsize,
        canceled: AtomicUsize,
        schedules: AtomicUsize,
        maintenance: AtomicUsize,
        interrupted: Mutex<Vec<LeaseKey>>,
        cleanup_gate: Semaphore,
        block_cleanup: AtomicBool,
        cleanup_started: AtomicUsize,
        fail_claim: AtomicBool,
        fail_execute: AtomicBool,
        panic_execute: AtomicBool,
        panic_scheduler: AtomicBool,
    }

    impl TestBackend {
        fn new(count: u128) -> Self {
            Self(Arc::new(State {
                queued: Mutex::new(
                    (1..=count)
                        .map(|token| LeaseKey {
                            // Deliberately share a run ID to exercise token identity.
                            run_id: Uuid::from_u128(1),
                            lease_token: Uuid::from_u128(token),
                        })
                        .collect(),
                ),
                gate: Semaphore::new(0),
                claimed: AtomicUsize::new(0),
                started: AtomicUsize::new(0),
                active: AtomicUsize::new(0),
                maximum_active: AtomicUsize::new(0),
                completed: AtomicUsize::new(0),
                canceled: AtomicUsize::new(0),
                schedules: AtomicUsize::new(0),
                maintenance: AtomicUsize::new(0),
                interrupted: Mutex::new(Vec::new()),
                cleanup_gate: Semaphore::new(0),
                block_cleanup: AtomicBool::new(false),
                cleanup_started: AtomicUsize::new(0),
                fail_claim: AtomicBool::new(false),
                fail_execute: AtomicBool::new(false),
                panic_execute: AtomicBool::new(false),
                panic_scheduler: AtomicBool::new(false),
            }))
        }
    }

    struct ExecutionGuard {
        state: Arc<State>,
        completed: bool,
    }

    impl Drop for ExecutionGuard {
        fn drop(&mut self) {
            self.state.active.fetch_sub(1, Ordering::SeqCst);
            if !self.completed {
                self.state.canceled.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    #[async_trait]
    impl Backend for TestBackend {
        type Lease = LeaseKey;

        fn key(&self, lease: &LeaseKey) -> LeaseKey {
            *lease
        }

        async fn claim(&self) -> Result<Option<LeaseKey>> {
            if self.0.fail_claim.swap(false, Ordering::SeqCst) {
                return Err(anyhow!("transient claim failure"));
            }
            let lease = self.0.queued.lock().unwrap().pop_front();
            if lease.is_some() {
                self.0.claimed.fetch_add(1, Ordering::SeqCst);
            }
            Ok(lease)
        }

        async fn execute(&self, _lease: LeaseKey) -> Result<()> {
            let active = self.0.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.0.maximum_active.fetch_max(active, Ordering::SeqCst);
            self.0.started.fetch_add(1, Ordering::SeqCst);
            let mut guard = ExecutionGuard {
                state: self.0.clone(),
                completed: false,
            };
            assert!(
                !self.0.panic_execute.load(Ordering::SeqCst),
                "executor panic"
            );
            if self.0.fail_execute.load(Ordering::SeqCst) {
                return Err(anyhow!("executor failure"));
            }
            self.0.gate.acquire().await.unwrap().forget();
            self.0.completed.fetch_add(1, Ordering::SeqCst);
            guard.completed = true;
            Ok(())
        }

        async fn interrupt(&self, lease: LeaseKey) -> Result<()> {
            // Cleanup must begin only after cancellation has dropped the future.
            assert_eq!(self.0.active.load(Ordering::SeqCst), 0);
            self.0.cleanup_started.fetch_add(1, Ordering::SeqCst);
            if self.0.block_cleanup.load(Ordering::SeqCst) {
                self.0.cleanup_gate.acquire().await.unwrap().forget();
            }
            self.0.interrupted.lock().unwrap().push(lease);
            Ok(())
        }

        async fn schedule(&self) -> Result<()> {
            let attempt = self.0.schedules.fetch_add(1, Ordering::SeqCst);
            assert!(
                !self.0.panic_scheduler.load(Ordering::SeqCst),
                "scheduler panic"
            );
            if attempt == 0 {
                return Err(anyhow!("transient schedule failure"));
            }
            Ok(())
        }

        async fn maintain(&self, _poll_count: u64) -> Result<()> {
            self.0.maintenance.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn settings(concurrency: usize, grace: Duration) -> Settings {
        Settings {
            poll_interval: Duration::from_millis(5),
            max_concurrency: concurrency,
            shutdown_grace: grace,
        }
    }

    async fn until(predicate: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !predicate() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("worker condition did not become true");
    }

    #[tokio::test]
    async fn blocked_execution_does_not_block_scheduler_or_maintenance_and_drains() {
        let backend = TestBackend::new(1);
        let state = backend.0.clone();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn(supervise(
            backend,
            settings(1, Duration::from_secs(1)),
            async {
                let _ = stop_rx.await;
            },
        ));
        until(|| {
            state.started.load(Ordering::SeqCst) == 1
                && state.schedules.load(Ordering::SeqCst) >= 3
                && state.maintenance.load(Ordering::SeqCst) >= 3
        })
        .await;
        assert_eq!(state.completed.load(Ordering::SeqCst), 0);
        stop_tx.send(()).unwrap();
        tokio::task::yield_now().await;
        assert!(!worker.is_finished());
        state.gate.add_permits(1);
        tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(state.completed.load(Ordering::SeqCst), 1);
        assert!(state.interrupted.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn admission_reserves_capacity_before_claiming() {
        let backend = TestBackend::new(5);
        let state = backend.0.clone();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn(supervise(
            backend,
            settings(2, Duration::from_secs(1)),
            async {
                let _ = stop_rx.await;
            },
        ));
        until(|| {
            state.started.load(Ordering::SeqCst) == 2 && state.schedules.load(Ordering::SeqCst) >= 5
        })
        .await;
        assert_eq!(state.claimed.load(Ordering::SeqCst), 2);
        assert_eq!(state.maximum_active.load(Ordering::SeqCst), 2);
        state.gate.add_permits(5);
        until(|| state.completed.load(Ordering::SeqCst) == 5).await;
        stop_tx.send(()).unwrap();
        worker.await.unwrap().unwrap();
        assert_eq!(state.maximum_active.load(Ordering::SeqCst), 2);
        assert_eq!(state.claimed.load(Ordering::SeqCst), 5);
    }

    #[tokio::test]
    async fn drain_deadline_cancels_tasks_before_interrupting_exact_lease_tokens() {
        let backend = TestBackend::new(3);
        let state = backend.0.clone();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn(supervise(
            backend,
            settings(2, Duration::from_millis(10)),
            async {
                let _ = stop_rx.await;
            },
        ));
        until(|| state.started.load(Ordering::SeqCst) == 2).await;
        stop_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let mut interrupted = state.interrupted.lock().unwrap().clone();
        interrupted.sort_by_key(|key| key.lease_token);
        assert_eq!(interrupted.len(), 2);
        assert_eq!(interrupted[0].lease_token, Uuid::from_u128(1));
        assert_eq!(interrupted[1].lease_token, Uuid::from_u128(2));
        assert_eq!(state.canceled.load(Ordering::SeqCst), 2);
        assert_eq!(state.claimed.load(Ordering::SeqCst), 2);
        assert_eq!(state.completed.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn claim_and_execution_errors_are_observed_without_replaying_the_run() {
        let backend = TestBackend::new(1);
        backend.0.fail_claim.store(true, Ordering::SeqCst);
        backend.0.fail_execute.store(true, Ordering::SeqCst);
        let state = backend.0.clone();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn(supervise(
            backend,
            settings(1, Duration::from_secs(1)),
            async {
                let _ = stop_rx.await;
            },
        ));
        until(|| {
            !state.interrupted.lock().unwrap().is_empty()
                && state.schedules.load(Ordering::SeqCst) >= 3
        })
        .await;
        stop_tx.send(()).unwrap();
        worker.await.unwrap().unwrap();
        assert_eq!(state.claimed.load(Ordering::SeqCst), 1);
        assert_eq!(state.started.load(Ordering::SeqCst), 1);
        assert_eq!(state.interrupted.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn panicked_execution_is_interrupted_and_background_failure_is_reported() {
        let backend = TestBackend::new(1);
        backend.0.panic_execute.store(true, Ordering::SeqCst);
        let state = backend.0.clone();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn(supervise(
            backend,
            settings(1, Duration::from_secs(1)),
            async {
                let _ = stop_rx.await;
            },
        ));
        until(|| state.interrupted.lock().unwrap().len() == 1).await;
        stop_tx.send(()).unwrap();
        worker.await.unwrap().unwrap();
        assert_eq!(state.canceled.load(Ordering::SeqCst), 1);

        let backend = TestBackend::new(0);
        backend.0.panic_scheduler.store(true, Ordering::SeqCst);
        let error = supervise(
            backend,
            settings(1, Duration::from_millis(10)),
            std::future::pending(),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("background task failed"));
    }

    #[tokio::test]
    async fn failed_execution_reserves_capacity_until_cleanup_finishes() {
        let backend = TestBackend::new(2);
        backend.0.fail_execute.store(true, Ordering::SeqCst);
        backend.0.block_cleanup.store(true, Ordering::SeqCst);
        let state = backend.0.clone();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn(supervise(
            backend,
            settings(1, Duration::from_secs(1)),
            async {
                let _ = stop_rx.await;
            },
        ));
        until(|| {
            state.cleanup_started.load(Ordering::SeqCst) == 1
                && state.schedules.load(Ordering::SeqCst) >= 4
        })
        .await;
        assert_eq!(state.claimed.load(Ordering::SeqCst), 1);
        assert!(state.maintenance.load(Ordering::SeqCst) >= 3);
        state.fail_execute.store(false, Ordering::SeqCst);
        state.cleanup_gate.add_permits(1);
        until(|| state.started.load(Ordering::SeqCst) == 2).await;
        state.gate.add_permits(1);
        until(|| state.completed.load(Ordering::SeqCst) == 1).await;
        stop_tx.send(()).unwrap();
        worker.await.unwrap().unwrap();
        assert_eq!(state.claimed.load(Ordering::SeqCst), 2);
        assert_eq!(state.interrupted.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn dropping_supervisor_cancels_all_owned_tasks() {
        let backend = TestBackend::new(2);
        let state = backend.0.clone();
        let worker = tokio::spawn(supervise(
            backend,
            settings(2, Duration::from_secs(1)),
            std::future::pending(),
        ));
        until(|| state.started.load(Ordering::SeqCst) == 2).await;
        worker.abort();
        assert!(worker.await.unwrap_err().is_cancelled());
        until(|| state.canceled.load(Ordering::SeqCst) == 2).await;
        let schedules = state.schedules.load(Ordering::SeqCst);
        let maintenance = state.maintenance.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(state.schedules.load(Ordering::SeqCst), schedules);
        assert_eq!(state.maintenance.load(Ordering::SeqCst), maintenance);
        assert_eq!(state.active.load(Ordering::SeqCst), 0);
    }
}
