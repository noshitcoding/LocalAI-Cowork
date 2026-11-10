//! Content-free runtime observations. Process liveness, enabled features and
//! qualified deployment evidence are deliberately different observations.
use std::{
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use tokio::sync::watch;
use uuid::Uuid;

use crate::{
    config::{Config, ProcessMode},
    error::ApiError,
    AppState,
};

static SCHEDULER_TICK: AtomicU64 = AtomicU64::new(0);
static MAINTENANCE_TICK: AtomicU64 = AtomicU64::new(0);
static DISPATCH_TICK: AtomicU64 = AtomicU64::new(0);
static ACTIVE_RUNS: AtomicU64 = AtomicU64::new(0);
static LOOP_FRESHNESS_MS: AtomicU64 = AtomicU64::new(20_000);
static DRAINING: AtomicBool = AtomicBool::new(false);

fn stamp(target: &AtomicU64) {
    target.store(
        Utc::now().timestamp_millis().max(0) as u64,
        Ordering::Relaxed,
    );
}
pub fn mark_scheduler_tick() {
    stamp(&SCHEDULER_TICK);
}
pub fn mark_maintenance_tick() {
    stamp(&MAINTENANCE_TICK);
}
pub fn mark_dispatch_tick() {
    stamp(&DISPATCH_TICK);
}
pub fn set_active_runs(count: usize) {
    ACTIVE_RUNS.store(count as u64, Ordering::Relaxed);
}
pub fn set_draining() {
    DRAINING.store(true, Ordering::Relaxed);
}

fn observations() -> Value {
    json!({
        "scheduler_tick_ms": SCHEDULER_TICK.load(Ordering::Relaxed),
        "maintenance_tick_ms": MAINTENANCE_TICK.load(Ordering::Relaxed),
        "dispatch_tick_ms": DISPATCH_TICK.load(Ordering::Relaxed),
        "active_runs": ACTIVE_RUNS.load(Ordering::Relaxed),
        "loop_freshness_ms": LOOP_FRESHNESS_MS.load(Ordering::Relaxed),
        "organisation_mfa_version":crate::organisation_mfa::instance_contract().unwrap_or(0),
        "disk_quota_version":crate::instance_disk_quotas::contract().unwrap_or(0)
    })
}

pub async fn wait_for_shutdown(mut receiver: watch::Receiver<bool>) {
    loop {
        if *receiver.borrow_and_update() {
            return;
        }
        if receiver.changed().await.is_err() {
            return;
        }
    }
}

/// The selected API admission owner. Cluster operation requires explicit
/// configuration; losing either kind permanently fences this process.
#[derive(Clone)]
pub enum ApiOwnership {
    Single(crate::api_replica::ApiReplicaGuard),
    Cluster(crate::api_cluster::ApiCluster),
}

impl ApiOwnership {
    pub async fn ensure_active(&self) -> anyhow::Result<()> {
        match self {
            Self::Single(guard) => guard.ensure_active().await,
            Self::Cluster(cluster) => cluster.ensure_active().await,
        }
    }

    pub async fn wait_until_lost(&self) {
        match self {
            Self::Single(guard) => guard.wait_until_lost().await,
            Self::Cluster(cluster) => cluster.wait_until_lost().await,
        }
    }

    pub fn begin_draining(&self) {
        match self {
            Self::Single(guard) => guard.begin_draining(),
            Self::Cluster(cluster) => cluster.begin_draining(),
        }
    }

    pub fn status(&self) -> Value {
        match self {
            Self::Single(guard) => json!(guard.status()),
            Self::Cluster(cluster) => json!(cluster.status()),
        }
    }
}

/// Cancel the socket relay on ownership loss while allowing its caller to run
/// session/audit cleanup after the relay's socket halves have been dropped.
pub async fn guard_api_future<T>(
    guard: Option<ApiOwnership>,
    operation: impl std::future::Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    let Some(guard) = guard else {
        anyhow::bail!("interactive connection requires an API owner");
    };
    guard.ensure_active().await?;
    tokio::select! {
        biased;
        _ = guard.wait_until_lost() => anyhow::bail!("API ownership lost; interactive connection closed"),
        result = operation => result,
    }
}

/// Owned by main, aborted/awaited at exit. Publication never substitutes for an
/// actual scheduler or dispatch tick, and never releases execution capacity.
pub async fn publish(pool: PgPool, config: Config, mut shutdown: watch::Receiver<bool>) {
    LOOP_FRESHNESS_MS.store(
        (config.worker_poll_interval.as_millis() as u64)
            .saturating_mul(3)
            .max(20_000),
        Ordering::Relaxed,
    );
    let process_id = Uuid::new_v4();
    let started_at = Utc::now();
    let mode = match config.mode {
        ProcessMode::Api => "api",
        ProcessMode::Worker => "worker",
        ProcessMode::All => "all",
        ProcessMode::Management => "management",
    };
    let migration = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(max(version),0) FROM _sqlx_migrations WHERE success",
    )
    .fetch_one(&pool)
    .await
    .unwrap_or(0);
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if *shutdown.borrow_and_update() {
            let _ = write_pulse(&pool, process_id, mode, migration, "stopped", started_at).await;
            return;
        }
        tokio::select! {
            _ = interval.tick() => {
                let lifecycle = if DRAINING.load(Ordering::Relaxed) { "draining" } else { "running" };
                if let Err(error) = write_pulse(&pool, process_id, mode, migration, lifecycle, started_at).await {
                    tracing::warn!(?error, "runtime health publication failed");
                }
                // Retain a bounded day of crash/stopped diagnostics, not a run log.
                let _ = sqlx::query("DELETE FROM server_process_health WHERE last_seen_at < now() - interval '1 day'")
                    .execute(&pool).await;
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow_and_update() {
                    let _ = write_pulse(&pool, process_id, mode, migration, "stopped", started_at).await;
                    return;
                }
            }
        }
    }
}

