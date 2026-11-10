//! Instance-side admission and durable completion relay. Disabled until the
//! operator qualifies the whole execution path; never silently fall back when
//! an explicitly configured management admission service is unavailable.
use crate::{error::ApiError, fleet};
use cowork_contracts::{enterprise_execution::*, ExecutorTarget, RunRecord};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::{sync::OnceLock, time::Duration};
use uuid::Uuid;

struct Admission {
    organisation: Uuid,
    instance: Uuid,
    epoch: Uuid,
    endpoint: String,
    client: reqwest::Client,
}
static ADMISSION: OnceLock<Option<Admission>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionOutcome {
    Admitted,
    Waiting(ExecutionCapacityReason),
}

impl AdmissionOutcome {
    pub fn is_admitted(self) -> bool {
        matches!(self, Self::Admitted)
    }
}

/// Offline compatibility declaration for an operator-approved immutable image.
/// This contains no environment values, credentials or runtime configuration.
pub fn compatibility_contract() -> Value {
    serde_json::json!({"version": 1, "reservation_before_claim": true,
        "durable_completion": true, "generation_fencing": true,
        "external_claims": true, "worker": true})
}

pub fn initialize() -> anyhow::Result<()> {
    let enabled = std::env::var("COWORK_FLEET_EXECUTION_ADMISSION")
        .unwrap_or_else(|_| "false".into())
        .parse::<bool>()?;
    let value = if enabled {
        anyhow::ensure!(
            fleet::enabled(),
            "execution admission requires instance authentication"
        );
        fleet::secret("COWORK_FLEET_KEY_FILE")
            .map_err(|_| anyhow::anyhow!("instance authentication is unavailable"))?;
        let url = reqwest::Url::parse(&std::env::var("COWORK_FLEET_MANAGEMENT_URL")?)?;
        anyhow::ensure!(
            matches!(url.scheme(), "https" | "http")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "invalid execution admission endpoint"
        );
        Some(Admission {
            organisation: std::env::var("COWORK_ORGANISATION_ID")?.parse()?,
            instance: std::env::var("COWORK_INSTANCE_ID")?.parse()?,
            epoch: std::env::var("COWORK_EXECUTION_EPOCH")?.parse()?,
            endpoint: url.as_str().trim_end_matches('/').into(),
            client: crate::management_transport::client(url.as_str(), Duration::from_secs(5))?,
        })
    } else {
        None
    };
    ADMISSION
        .set(value)
        .map_err(|_| anyhow::anyhow!("execution admission already initialized"))
}
fn configured() -> Option<&'static Admission> {
    ADMISSION.get().and_then(Option::as_ref)
}
pub fn enabled() -> bool {
    configured().is_some()
}
fn unavailable() -> ApiError {
    ApiError::Unavailable(
        "central execution admission unavailable; capacity remains reserved".into(),
    )
}

impl Admission {
    async fn call<T: Serialize, R: DeserializeOwned>(
        &self,
        path: &str,
        value: &T,
    ) -> Result<R, ApiError> {
        let response = self
            .client
            .post(format!("{}/management/runner/v1/{path}", self.endpoint))
            .header(
                "x-cowork-instance-key",
                fleet::secret("COWORK_FLEET_KEY_FILE")?,
            )
            .json(value)
            .send()
            .await
            .map_err(|_| unavailable())?;
        if !response.status().is_success() {
            return Err(unavailable());
        }
        response.json().await.map_err(|_| unavailable())
    }
}

pub async fn reserve(
    tx: &mut Transaction<'_, Postgres>,
    run: &RunRecord,
    lease: Uuid,
) -> Result<AdmissionOutcome, ApiError> {
    let Some(config) = configured() else {
        return Ok(AdmissionOutcome::Admitted);
    };
    let request = ExecutionAdmissionRequest {
        organisation_id: config.organisation,
        instance_id: config.instance,
        execution_epoch: config.epoch,
        run_id: run.spec.id,
        user_id: run.spec.creator_user_id,
        target: run.spec.executor_target.clone(),
        mfa_contract: crate::organisation_mfa::instance_contract(),
    };
    let decision: ExecutionAdmissionDecision = config.call("execution-reserve", &request).await?;
    if !decision.admitted {
        if decision.reservation_id.is_some() {
            return Err(unavailable());
        }
        return Ok(AdmissionOutcome::Waiting(
            decision.reason.ok_or_else(unavailable)?,
        ));
    }
    let reservation = decision
        .reservation_id
        .filter(|_| decision.reason.is_none())
        .ok_or_else(unavailable)?;
    let changed = sqlx::query("INSERT INTO instance_execution_reservations(execution_epoch,run_id,reservation_id,lease_token) VALUES($1,$2,$3,$4) ON CONFLICT(execution_epoch,run_id) DO UPDATE SET lease_token=EXCLUDED.lease_token WHERE instance_execution_reservations.reservation_id=EXCLUDED.reservation_id AND instance_execution_reservations.completion IS NULL")
        .bind(config.epoch).bind(run.spec.id).bind(reservation).bind(lease).execute(&mut **tx).await?.rows_affected();
    if changed != 1 {
        return Err(ApiError::Conflict(
            "local execution reservation changed".into(),
        ));
    }
    // A failed local commit leaves the remote reservation held. A subsequent
    // claim of this still-queued run obtains the same reservation, not a new slot.
    Ok(AdmissionOutcome::Admitted)
}

