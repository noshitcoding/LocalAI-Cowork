use super::*;
use sqlx::{Any, AnyPool, Row, Transaction};
mod delivery_origin;
pub(crate) mod result_clock;
mod result_context;
mod result_read;
pub use result_read::ManagedResultReadPermit;

// Real observations are sampled only after acquiring the serialized authority
// turn. Sampling before a pool/row-lock wait can falsely report clock rollback
// when a later-arriving operation commits a newer real timestamp first.
enum DecisionClock {
    Live,
    #[cfg(test)]
    At(i64),
}
impl DecisionClock {
    fn sample(self) -> i64 {
        match self {
            Self::Live => chrono::Utc::now().timestamp_millis(),
            #[cfg(test)]
            Self::At(value) => value,
        }
    }
}

#[derive(Clone)]
pub struct Broker {
    pool: AnyPool,
    binding: Binding,
    signer: SigningKey,
    instance: String,
    rollback_observation: Arc<std::sync::Mutex<Option<ClockRollbackObservation>>>,
    input_epochs: Arc<std::sync::Mutex<std::collections::BTreeMap<String, InputEpoch>>>,
    result_read_epochs:
        Arc<std::sync::Mutex<std::collections::BTreeMap<String, result_read::ReadEpoch>>>,
    result_context_epochs:
        Arc<std::sync::Mutex<std::collections::BTreeMap<String, result_context::ContextEpoch>>>,
}
struct InputEpoch {
    fingerprint: String,
    run_id: String,
    tool_call_id: String,
    observed_at: i64,
    anchor: std::time::Instant,
    deadline: std::time::Instant,
}
/// A pre-admission custody tombstone, never an execution receipt or a grant.
/// It only prevents a restart from minting a fresh monotonic input epoch for
/// already observed bytes. The original Instant is intentionally not persisted.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputCustodyRecord {
    request: LaunchRequest,
    broker_instance_sha256: String,
    observed_at: i64,
}
/// Private operator diagnosis only. Never consulted by authority decisions,
/// used to adjust time, persisted as a grant, or accepted from a client.
#[derive(Clone, Debug, Serialize)]
pub struct ClockRollbackObservation {
    pub high_water: i64,
    pub observed: i64,
}
/// Not serializable or cloneable. Restart/replay cannot recreate actual dispatch authority.
pub struct LaunchPermit {
    instance: String,
    receipt: Signed<Receipt>,
}
/// One in-memory handoff from the existing durable dispatch boundary to an
/// adapter. Receipts, cloned request bytes and restart recovery cannot construct
/// this value. Dropping it leaves DispatchCommitted uncertain, never retryable.
/// This is not a qualified runtime, verified input snapshot or physical result.
pub struct DispatchedLaunch {
    request: LaunchRequest,
    instance: String,
}
/// One fresh in-memory input operation, never dispatch authority. A serialized
/// receipt, copied request or reopened journal cannot reconstruct this permit.
pub struct InputTransferPermit {
    instance: String,
    request: LaunchRequest,
    observed_at: i64,
    deadline: std::time::Instant,
}
impl DispatchedLaunch {
    pub fn request(&self) -> &LaunchRequest {
        &self.request
    }
}
pub enum Admission {
    Launch(LaunchPermit),
    Existing(Signed<Receipt>),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct ExecutionLeaseReceipt {
    renewal: Signed<ExecutionLeaseRenewal>,
    previous_revision: u64,
    previous_expires_at: i64,
    accepted_at: i64,
}

impl Broker {
    /// The pool must already use durable full-sync storage and migration serialization.
    /// Pinning a control key is an operator bootstrap action, never a client request.
    pub async fn open(pool: AnyPool, binding: Binding, signer: SigningKey) -> Result<Self> {
        binding.validate()?;
        if signer.public_key() != binding.runner_public_key {
            return Err(Error::Authority);
        }
        let mut tx = pool.begin().await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS runner_binding (id INTEGER PRIMARY KEY, payload TEXT NOT NULL, high_water BIGINT NOT NULL)").execute(&mut *tx).await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS runner_executions (attempt_id TEXT PRIMARY KEY, run_id TEXT NOT NULL, tool_call_id TEXT NOT NULL, request_hash TEXT NOT NULL, revision BIGINT NOT NULL, payload TEXT NOT NULL, UNIQUE(run_id,tool_call_id))").execute(&mut *tx).await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS runner_fences (run_id TEXT PRIMARY KEY, epoch BIGINT NOT NULL, revoked INTEGER NOT NULL, payload TEXT NOT NULL)").execute(&mut *tx).await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS runner_execution_lease_renewals (attempt_id TEXT NOT NULL, revision BIGINT NOT NULL, expires_at BIGINT NOT NULL, payload TEXT NOT NULL, PRIMARY KEY(attempt_id,revision))").execute(&mut *tx).await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS runner_input_custody (attempt_id TEXT PRIMARY KEY, run_id TEXT NOT NULL, tool_call_id TEXT NOT NULL, payload TEXT NOT NULL, UNIQUE(run_id,tool_call_id))").execute(&mut *tx).await?;
        let payload = serde_json::to_string(&binding)?;
        sqlx::query("INSERT INTO runner_binding(id,payload,high_water) VALUES(1,$1,0) ON CONFLICT(id) DO NOTHING").bind(&payload).execute(&mut *tx).await?;
        let stored: String = sqlx::query("SELECT payload FROM runner_binding WHERE id=1")
            .fetch_one(&mut *tx)
            .await?
            .try_get("payload")?;
        if stored != payload {
            return Err(Error::Authority);
        }
        tx.commit().await?;
        Ok(Self {
            pool,
            binding,
            signer,
            instance: uuid::Uuid::new_v4().to_string(),
            rollback_observation: Arc::new(std::sync::Mutex::new(None)),
            input_epochs: Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new())),
            result_read_epochs: Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new())),
            result_context_epochs: Arc::new(std::sync::Mutex::new(
                std::collections::BTreeMap::new(),
            )),
        })
    }
    async fn lock(&self, tx: &mut Transaction<'_, Any>) -> Result<i64> {
        sqlx::query("UPDATE runner_binding SET high_water=high_water WHERE id=1")
            .execute(&mut **tx)
            .await?;
        let row = sqlx::query("SELECT payload,high_water FROM runner_binding WHERE id=1")
            .fetch_one(&mut **tx)
            .await?;
        if row.try_get::<String, _>("payload")? != serde_json::to_string(&self.binding)? {
            return Err(Error::Authority);
        }
        Ok(row.try_get("high_water")?)
    }
    async fn clock(&self, tx: &mut Transaction<'_, Any>, high_water: i64, now: i64) -> Result<()> {
        if now < high_water {
            *self
                .rollback_observation
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = Some(ClockRollbackObservation {
                high_water,
                observed: now,
            });
            return Err(Error::ClockRollback);
        }
        sqlx::query("UPDATE runner_binding SET high_water=$1 WHERE id=1")
            .bind(now)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }
    pub fn last_clock_rollback_observation(&self) -> Option<ClockRollbackObservation> {
        self.rollback_observation
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
    pub fn is_bound_to(&self, expected: &Binding) -> bool {
        &self.binding == expected
    }
    pub fn is_same_instance(&self, other: &Broker) -> bool {
        self.instance == other.instance
    }
    /// Local custody binding, never an authentication token or portable grant.
    pub fn instance_fingerprint(&self) -> String {
        canonical_hash(&("runner_input_custody_v1", &self.instance, &self.binding))
    }
    pub fn initial_input_deadline(
        &self,
        request: &LaunchRequest,
        absolute_expiry: i64,
    ) -> Result<std::time::Instant> {
        let epochs = self.input_epochs.lock().unwrap_or_else(|p| p.into_inner());
        let epoch = epochs
            .get(&request.process_attempt_id)
            .ok_or(Error::Uncertain)?;
        if epoch.fingerprint != canonical_hash(request) {
            return Err(Error::Conflict);
        }
        let remaining = absolute_expiry
            .checked_sub(epoch.observed_at)
            .filter(|v| *v > 0)
            .ok_or(Error::Expired)?;
        Ok(epoch.deadline.min(
            epoch.anchor
                + std::time::Duration::from_millis(
                    remaining.try_into().map_err(|_| Error::Expired)?,
                ),
        ))
    }
    pub(crate) fn retain_input_epoch(
        &self,
        request: &LaunchRequest,
        observed_at: i64,
        anchor: std::time::Instant,
    ) -> Result<std::time::Instant> {
        let fingerprint = canonical_hash(request);
        let mut epochs = self.input_epochs.lock().unwrap_or_else(|p| p.into_inner());
        if epochs.values().any(|e| {
            e.run_id == request.run_id
                && e.tool_call_id == request.tool_call_id
                && e.fingerprint != fingerprint
        }) {
            return Err(Error::Conflict);
        }
        if let Some(epoch) = epochs.get(&request.process_attempt_id) {
            if epoch.fingerprint != fingerprint {
                return Err(Error::Conflict);
            }
            if observed_at < epoch.observed_at {
                return Err(Error::ClockRollback);
            }
            if anchor >= epoch.deadline {
                return Err(Error::Expired);
            }
            return Ok(epoch.deadline);
        }
        if epochs.len() >= 4096 {
            return Err(Error::Uncertain);
        }
        let remaining = request
            .expires_at
            .checked_sub(observed_at)
            .filter(|v| *v > 0)
            .ok_or(Error::Expired)?;
        let deadline = anchor
            + std::time::Duration::from_millis(remaining.try_into().map_err(|_| Error::Expired)?);
        epochs.insert(
            request.process_attempt_id.clone(),
            InputEpoch {
                fingerprint,
                run_id: request.run_id.clone(),
                tool_call_id: request.tool_call_id.clone(),
                observed_at,
                anchor,
                deadline,
            },
        );
        Ok(deadline)
    }
    fn check_known_input_deadline(&self, request: &LaunchRequest) -> Result<()> {
        let epochs = self.input_epochs.lock().unwrap_or_else(|p| p.into_inner());
        let fingerprint = canonical_hash(request);
        if epochs.values().any(|e| {
            e.run_id == request.run_id
                && e.tool_call_id == request.tool_call_id
                && e.fingerprint != fingerprint
        }) {
            return Err(Error::Conflict);
        }
        if let Some(epoch) = epochs.get(&request.process_attempt_id) {
            if epoch.fingerprint != canonical_hash(request) {
                return Err(Error::Conflict);
            }
            if std::time::Instant::now() >= epoch.deadline {
                return Err(Error::Expired);
            }
        }
        Ok(())
    }
    async fn input_custody(
        &self,
        tx: &mut Transaction<'_, Any>,
        request: &LaunchRequest,
        create_at: Option<i64>,
    ) -> Result<()> {
        let row = sqlx::query("SELECT * FROM runner_input_custody WHERE attempt_id=$1 OR (run_id=$2 AND tool_call_id=$3)")
            .bind(&request.process_attempt_id).bind(&request.run_id).bind(&request.tool_call_id)
            .fetch_optional(&mut **tx).await?;
        if let Some(row) = row {
            let signed: Signed<InputCustodyRecord> =
                serde_json::from_str(&row.try_get::<String, _>("payload")?)?;
            signed.verify("input_custody", &self.binding.runner_public_key)?;
            let record = signed.payload;
            record.request.validate(&self.binding)?;
            if record.request.process_attempt_id != row.try_get::<String, _>("attempt_id")?
                || record.request.run_id != row.try_get::<String, _>("run_id")?
                || record.request.tool_call_id != row.try_get::<String, _>("tool_call_id")?
                || record.observed_at < record.request.issued_at
                || record.observed_at >= record.request.expires_at
            {
                return Err(Error::Authority);
            }
            if record.request != *request {
                return Err(Error::Conflict);
            }
            if record.broker_instance_sha256 != self.instance_fingerprint() {
                return Err(Error::Uncertain);
            }
            // Even a valid tombstone cannot recreate a lost in-memory epoch.
            if !self
                .input_epochs
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .contains_key(&request.process_attempt_id)
            {
                return Err(Error::Uncertain);
            }
        } else if let Some(observed_at) = create_at {
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runner_input_custody")
                .fetch_one(&mut **tx)
                .await?;
            if count >= 4096 {
                return Err(Error::Uncertain);
            }
            let signed = self.signer.sign(
                "input_custody",
                InputCustodyRecord {
                    request: request.clone(),
                    broker_instance_sha256: self.instance_fingerprint(),
                    observed_at,
                },
            )?;
            sqlx::query("INSERT INTO runner_input_custody(attempt_id,run_id,tool_call_id,payload) VALUES($1,$2,$3,$4)")
                .bind(&request.process_attempt_id).bind(&request.run_id).bind(&request.tool_call_id)
                .bind(serde_json::to_string(&signed)?).execute(&mut **tx).await?;
        }
        Ok(())
    }
    /// Request equality cannot prove that this ledger consumed its permit.
    pub fn validate_owned_dispatch(&self, dispatch: &DispatchedLaunch) -> Result<()> {
        if dispatch.instance != self.instance {
            return Err(Error::Authority);
        }
        dispatch.request.validate(&self.binding)
    }
    fn checked(&self, row: sqlx::any::AnyRow) -> Result<Signed<Receipt>> {
        let receipt: Signed<Receipt> = serde_json::from_str(&row.try_get::<String, _>("payload")?)?;
        receipt.verify("receipt", &self.binding.runner_public_key)?;
        receipt.payload.validate(&self.binding)?;
        let r = &receipt.payload;
        if r.request.process_attempt_id != row.try_get::<String, _>("attempt_id")?
            || r.request.run_id != row.try_get::<String, _>("run_id")?
            || r.request.tool_call_id != row.try_get::<String, _>("tool_call_id")?
            || r.request_envelope_sha256 != row.try_get::<String, _>("request_hash")?
            || r.revision != row.try_get::<i64, _>("revision")?
        {
            return Err(Error::Authority);
        }
        Ok(receipt)
    }
    pub async fn receipt(&self, attempt: &str) -> Result<Signed<Receipt>> {
        let row = sqlx::query("SELECT * FROM runner_executions WHERE attempt_id=$1")
            .bind(attempt)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)?;
        self.checked(row)
    }
    /// Input transfer precedes execution admission. Require the current signed
    /// intent/fence on every operation, with no new run/admission ledger.
    pub async fn authorize_input_transfer(
        &self,
        signed: &Signed<LaunchRequest>,
    ) -> Result<InputTransferPermit> {
        signed.verify("launch", &self.binding.control_public_key)?;
        signed.payload.validate(&self.binding)?;
        let (observed_at, deadline) = self.check_input_intent(&signed.payload, None).await?;
        Ok(InputTransferPermit {
            instance: self.instance.clone(),
            request: signed.payload.clone(),
            observed_at,
            deadline,
        })
    }
    /// Recheck the live fence, clock and execution state when consuming the
    /// original one-use input permit. A queued input operation cannot outlive
    /// revocation merely because it was prepared while the fence was live.
    pub async fn consume_input_transfer(
        &self,
        permit: InputTransferPermit,
    ) -> Result<LaunchRequest> {
        if permit.instance != self.instance {
            return Err(Error::Authority);
        }
        self.check_input_intent(&permit.request, Some((permit.observed_at, permit.deadline)))
            .await?;
        Ok(permit.request)
    }
    async fn check_input_intent(
        &self,
        request: &LaunchRequest,
        previous: Option<(i64, std::time::Instant)>,
    ) -> Result<(i64, std::time::Instant)> {
        let mut tx = self.pool.begin().await?;
        let high_water = self.lock(&mut tx).await?;
        let observed_instant = std::time::Instant::now();
        let now = DecisionClock::Live.sample();
        if let Some((observed, deadline)) = previous {
            if now < observed {
                return Err(Error::ClockRollback);
            }
            if observed_instant >= deadline {
                return Err(Error::Expired);
            }
        }
        request.assert_fresh(now)?;
        self.clock(&mut tx, high_water, now).await?;
        let row=sqlx::query("SELECT * FROM runner_executions WHERE attempt_id=$1 OR (run_id=$2 AND tool_call_id=$3)")
            .bind(&request.process_attempt_id).bind(&request.run_id).bind(&request.tool_call_id).fetch_optional(&mut *tx).await?;
        if let Some(row) = row {
            self.checked(row)?;
            return Err(Error::Uncertain);
        }
        self.check_fence(&mut tx, &request.run_id, request.lease_epoch)
            .await?;
        self.input_custody(&mut tx, request, Some(now)).await?;
        let fresh = self.retain_input_epoch(request, now, observed_instant)?;
        let deadline = previous.map_or(fresh, |(_, original)| original.min(fresh));
        tx.commit().await?;
        if std::time::Instant::now() >= deadline {
            return Err(Error::Expired);
        }
        Ok((now, deadline))
    }
    pub async fn admit(&self, request: Signed<LaunchRequest>) -> Result<Admission> {
        self.admit_with_clock(request, DecisionClock::Live).await
    }
    #[cfg(test)]
    pub(crate) async fn admit_at(
        &self,
        request: Signed<LaunchRequest>,
        now: i64,
    ) -> Result<Admission> {
        self.admit_with_clock(request, DecisionClock::At(now)).await
    }
    async fn admit_with_clock(
        &self,
        request: Signed<LaunchRequest>,
        clock: DecisionClock,
    ) -> Result<Admission> {
        request.verify("launch", &self.binding.control_public_key)?;
        request.payload.validate(&self.binding)?;
        let r = &request.payload;
        let mut tx = self.pool.begin().await?;
        let high_water = self.lock(&mut tx).await?;
        let now = clock.sample();
        // Replay is read-only history, including after expiry/revocation. No new permit.
        if let Some(row) = sqlx::query("SELECT * FROM runner_executions WHERE attempt_id=$1 OR (run_id=$2 AND tool_call_id=$3)")
            .bind(&r.process_attempt_id).bind(&r.run_id).bind(&r.tool_call_id).fetch_optional(&mut *tx).await? {
            let previous = self.checked(row)?;
            if previous.payload.request != *r || previous.payload.request_envelope_sha256 != envelope_hash(&request) { return Err(Error::Conflict); }
            tx.commit().await?;
            return Ok(Admission::Existing(previous));
        }
        r.assert_fresh(now)?;
        self.input_custody(&mut tx, r, None).await?;
        self.check_known_input_deadline(r)?;
        self.clock(&mut tx, high_water, now).await?;
        self.check_fence(&mut tx, &r.run_id, r.lease_epoch).await?;
        let receipt = self.signer.sign(
            "receipt",
            Receipt {
                request: r.clone(),
                request_envelope_sha256: envelope_hash(&request),
                state: ExecutionState::Admitted,
                revision: 1,
                updated_at: now,
            },
        )?;
        sqlx::query("INSERT INTO runner_executions(attempt_id,run_id,tool_call_id,request_hash,revision,payload) VALUES($1,$2,$3,$4,1,$5)")
            .bind(&r.process_attempt_id).bind(&r.run_id).bind(&r.tool_call_id).bind(&receipt.payload.request_envelope_sha256)
            .bind(serde_json::to_string(&receipt)?).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(Admission::Launch(LaunchPermit {
            instance: self.instance.clone(),
            receipt,
        }))
    }
    async fn check_fence(
        &self,
        tx: &mut Transaction<'_, Any>,
        run: &str,
        epoch: u64,
    ) -> Result<()> {
        if let Some(row) = sqlx::query("SELECT * FROM runner_fences WHERE run_id=$1")
            .bind(run)
            .fetch_optional(&mut **tx)
            .await?
        {
            let fence = self.checked_fence(row)?;
            if fence.lease_epoch != epoch || fence.revoked {
                return Err(Error::Authority);
            }
        } else {
            // The signed controller's first epoch establishes this exact run's fence.
            self.write_fence(
                tx,
                &FenceRequest {
                    runner_id: self.binding.runner_id.clone(),
                    runner_generation: self.binding.generation,
                    run_id: run.into(),
                    lease_epoch: epoch,
                    revoked: false,
                },
            )
            .await?;
        }
        Ok(())
    }
    fn checked_fence(&self, row: sqlx::any::AnyRow) -> Result<FenceRequest> {
        let signed: Signed<FenceRequest> =
            serde_json::from_str(&row.try_get::<String, _>("payload")?)?;
        signed.verify("fence", &self.binding.runner_public_key)?;
        let r = signed.payload;
        if r.runner_id != self.binding.runner_id
            || r.runner_generation != self.binding.generation
            || r.run_id != row.try_get::<String, _>("run_id")?
            || r.lease_epoch == 0
            || r.lease_epoch > i64::MAX as u64
            || r.lease_epoch as i64 != row.try_get::<i64, _>("epoch")?
            || i32::from(r.revoked) != row.try_get::<i32, _>("revoked")?
        {
            return Err(Error::Authority);
        }
        Ok(r)
    }
    async fn write_fence(
        &self,
        tx: &mut Transaction<'_, Any>,
        request: &FenceRequest,
    ) -> Result<()> {
        let signed = self.signer.sign("fence", request.clone())?;
        sqlx::query("INSERT INTO runner_fences(run_id,epoch,revoked,payload) VALUES($1,$2,$3,$4) ON CONFLICT(run_id) DO UPDATE SET epoch=excluded.epoch,revoked=excluded.revoked,payload=excluded.payload")
            .bind(&request.run_id).bind(request.lease_epoch as i64).bind(i32::from(request.revoked))
            .bind(serde_json::to_string(&signed)?).execute(&mut **tx).await?;
        Ok(())
    }
    /// Called at the qualified adapter's actual spawn boundary. Commit consumes the
    /// permit before any OS effect; a crash between this commit and spawn is uncertain.
    pub async fn dispatch(&self, permit: LaunchPermit) -> Result<LaunchRequest> {
        Ok(self
            .dispatch_with_clock(permit, DecisionClock::Live)
            .await?
            .request)
    }
    /// Preferred adapter handoff. Uses the very same transaction as dispatch;
    /// no additional execution journal or replayable dispatch token is created.
    pub async fn dispatch_owned(&self, permit: LaunchPermit) -> Result<DispatchedLaunch> {
        self.dispatch_with_clock(permit, DecisionClock::Live).await
    }
    #[cfg(test)]
    pub(crate) async fn dispatch_at(
        &self,
        permit: LaunchPermit,
        now: i64,
    ) -> Result<LaunchRequest> {
        Ok(self.dispatch_owned_at(permit, now).await?.request)
    }
    #[cfg(test)]
    pub(crate) async fn dispatch_owned_at(
        &self,
        permit: LaunchPermit,
        now: i64,
    ) -> Result<DispatchedLaunch> {
        self.dispatch_with_clock(permit, DecisionClock::At(now))
            .await
    }
    async fn dispatch_with_clock(
        &self,
        permit: LaunchPermit,
        clock: DecisionClock,
    ) -> Result<DispatchedLaunch> {
        if permit.instance != self.instance {
            return Err(Error::Authority);
        }
        let request = permit.receipt.payload.request;
        let mut tx = self.pool.begin().await?;
        let high_water = self.lock(&mut tx).await?;
        let now = clock.sample();
        self.clock(&mut tx, high_water, now).await?;
        request.assert_fresh(now)?;
        self.input_custody(&mut tx, &request, None).await?;
        self.check_known_input_deadline(&request)?;
        self.check_fence(&mut tx, &request.run_id, request.lease_epoch)
            .await?;
        let row = sqlx::query("SELECT * FROM runner_executions WHERE attempt_id=$1")
            .bind(&request.process_attempt_id)
            .fetch_one(&mut *tx)
            .await?;
        let mut receipt = self.checked(row)?.payload;
        if receipt.request != request || receipt.state != ExecutionState::Admitted {
            return Err(Error::Uncertain);
        }
        receipt.state = ExecutionState::DispatchCommitted;
        receipt.revision = 2;
        receipt.updated_at = now;
        self.write(&mut tx, &receipt, 1).await?;
        tx.commit().await?;
        Ok(DispatchedLaunch {
            request,
            instance: self.instance.clone(),
        })
    }
    async fn write(
        &self,
        tx: &mut Transaction<'_, Any>,
        receipt: &Receipt,
        expected: i64,
    ) -> Result<Signed<Receipt>> {
        receipt.validate(&self.binding)?;
        let signed = self.signer.sign("receipt", receipt.clone())?;
        if sqlx::query("UPDATE runner_executions SET revision=$1,payload=$2 WHERE attempt_id=$3 AND revision=$4")
            .bind(receipt.revision).bind(serde_json::to_string(&signed)?).bind(&receipt.request.process_attempt_id).bind(expected)
            .execute(&mut **tx).await?.rows_affected() != 1 { return Err(Error::Conflict); }
        Ok(signed)
    }
    /// Final result requires exact request identity and observed physical termination.
    /// Expired/revoked launch authority does not suppress recording an actual result.
    pub async fn complete(
        &self,
        attempt: &str,
        request_hash: &str,
        result: ProcessResult,
    ) -> Result<Signed<Receipt>> {
        validate_process_result(&result).map_err(|_| Error::Authority)?;
        let mut tx = self.pool.begin().await?;
        let high_water = self.lock(&mut tx).await?;
        let row = sqlx::query("SELECT * FROM runner_executions WHERE attempt_id=$1")
            .bind(attempt)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(Error::NotFound)?;
        let previous = self.checked(row)?;
        let mut receipt = previous.payload.clone();
        if receipt.request.request_sha256 != request_hash {
            return Err(Error::Conflict);
        }
        if let ExecutionState::Completed { result: old } = &receipt.state {
            if old != &result {
                return Err(Error::Conflict);
            }
            sqlx::query("DELETE FROM runner_input_custody WHERE attempt_id=$1")
                .bind(attempt)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            self.input_epochs
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(attempt);
            return Ok(previous);
        }
        if receipt.state != ExecutionState::DispatchCommitted {
            return Err(Error::Uncertain);
        }
        receipt.state = ExecutionState::Completed { result };
        receipt.revision = 3;
        // Completion is terminal observation, no authority repair; preserve clock HWM.
        receipt.updated_at = chrono::Utc::now()
            .timestamp_millis()
            .max(high_water)
            .max(receipt.updated_at);
        let signed = self.write(&mut tx, &receipt, 2).await?;
        sqlx::query("DELETE FROM runner_input_custody WHERE attempt_id=$1")
            .bind(attempt)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        // Only a durable terminal row makes this transient custody epoch
        // unnecessary. Expired/unadmitted epochs must not be reminted.
        self.input_epochs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(attempt);
        Ok(signed)
    }
    /// Authenticate revocation even during clock rollback. Never decrement or reopen a fence.
    pub async fn fence(&self, request: Signed<FenceRequest>) -> Result<()> {
        request.verify("fence", &self.binding.control_public_key)?;
        let r = request.payload;
        text(&r.run_id)?;
        if r.runner_id != self.binding.runner_id
            || r.runner_generation != self.binding.generation
            || r.lease_epoch == 0
            || r.lease_epoch > i64::MAX as u64
        {
            return Err(Error::Authority);
        }
        let mut tx = self.pool.begin().await?;
        self.lock(&mut tx).await?;
        if let Some(row) = sqlx::query("SELECT * FROM runner_fences WHERE run_id=$1")
            .bind(&r.run_id)
            .fetch_optional(&mut *tx)
            .await?
        {
            let previous = self.checked_fence(row)?;
            if r.lease_epoch < previous.lease_epoch
                || (r.lease_epoch == previous.lease_epoch && previous.revoked && !r.revoked)
            {
                return Err(Error::Authority);
            }
            if !r.revoked && r.lease_epoch > previous.lease_epoch {
                // A newer epoch must not give concurrent old admitted/uncertain work a new permit.
                let count: i64 = sqlx::query(
                    "SELECT COUNT(*) AS n FROM runner_executions WHERE run_id=$1 AND revision<3",
                )
                .bind(&r.run_id)
                .fetch_one(&mut *tx)
                .await?
                .try_get("n")?;
                if count != 0 {
                    return Err(Error::Uncertain);
                }
            }
        }
        self.write_fence(&mut tx, &r).await?;
        tx.commit().await?;
        Ok(())
    }
    fn checked_execution_lease(
        &self,
        row: sqlx::any::AnyRow,
        receipt: &Receipt,
    ) -> Result<Signed<ExecutionLeaseReceipt>> {
        let signed: Signed<ExecutionLeaseReceipt> =
            serde_json::from_str(&row.try_get::<String, _>("payload")?)?;
        signed.verify("execution_lease_receipt", &self.binding.runner_public_key)?;
        let lease = &signed.payload;
        lease
            .renewal
            .verify("execution_lease", &self.binding.control_public_key)?;
        let renewal = &lease.renewal.payload;
        renewal.validate(&self.binding, receipt)?;
        if renewal.process_attempt_id != row.try_get::<String, _>("attempt_id")?
            || renewal.revision as i64 != row.try_get::<i64, _>("revision")?
            || renewal.expires_at != row.try_get::<i64, _>("expires_at")?
            || lease.previous_revision < receipt.request.lease_epoch
            || lease.previous_revision >= renewal.revision
            || lease.previous_expires_at < receipt.request.expires_at
            || lease.previous_expires_at >= renewal.expires_at
            || (lease.previous_revision == receipt.request.lease_epoch)
                != (lease.previous_expires_at == receipt.request.expires_at)
            || lease.accepted_at < renewal.issued_at
            || lease.accepted_at >= lease.previous_expires_at
        {
            return Err(Error::Authority);
        }
        Ok(signed)
    }

    async fn latest_execution_lease(
        &self,
        tx: &mut Transaction<'_, Any>,
        receipt: &Receipt,
        now: i64,
    ) -> Result<(u64, i64)> {
        let row = sqlx::query("SELECT * FROM runner_execution_lease_renewals WHERE attempt_id=$1 ORDER BY revision DESC LIMIT 1")
            .bind(&receipt.request.process_attempt_id).fetch_optional(&mut **tx).await?;
        if let Some(row) = row {
            let lease = self.checked_execution_lease(row, receipt)?.payload;
            if now < lease.accepted_at {
                return Err(Error::ClockRollback);
            }
            Ok((
                lease.renewal.payload.revision,
                lease.renewal.payload.expires_at,
            ))
        } else {
            Ok((receipt.request.lease_epoch, receipt.request.expires_at))
        }
    }

    /// Extend one dispatched execution's durable lease. A lost controller stops
    /// renewing it; qualified independent supervisors must terminate on expiry.
    /// Accepted signed revisions remain immutable history. Exact replay cannot
    /// extend a lease, reopen an epoch or create dispatch authority.
    pub async fn renew_execution_lease(
        &self,
        request: Signed<ExecutionLeaseRenewal>,
    ) -> Result<()> {
        self.renew_with_clock(request, DecisionClock::Live).await
    }

    #[cfg(test)]
    pub(crate) async fn renew_execution_lease_at(
        &self,
        request: Signed<ExecutionLeaseRenewal>,
        now: i64,
    ) -> Result<()> {
        self.renew_with_clock(request, DecisionClock::At(now)).await
    }
    async fn renew_with_clock(
        &self,
        request: Signed<ExecutionLeaseRenewal>,
        clock: DecisionClock,
    ) -> Result<()> {
        request.verify("execution_lease", &self.binding.control_public_key)?;
        let mut tx = self.pool.begin().await?;
        let high_water = self.lock(&mut tx).await?;
        let now = clock.sample();
        self.clock(&mut tx, high_water, now).await?;
        let row = sqlx::query("SELECT * FROM runner_executions WHERE attempt_id=$1")
            .bind(&request.payload.process_attempt_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(Error::NotFound)?;
        let receipt = self.checked(row)?.payload;
        request.payload.validate(&self.binding, &receipt)?;
        if let Some(row) = sqlx::query(
            "SELECT * FROM runner_execution_lease_renewals WHERE attempt_id=$1 AND revision=$2",
        )
        .bind(&request.payload.process_attempt_id)
        .bind(request.payload.revision as i64)
        .fetch_optional(&mut *tx)
        .await?
        {
            let previous = self.checked_execution_lease(row, &receipt)?;
            if previous.payload.renewal != request {
                return Err(Error::Conflict);
            }
            if now < previous.payload.accepted_at {
                return Err(Error::ClockRollback);
            }
            tx.commit().await?;
            return Ok(());
        }
        if receipt.state != ExecutionState::DispatchCommitted {
            return Err(Error::Uncertain);
        }
        let (previous_revision, previous_expiry) =
            self.latest_execution_lease(&mut tx, &receipt, now).await?;
        if now >= previous_expiry || now >= receipt.request.execution_deadline {
            // Persist the observed expiry clock even on denial. A later clock
            // rollback must not resurrect the previously expired authority.
            tx.commit().await?;
            return Err(Error::Expired);
        }
        let renewal = &request.payload;
        if renewal.issued_at > now || now >= renewal.expires_at {
            return Err(Error::Expired);
        }
        if renewal.revision <= previous_revision || renewal.expires_at <= previous_expiry {
            return Err(Error::Conflict);
        }
        let row = sqlx::query("SELECT * FROM runner_fences WHERE run_id=$1")
            .bind(&receipt.request.run_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(Error::Authority)?;
        let fence = self.checked_fence(row)?;
        if fence.revoked || fence.lease_epoch != receipt.request.lease_epoch {
            return Err(Error::Authority);
        }
        let accepted = self.signer.sign(
            "execution_lease_receipt",
            ExecutionLeaseReceipt {
                renewal: request.clone(),
                previous_revision,
                previous_expires_at: previous_expiry,
                accepted_at: now,
            },
        )?;
        sqlx::query("INSERT INTO runner_execution_lease_renewals(attempt_id,revision,expires_at,payload) VALUES($1,$2,$3,$4)")
            .bind(&renewal.process_attempt_id).bind(renewal.revision as i64).bind(renewal.expires_at)
            .bind(serde_json::to_string(&accepted)?).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Qualified supervisors poll this and terminate exact owned execution on denial.
    pub async fn execution_allowed(&self, request: &LaunchRequest) -> Result<bool> {
        self.allowed_with_clock(request, DecisionClock::Live).await
    }

    #[cfg(test)]
    pub(crate) async fn execution_allowed_at(
        &self,
        request: &LaunchRequest,
        now: i64,
    ) -> Result<bool> {
        self.allowed_with_clock(request, DecisionClock::At(now))
            .await
    }
    async fn allowed_with_clock(
        &self,
        request: &LaunchRequest,
        clock: DecisionClock,
    ) -> Result<bool> {
        request.validate(&self.binding)?;
        let mut tx = self.pool.begin().await?;
        let high_water = self.lock(&mut tx).await?;
        let now = clock.sample();
        // The supervisor's copied request is not authority. Bind every poll to the
        // authenticated immutable launch that actually consumed a dispatch permit.
        let row = sqlx::query("SELECT * FROM runner_executions WHERE attempt_id=$1")
            .bind(&request.process_attempt_id)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(row) = row else {
            return Ok(false);
        };
        let receipt = self.checked(row)?;
        if receipt.payload.request != *request
            || receipt.payload.state != ExecutionState::DispatchCommitted
        {
            return Ok(false);
        }
        self.clock(&mut tx, high_water, now).await?;
        let (_, lease_expiry) = self
            .latest_execution_lease(&mut tx, &receipt.payload, now)
            .await?;
        let row = sqlx::query("SELECT * FROM runner_fences WHERE run_id=$1")
            .bind(&request.run_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(Error::Authority)?;
        let fence = self.checked_fence(row)?;
        let allowed = fence.lease_epoch == request.lease_epoch
            && !fence.revoked
            && now < lease_expiry
            && now < request.execution_deadline;
        tx.commit().await?;
        Ok(allowed)
    }
}