async fn write_pulse(
    pool: &PgPool,
    id: Uuid,
    mode: &str,
    migration: i64,
    lifecycle: &str,
    started: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO server_process_health(process_id,process_mode,build_version,migration_version,lifecycle,started_at,last_seen_at,observations) VALUES($1,$2,$3,$4,$5,$6,now(),$7) ON CONFLICT(process_id) DO UPDATE SET lifecycle=EXCLUDED.lifecycle,last_seen_at=now(),observations=EXCLUDED.observations")
        .bind(id).bind(mode).bind(env!("CARGO_PKG_VERSION")).bind(migration)
        .bind(lifecycle).bind(started).bind(observations()).execute(pool).await?;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeStatus {
    pub process_groups: Vec<ProcessGroup>,
    pub queue_oldest_age_seconds: i64,
    pub expired_active_leases: i64,
    pub features: FeatureStatus,
    pub api_routing: Value,
    pub event_delivery: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessGroup {
    pub mode: String,
    pub build_version: String,
    pub migration_version: i64,
    pub lifecycle: String,
    pub processes: i64,
    pub fresh_processes: i64,
    pub active_runs: i64,
    pub last_pulse_age_seconds: i64,
    pub scheduler_age_seconds: Option<i64>,
    pub maintenance_age_seconds: Option<i64>,
    pub dispatch_age_seconds: Option<i64>,
    pub scheduler_unhealthy_processes: i64,
    pub maintenance_unhealthy_processes: i64,
    pub dispatch_unhealthy_processes: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FeatureStatus {
    pub process_role: &'static str,
    pub api_replica_policy: &'static str,
    pub event_delivery: &'static str,
    pub chat_workspace_allocation: bool,
    pub central_admission: bool,
    pub deployment_qualification: &'static str,
}

pub async fn collect(state: &AppState) -> Result<RuntimeStatus, ApiError> {
    let rows = sqlx::query(r#"
        SELECT process_mode,build_version,migration_version,lifecycle,count(*) AS processes,
          count(*) FILTER(WHERE lifecycle!='stopped' AND last_seen_at > now()-interval '20 seconds') AS fresh_processes,
          COALESCE(sum((observations->>'active_runs')::bigint) FILTER(WHERE lifecycle!='stopped' AND last_seen_at > now()-interval '20 seconds'),0)::bigint AS active_runs,
          GREATEST(0,EXTRACT(epoch FROM now()-max(last_seen_at)))::bigint AS pulse_age,
          max((observations->>'scheduler_tick_ms')::bigint) AS scheduler_tick,
          max((observations->>'maintenance_tick_ms')::bigint) AS maintenance_tick,
          max((observations->>'dispatch_tick_ms')::bigint) AS dispatch_tick,
          COALESCE(jsonb_agg(observations) FILTER(WHERE process_mode IN ('worker','all') AND lifecycle!='stopped' AND last_seen_at>now()-interval '20 seconds'),'[]'::jsonb) AS loop_observations
        FROM server_process_health GROUP BY process_mode,build_version,migration_version,lifecycle
        ORDER BY process_mode,build_version,migration_version,lifecycle
    "#).fetch_all(&state.pool).await?;
    let now = Utc::now().timestamp_millis();
    let mut groups = Vec::new();
    for row in rows {
        let samples: Value = row.try_get("loop_observations")?;
        let unhealthy = unhealthy_loops(samples.as_array().map(Vec::as_slice).unwrap_or(&[]), now);
        groups.push(ProcessGroup {
            mode: row.try_get("process_mode")?,
            build_version: row.try_get("build_version")?,
            migration_version: row.try_get("migration_version")?,
            lifecycle: row.try_get("lifecycle")?,
            processes: row.try_get("processes")?,
            fresh_processes: row.try_get("fresh_processes")?,
            active_runs: row.try_get("active_runs")?,
            last_pulse_age_seconds: row.try_get("pulse_age")?,
            scheduler_age_seconds: tick_age(row.try_get("scheduler_tick")?, now),
            maintenance_age_seconds: tick_age(row.try_get("maintenance_tick")?, now),
            dispatch_age_seconds: tick_age(row.try_get("dispatch_tick")?, now),
            scheduler_unhealthy_processes: unhealthy[0],
            maintenance_unhealthy_processes: unhealthy[1],
            dispatch_unhealthy_processes: unhealthy[2],
        });
    }
    Ok(RuntimeStatus {
        process_groups: groups,
        queue_oldest_age_seconds: sqlx::query_scalar("SELECT COALESCE(GREATEST(0,EXTRACT(epoch FROM now()-min(created_at))),0)::bigint FROM runs WHERE state='queued'").fetch_one(&state.pool).await?,
        expired_active_leases: sqlx::query_scalar("SELECT count(*) FROM runs WHERE state IN ('running','waiting_approval','waiting_input') AND lease_expires_at<=now()").fetch_one(&state.pool).await?,
        features: FeatureStatus {
            process_role: if state.management_mode { "management" } else { "instance" },
            api_replica_policy: if state.management_mode { "stateless_management" } else if state.api_cluster.is_some() { "durable_cluster_owner" } else { "single_connection_owner" },
            event_delivery: "postgres_notification_with_durable_replay",
            chat_workspace_allocation: std::env::var("COWORK_CHAT_WORKSPACES_ENABLED").is_ok_and(|value|value=="true"),
            central_admission: if state.management_mode {
                std::env::var("COWORK_MANAGEMENT_EXECUTION_ADMISSION_ENABLED").is_ok_and(|value| value == "true")
            } else { crate::fleet_execution::enabled() },
            deployment_qualification: "requires_image_bound_operator_evidence",
        },
        api_routing: state.api_ownership().map_or_else(
            || json!({"mode":"management","executor_connections":false}),
            |guard| guard.status()),
        event_delivery: json!(state.event_notifications.status()),
    })
}

/// One healthy worker must not hide a stalled or uninitialized peer.
fn unhealthy_loops(samples: &[Value], now: i64) -> [i64; 3] {
    let mut counts = [0; 3];
    for sample in samples {
        let bound = sample["loop_freshness_ms"]
            .as_i64()
            .unwrap_or(20_000)
            .max(20_000);
        for (index, key) in [
            "scheduler_tick_ms",
            "maintenance_tick_ms",
            "dispatch_tick_ms",
        ]
        .iter()
        .enumerate()
        {
            let tick = sample[*key].as_i64().unwrap_or(0);
            if tick <= 0 || now.saturating_sub(tick) > bound {
                counts[index] += 1;
            }
        }
    }
    counts
}

fn tick_age(tick: Option<i64>, now: i64) -> Option<i64> {
    tick.filter(|value| *value > 0)
        .map(|value| now.saturating_sub(value).max(0) / 1000)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absent_work_is_not_reported_as_a_recent_successful_tick() {
        assert_eq!(tick_age(None, 20_000), None);
        assert_eq!(tick_age(Some(0), 20_000), None);
        assert_eq!(tick_age(Some(5_000), 20_000), Some(15));
        assert_eq!(tick_age(Some(30_000), 20_000), Some(0));
    }
    #[test]
    fn healthy_worker_cannot_mask_stalled_or_missing_peer_ticks() {
        let healthy = json!({"scheduler_tick_ms":90_000,"maintenance_tick_ms":90_000,"dispatch_tick_ms":90_000});
        let stalled =
            json!({"scheduler_tick_ms":60_000,"maintenance_tick_ms":90_000,"dispatch_tick_ms":0});
        assert_eq!(
            unhealthy_loops(&[healthy.clone(), stalled], 100_000),
            [1, 0, 1]
        );
        assert_eq!(unhealthy_loops(&[healthy, json!({})], 100_000), [1, 1, 1]);
        assert_eq!(unhealthy_loops(&[], 100_000), [0, 0, 0]);
    }
    #[tokio::test]
    async fn shutdown_observes_preexisting_signal_and_sender_loss() {
        let (sender, receiver) = watch::channel(true);
        wait_for_shutdown(receiver).await;
        drop(sender);
        let (sender, receiver) = watch::channel(false);
        drop(sender);
        wait_for_shutdown(receiver).await;
    }
}