async fn record_completion(
    tx: &mut Transaction<'_, Postgres>,
    run: Uuid,
    lease: Uuid,
    outcome: ExecutionCompletionOutcome,
) -> Result<(), ApiError> {
    let Some(config) = configured() else {
        return Ok(());
    };
    let row = sqlx::query("SELECT reservation_id,completion FROM instance_execution_reservations WHERE execution_epoch=$1 AND run_id=$2 AND lease_token=$3 FOR UPDATE")
        .bind(config.epoch).bind(run).bind(lease).fetch_optional(&mut **tx).await?.ok_or_else(unavailable)?;
    if row.try_get::<Option<Value>, _>("completion")?.is_some() {
        return Ok(());
    }
    let receipt = ExecutionReleaseRequest {
        organisation_id: config.organisation,
        instance_id: config.instance,
        execution_epoch: config.epoch,
        run_id: run,
        reservation_id: row.try_get("reservation_id")?,
        completion_id: Uuid::new_v4(),
        outcome,
        execution_stopped: true,
    };
    sqlx::query("UPDATE instance_execution_reservations SET completion=$3 WHERE execution_epoch=$1 AND run_id=$2")
        .bind(config.epoch).bind(run).bind(serde_json::to_value(receipt)?).execute(&mut **tx).await?;
    Ok(())
}

/// Persist the exact completion in the same transaction as the terminal run
/// state. A database failure must not commit one without the other.
pub async fn completion(
    tx: &mut Transaction<'_, Postgres>,
    run: &RunRecord,
    lease: Uuid,
    outcome: cowork_contracts::RunState,
    server_execution_stopped: bool,
) -> Result<(), ApiError> {
    if matches!(run.spec.executor_target, ExecutorTarget::ServerLinux { .. })
        && !server_execution_stopped
    {
        return Ok(());
    }
    let outcome = match outcome {
        cowork_contracts::RunState::Completed => ExecutionCompletionOutcome::Completed,
        cowork_contracts::RunState::Failed => ExecutionCompletionOutcome::Failed,
        _ => return Ok(()), // Expiry, interruption and cancellation are not proof of a stopped executor.
    };
    record_completion(tx, run.spec.id, lease, outcome).await
}

/// Cancellation releases only after authenticated physical stop of this exact
/// original lease and every durable attempt, plus local desktop cleanup.
pub(crate) async fn completion_after_verified_stop(
    tx: &mut Transaction<'_, Postgres>,
    run: &RunRecord,
    lease: Uuid,
    attempts: &[crate::runner_admission::ExpectedAttempt],
    proof: &crate::runner_admission::VerifiedStop,
) -> Result<(), ApiError> {
    if !proof.matches(run.spec.id, None, Some(lease), attempts) {
        return Err(ApiError::Conflict(
            "cancellation physical stop binding changed".into(),
        ));
    }
    let active: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM desktop_sessions WHERE run_id=$1 AND state IN ('starting','agent_controlled','user_controlled','paused'))")
        .bind(run.spec.id).fetch_one(&mut **tx).await?;
    if active {
        return Err(ApiError::Conflict(
            "desktop execution stop remains unconfirmed".into(),
        ));
    }
    record_completion(tx, run.spec.id, lease, ExecutionCompletionOutcome::Canceled).await
}

pub(crate) async fn retained_cancel_lease(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<Option<Uuid>, ApiError> {
    let Some(config) = configured() else {
        return Ok(None);
    };
    Ok(sqlx::query_scalar("SELECT lease_token FROM instance_execution_reservations WHERE execution_epoch=$1 AND run_id=$2 AND completion IS NULL")
        .bind(config.epoch).bind(run_id).fetch_optional(&mut **tx).await?)
}

pub fn spawn_relay(pool: PgPool) {
    let Some(config) = configured() else { return };
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_secs(2));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            let rows: Result<Vec<Value>,_> = sqlx::query_scalar("SELECT completion FROM instance_execution_reservations WHERE completion IS NOT NULL AND acknowledged_at IS NULL ORDER BY run_id LIMIT 100").fetch_all(&pool).await;
            let Ok(rows) = rows else {
                tracing::warn!("execution completion outbox unavailable");
                continue;
            };
            for value in rows {
                let Ok(receipt) = serde_json::from_value::<ExecutionReleaseRequest>(value.clone())
                else {
                    tracing::warn!("invalid execution completion evidence");
                    continue;
                };
                if receipt.instance_id != config.instance
                    || receipt.organisation_id != config.organisation
                {
                    tracing::warn!("execution completion scope mismatch");
                    continue;
                }
                let result: Result<ExecutionReleaseAcknowledgement, _> =
                    config.call("execution-release", &receipt).await;
                match result {
                    Ok(ack) if ack.reservation_id == receipt.reservation_id && ack.released => {
                        if sqlx::query("UPDATE instance_execution_reservations SET acknowledged_at=now() WHERE reservation_id=$1 AND completion=$2 AND acknowledged_at IS NULL")
                            .bind(receipt.reservation_id).bind(value).execute(&pool).await.is_err() { tracing::warn!("execution acknowledgement persistence failed; receipt will be retried"); }
                    }
                    _ => tracing::warn!(reservation_id=%receipt.reservation_id, "execution completion not acknowledged; capacity remains reserved"),
                }
            }
        }
    });
}
