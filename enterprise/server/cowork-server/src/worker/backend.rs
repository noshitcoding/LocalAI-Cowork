use super::{execute_run, lifecycle, WorkerRuntime};
use crate::{config::Config, db, desktop, storage, workflow};
use anyhow::Result;
use async_trait::async_trait;
use cowork_contracts::RunLease;
use sqlx::PgPool;
use tokio::time::Instant;

#[derive(Clone)]
pub(super) struct ClaimedRun {
    lease: RunLease,
    claim_started: Instant,
}

pub(super) struct ServerBackend {
    pub pool: PgPool,
    pub config: Config,
    pub runtime: WorkerRuntime,
}

#[async_trait]
impl lifecycle::Backend for ServerBackend {
    type Lease = ClaimedRun;

    fn key(&self, claimed: &ClaimedRun) -> lifecycle::LeaseKey {
        let lease = &claimed.lease;
        lifecycle::LeaseKey {
            run_id: lease.run.spec.id,
            lease_token: lease.lease_token,
        }
    }

    async fn claim(&self) -> Result<Option<ClaimedRun>> {
        let claim_started = Instant::now();
        Ok(db::claim_server_run(
            &self.pool,
            self.config.worker_id,
            self.config.lease_duration.as_secs() as i64,
        )
        .await?
        .map(|lease| ClaimedRun {
            lease,
            claim_started,
        }))
    }

    async fn execute(&self, claimed: ClaimedRun) -> Result<()> {
        crate::instance_disk_quotas::guard(execute_run(
            &self.pool,
            self.config.worker_id,
            self.config.lease_duration,
            &self.runtime,
            claimed.lease,
            claimed.claim_started,
        ))
        .await
    }

    async fn interrupt(&self, claimed: ClaimedRun) -> Result<()> {
        let lease = claimed.lease;
        if let Some(runner) = &self.runtime.desktop_runner {
            // Fences only this original token, including delayed job POSTs.
            // A successor lease must never be stopped by stale worker cleanup.
            let request = match tokio::time::timeout(
                std::time::Duration::from_secs(1),
                db::runner_lease_stop_request(&self.pool, lease.run.spec.id, lease.lease_token),
            )
            .await
            {
                Ok(Ok(request)) => request,
                error => {
                    tracing::warn!(?error,run_id=%lease.run.spec.id,"attempt inventory unavailable; fencing original runner lease without release authority");
                    crate::runner_admission::StopRequest {
                        version: 1,
                        run_id: lease.run.spec.id,
                        lease_token: Some(lease.lease_token),
                        cancel_lease: None,
                        attempts: vec![],
                    }
                }
            };
            match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                runner.stop_execution(&request),
            )
            .await
            {
                Ok(Ok(_)) => {}
                error => {
                    tracing::warn!(?error,run_id=%lease.run.spec.id,"physical stop of interrupted runner execution remains unconfirmed")
                }
            }
            if let Err(error) = tokio::time::timeout(
                std::time::Duration::from_secs(8),
                desktop::end_worker_leased_sessions(
                    &self.pool,
                    runner,
                    lease.run.spec.id,
                    self.config.worker_id,
                    lease.lease_token,
                ),
            )
            .await
            .map_err(anyhow::Error::from)
            .and_then(std::convert::identity)
            {
                tracing::warn!(?error, run_id = %lease.run.spec.id, "failed to clean up interrupted desktop sessions");
            }
        }
        db::interrupt_server_owned_lease(
            &self.pool,
            self.config.worker_id,
            lease.run.spec.id,
            lease.lease_token,
        )
        .await?;
        Ok(())
    }

    async fn schedule(&self) -> Result<()> {
        let count = workflow::trigger_due_schedules(
            &self.pool,
            &self.config.server_capabilities,
            chrono::Utc::now(),
            25,
        )
        .await?;
        if count > 0 {
            tracing::info!(count, "triggered due schedules");
        }
        Ok(())
    }

    async fn maintain(&self, poll_count: u64) -> Result<()> {
        if poll_count % 10 == 0 {
            match db::interrupt_expired_leases(&self.pool).await {
                Ok(count) if count > 0 => tracing::warn!(count, "interrupted expired run leases"),
                Err(error) => tracing::error!(?error, "failed to reap expired run leases"),
                _ => {}
            }
            match desktop::reap_terminal_sessions(&self.pool, self.runtime.desktop_runner.as_ref())
                .await
            {
                Ok(count) if count > 0 => {
                    tracing::info!(count, "cleaned up terminal run desktop sessions")
                }
                Err(error) => tracing::error!(?error, "failed to reap terminal desktop sessions"),
                _ => {}
            }
            match workflow::expire_pending_workflows(&self.pool, chrono::Utc::now()).await {
                Ok(count) if count > 0 => {
                    tracing::info!(count, "expired pending approvals or inputs")
                }
                Err(error) => tracing::error!(?error, "failed to expire approvals or inputs"),
                _ => {}
            }
        }
        if poll_count % self.config.maintenance_every_polls == 0 {
            match db::enforce_auth_session_retention(&self.pool, chrono::Utc::now(), 10_000).await {
                Ok(count) if count > 0 => {
                    tracing::info!(count, "removed authentication sessions past retention")
                }
                Err(error) => tracing::error!(?error, "authentication retention failed"),
                _ => {}
            }
            match db::enforce_run_event_retention(&self.pool, chrono::Utc::now(), 10_000).await {
                Ok(count) if count > 0 => {
                    tracing::info!(count, "removed run events past the 90-day retention window")
                }
                Err(error) => tracing::error!(?error, "run-event retention failed"),
                _ => {}
            }
            if let Some(store) = &self.runtime.object_store {
                match storage::garbage_collect(&self.pool, store, 100).await {
                    Ok(count) if count > 0 => {
                        tracing::info!(count, "garbage-collected snapshot chunks")
                    }
                    Err(error) => tracing::error!(?error, "snapshot garbage collection failed"),
                    _ => {}
                }
            }
        }
        Ok(())
    }
}
