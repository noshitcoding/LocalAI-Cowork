//! Original-instance, first-observation epochs for result history reads.
use super::result_clock::{ResultReadClock, ResultReadDeadline};
use super::*;
use cowork_contracts::ExecutionTargetRef;
use std::time::Instant;

pub(super) struct ContextEpoch {
    fingerprint: String,
    signed: Signed<ManagedResultReadContext>,
    deadline: ResultReadDeadline,
}

impl Broker {
    /// Observe an existing completed receipt and current fence. The native
    /// caller must independently retain the original published result and its
    /// wall/monotonic/boot retention through this call. Never creates a fence.
    pub async fn observe_managed_result_context(
        &self,
        signed: Signed<ManagedResultContextRequest>,
        target: &ExecutionTargetRef,
        retained_until: i64,
    ) -> Result<Signed<ManagedResultReadContext>> {
        signed.verify(
            "managed_result_context_request",
            &self.binding.control_public_key,
        )?;
        let request = signed.payload;
        let mut tx = self.pool.begin().await?;
        let high_water = self.lock(&mut tx).await?;
        let anchor = ResultReadClock::sample()?;
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
        let row = sqlx::query("SELECT * FROM runner_fences WHERE run_id=$1")
            .bind(&request.run_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(Error::Authority)?;
        let fence = self.checked_fence(row)?;
        if fence.lease_epoch < receipt.payload.request.lease_epoch {
            return Err(Error::Authority);
        }
        let (observed, _) = self.retain_managed_result_context(
            request,
            canonical_hash(&fence),
            now,
            anchor,
            high_water,
            retained_until,
        )?;
        tx.commit().await?;
        self.managed_result_context_deadline(
            &observed,
            DecisionClock::Live.sample(),
            ResultReadClock::sample()?,
        )?;
        Ok(observed)
    }

    pub(crate) fn retain_managed_result_context(
        &self,
        request: ManagedResultContextRequest,
        fence_sha256: String,
        now: i64,
        anchor: ResultReadClock,
        durable_high_water: i64,
        retained_until: i64,
    ) -> Result<(Signed<ManagedResultReadContext>, Instant)> {
        let fingerprint = canonical_hash(&request);
        let mut epochs = self
            .result_context_epochs
            .lock()
            .map_err(|_| Error::Uncertain)?;
        // Never prune by this transaction's tentative future wall observation.
        epochs.retain(|_, epoch| epoch.signed.payload.expires_at > durable_high_water);
        if let Some(epoch) = epochs.get(&request.request_id) {
            if epoch.fingerprint != fingerprint || epoch.signed.payload.fence_sha256 != fence_sha256
            {
                return Err(Error::Conflict);
            }
            epoch
                .signed
                .payload
                .validate(&request, &self.binding, now)?;
            if now >= retained_until {
                return Err(Error::Expired);
            }
            return Ok((epoch.signed.clone(), epoch.deadline.check(anchor)?));
        }
        if epochs.len() >= 4096 {
            return Err(Error::Uncertain);
        }
        let expires_at = request.expires_at.min(retained_until);
        let remaining = expires_at
            .checked_sub(now)
            .filter(|v| *v > 0)
            .ok_or(Error::Expired)?;
        let context = ManagedResultReadContext {
            format_version: 1,
            request,
            fence_sha256,
            observed_at: now,
            expires_at,
        };
        context.validate(&context.request, &self.binding, now)?;
        let signed = self.signer.sign("managed_result_context", context)?;
        let deadline = ResultReadDeadline::new(anchor, remaining)?;
        let instant_deadline = deadline.check(anchor)?;
        epochs.insert(
            signed.payload.request.request_id.clone(),
            ContextEpoch {
                fingerprint,
                signed: signed.clone(),
                deadline,
            },
        );
        Ok((signed, instant_deadline))
    }

    pub(crate) fn managed_result_context_deadline(
        &self,
        context: &Signed<ManagedResultReadContext>,
        now: i64,
        anchor: ResultReadClock,
    ) -> Result<Instant> {
        let epochs = self
            .result_context_epochs
            .lock()
            .map_err(|_| Error::Uncertain)?;
        let epoch = epochs
            .get(&context.payload.request.request_id)
            .ok_or(Error::Authority)?;
        if canonical_hash(&epoch.signed) != canonical_hash(context) {
            return Err(Error::Authority);
        }
        context
            .payload
            .validate(&context.payload.request, &self.binding, now)?;
        epoch.deadline.check(anchor)
    }
}
