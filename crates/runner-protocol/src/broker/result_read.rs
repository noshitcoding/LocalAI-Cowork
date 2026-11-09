//! One bounded read of the original managed result. Current actor/content rights
//! are resolved by the controller; the native result adapter additionally binds
//! the actual immutable output/receipt. This boundary never admits execution.
use super::result_clock::{ResultReadClock, ResultReadDeadline};
use super::*;
use cowork_contracts::ExecutionTargetRef;
use std::time::Instant;

pub(super) struct ReadEpoch {
    fingerprint: String,
    observed_at: i64,
    expires_at: i64,
    deadline: ResultReadDeadline,
}
impl ReadEpoch {
    fn check(
        &self,
        request: &ManagedResultReadRequest,
        now: i64,
        clock: ResultReadClock,
    ) -> Result<Instant> {
        if self.fingerprint != canonical_hash(request) {
            return Err(Error::Conflict);
        }
        if now < self.observed_at {
            return Err(Error::ClockRollback);
        }
        if now >= self.expires_at {
            return Err(Error::Expired);
        }
        self.deadline.check(clock)
    }
}
fn read_clock(request: &ManagedResultReadRequest) -> Result<ResultReadClock> {
    if request.context.is_some() {
        ResultReadClock::sample()
    } else {
        // Preserve V1's original Instant semantics. Only V2 content requests
        // require the independent suspend-aware native deadline.
        Ok(ResultReadClock {
            instant: Instant::now(),
            boot_ns: None,
        })
    }
}
/// Non-cloneable and non-serializable. The result adapter consumes it after
/// reading, before returning bytes. A dropped permit cannot recreate a newer
/// deadline, and a reopened Broker cannot accept an old instance-bound grant.
pub struct ManagedResultReadPermit {
    instance: String,
    request: ManagedResultReadRequest,
    observed_at: i64,
    deadline: Instant,
}
impl ManagedResultReadPermit {
    pub fn request(&self) -> &ManagedResultReadRequest {
        &self.request
    }
}
impl Broker {
    pub async fn authorize_managed_result_read(
        &self,
        signed: Signed<ManagedResultReadRequest>,
        current_target: &ExecutionTargetRef,
    ) -> Result<ManagedResultReadPermit> {
        signed.verify("managed_result_read", &self.binding.control_public_key)?;
        let (observed_at, deadline) = self
            .check_managed_result_read(&signed.payload, current_target, None)
            .await?;
        Ok(ManagedResultReadPermit {
            instance: self.instance.clone(),
            request: signed.payload,
            observed_at,
            deadline,
        })
    }
    pub async fn consume_managed_result_read(
        &self,
        permit: ManagedResultReadPermit,
        current_target: &ExecutionTargetRef,
    ) -> Result<ManagedResultReadRequest> {
        if permit.instance != self.instance {
            return Err(Error::Authority);
        }
        self.check_managed_result_read(
            &permit.request,
            current_target,
            Some((permit.observed_at, permit.deadline)),
        )
        .await?;
        Ok(permit.request)
    }
    async fn check_managed_result_read(
        &self,
        request: &ManagedResultReadRequest,
        target: &ExecutionTargetRef,
        previous: Option<(i64, Instant)>,
    ) -> Result<(i64, Instant)> {
        let mut tx = self.pool.begin().await?;
        let high_water = self.lock(&mut tx).await?;
        let anchor = read_clock(request)?;
        let now = DecisionClock::Live.sample();
        self.clock(&mut tx, high_water, now).await?;
        request.validate(&self.binding, target, &self.instance_fingerprint(), now)?;
        let row = sqlx::query("SELECT * FROM runner_executions WHERE attempt_id=$1")
            .bind(&request.process_attempt_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(Error::Uncertain)?;
        let receipt = self.checked(row)?;
        if !matches!(receipt.payload.state, ExecutionState::Completed { .. })
            || receipt.payload.request.run_id != request.run_id
            || receipt.payload.request_envelope_sha256 != request.launch_envelope_sha256
            || canonical_hash(&receipt) != request.execution_receipt_sha256
        {
            return Err(Error::Authority);
        }
        // History reads only observe an existing checked fence. Legacy grants
        // keep their exact original epoch. V2 binds a separately fresh, original
        // instance observation, never a positive execution fence or lease.
        let row = sqlx::query("SELECT * FROM runner_fences WHERE run_id=$1")
            .bind(&receipt.payload.request.run_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(Error::Authority)?;
        let fence = self.checked_fence(row)?;
        let context_deadline = match &request.context {
            Some(context) => {
                if fence.lease_epoch < receipt.payload.request.lease_epoch
                    || canonical_hash(&fence) != context.payload.fence_sha256
                {
                    return Err(Error::Authority);
                }
                Some(self.managed_result_context_deadline(context, now, read_clock(request)?)?)
            }
            None => {
                if fence.revoked || fence.lease_epoch != receipt.payload.request.lease_epoch {
                    return Err(Error::Authority);
                }
                None
            }
        };
        if let Some((observed, deadline)) = previous {
            if now < observed {
                return Err(Error::ClockRollback);
            }
            if anchor.instant >= deadline {
                return Err(Error::Expired);
            }
        }
        let initial = self.retain_result_read_epoch(request, now, anchor, high_water)?;
        let initial = context_deadline.map_or(initial, |deadline| initial.min(deadline));
        let deadline = previous.map_or(initial, |(_, deadline)| deadline.min(initial));
        tx.commit().await?;
        if Instant::now() >= deadline {
            return Err(Error::Expired);
        }
        if let Some(context) = &request.context {
            let current = read_clock(request)?;
            let observed = DecisionClock::Live.sample();
            if observed < now {
                return Err(Error::ClockRollback);
            }
            request.validate(
                &self.binding,
                target,
                &self.instance_fingerprint(),
                observed,
            )?;
            self.managed_result_read_deadline(request, observed, current)?;
            self.managed_result_context_deadline(context, observed, current)?;
        }
        Ok((now, deadline))
    }
    pub(crate) fn retain_result_read_epoch(
        &self,
        request: &ManagedResultReadRequest,
        now: i64,
        anchor: ResultReadClock,
        durable_high_water: i64,
    ) -> Result<Instant> {
        let fingerprint = canonical_hash(request);
        let mut epochs = self
            .result_read_epochs
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // Use only the ALREADY COMMITTED floor. Pruning on this transaction's
        // tentative `now` would permit reminting if its eventual commit failed.
        epochs.retain(|_, epoch| epoch.expires_at > durable_high_water);
        if let Some(epoch) = epochs.get(&request.request_id) {
            return epoch.check(request, now, anchor);
        }
        if epochs.len() >= 4096 {
            return Err(Error::Uncertain);
        }
        let remaining = request
            .expires_at
            .checked_sub(now)
            .filter(|n| *n > 0)
            .ok_or(Error::Expired)?;
        let deadline = ResultReadDeadline::new(anchor, remaining)?;
        let instant_deadline = deadline.check(anchor)?;
        epochs.insert(
            request.request_id.clone(),
            ReadEpoch {
                fingerprint,
                observed_at: now,
                expires_at: request.expires_at,
                deadline,
            },
        );
        Ok(instant_deadline)
    }
    pub(crate) fn managed_result_read_deadline(
        &self,
        request: &ManagedResultReadRequest,
        now: i64,
        clock: ResultReadClock,
    ) -> Result<Instant> {
        let epochs = self
            .result_read_epochs
            .lock()
            .map_err(|_| Error::Uncertain)?;
        epochs
            .get(&request.request_id)
            .ok_or(Error::Authority)?
            .check(request, now, clock)
    }
}
