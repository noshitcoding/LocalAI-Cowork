//! Private immutable delivery intent and separately progressing retirement history.
//! Only an already verified local Ready version can prepare a delivery. Neither
//! a receipt nor a database row is a native volume-retirement capability.
use super::*;
use cowork_runner_protocol::{
    ManagedResultDeliveryReceipt, ManagedResultDeliveryRequest, ManagedResultRetirementReceipt,
    Signed,
};
use serde::{Deserialize, Serialize};

pub const MAX_MANAGED_DELIVERY_INTENTS_PER_VERSION: usize = 16;
pub const MAX_MANAGED_DELIVERY_INTENTS_TOTAL: i64 = 4096;
const MAX_RECORD_BYTES: usize = 65536;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManagedResultDeliveryState {
    Prepared,
    Committed,
    Expired,
    Accepted,
    Retired,
    Unconfirmed,
}
impl ManagedResultDeliveryState {
    fn name(&self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Committed => "committed",
            Self::Expired => "expired",
            Self::Accepted => "accepted",
            Self::Retired => "retired",
            Self::Unconfirmed => "unconfirmed",
        }
    }
}

/// Native origin and version revision stay in the controller journal. Only the
/// portable request is sent to the native service, signed by its actual client.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManagedResultDelivery {
    pub id: String,
    pub result_version_id: String,
    pub result_version_revision: i64,
    pub run_id: String,
    pub attempt_id: String,
    pub plan_id: String,
    pub native_origin_sha256: String,
    pub physical_completion_sha256: String,
    pub ordinal: i64,
    pub request: ManagedResultDeliveryRequest,
    pub state: ManagedResultDeliveryState,
    pub accepted: Option<Signed<ManagedResultDeliveryReceipt>>,
    pub retirement: Option<Signed<ManagedResultRetirementReceipt>>,
    pub created_at: i64,
    pub revision: i64,
}
use cowork_broker::OwnedVerifiedManagedResultStage;
use cowork_runner_protocol::{ManagedResultRetirementState, envelope_hash};
use sqlx::{Any, Transaction};

/// One original confirmation and its currently held local pack capability.
/// This in-process parameter record is neither portable nor deserializable.
pub struct ManagedResultDeliveryCommit<'a> {
    pub order: &'a str,
    pub run: &'a str,
    pub attempt: &'a str,
    pub origin: &'a str,
    pub id: &'a str,
    pub expected_request_sha256: &'a str,
    pub authorization: &'a str,
    pub verified: &'a OwnedVerifiedManagedResultStage,
}

fn binding(version: &ManagedResultVersion) -> cowork_runner_protocol::Binding {
    managed_inputs::binding(&version.result.attestation.payload.runner)
}

async fn original(
    tx: &mut Transaction<'_, Any>,
    order: &str,
    run: &str,
    attempt: &str,
    origin: &str,
) -> Result<(
    managed_processes::ManagedProcessHistoryContext,
    ManagedResultVersion,
)> {
    let history = managed_processes::delivery_history(tx, order, run, attempt, origin).await?;
    let row = sqlx::query(
        "SELECT * FROM owner_managed_result_versions WHERE attempt_id=$1 AND run_id=$2",
    )
    .bind(attempt)
    .bind(run)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?
    .ok_or(Error::NotFound)?;
    let version = checked_history(&row, &history, origin)?;
    Ok((history, version))
}

fn checked_delivery(
    row: &sqlx::any::AnyRow,
    history: &managed_processes::ManagedProcessHistoryContext,
    version: &ManagedResultVersion,
    origin: &str,
) -> Result<ManagedResultDelivery> {
    let raw: String = row.try_get("payload").map_err(storage_error)?;
    if raw.len() > MAX_RECORD_BYTES {
        return Err(Error::ExecutionPlanChanged);
    }
    let value: ManagedResultDelivery = parse(&raw)?;
    let request = &value.request;
    let receipt = history.receipt.as_ref().ok_or(Error::NotFound)?;
    let input = &history.input.grant.payload;
    request
        .validate(
            &binding(version),
            &input.target,
            &version.result.attestation.payload.runtime_instance_sha256,
            request.issued_at,
        )
        .map_err(|_| Error::ExecutionPlanChanged)?;
    if canonical_hash(&value)
        != row
            .try_get::<String, _>("payload_sha256")
            .map_err(storage_error)?
        || value.id != row.try_get::<String, _>("id").map_err(storage_error)?
        || value.result_version_id
            != row
                .try_get::<String, _>("result_version_id")
                .map_err(storage_error)?
        || value.run_id != row.try_get::<String, _>("run_id").map_err(storage_error)?
        || value.attempt_id
            != row
                .try_get::<String, _>("attempt_id")
                .map_err(storage_error)?
        || value.plan_id != row.try_get::<String, _>("plan_id").map_err(storage_error)?
        || value.ordinal != row.try_get::<i64, _>("ordinal").map_err(storage_error)?
        || value.state.name() != row.try_get::<String, _>("state").map_err(storage_error)?
        || value.revision != row.try_get::<i64, _>("revision").map_err(storage_error)?
        || value.revision <= 0
        || value.ordinal <= 0
        || value.ordinal > MAX_MANAGED_DELIVERY_INTENTS_PER_VERSION as i64
        || value.created_at != request.issued_at
        || value.native_origin_sha256 != origin
        || value.result_version_id != version.id
        || value.result_version_revision <= 0
        || value.result_version_revision > version.revision
        || value.run_id != version.run_id
        || value.attempt_id != version.attempt_id
        || value.plan_id != version.plan_id
        || value.physical_completion_sha256
            != version.result.attestation.payload.physical_record_sha256
        || value.id != request.request_id
        || request.recipient_version_id != version.id
        || request.actor_id
            != format!(
                "installation-owner:{}",
                history.plan.plan.target.controller().id
            )
        || request.execution_plan_id != history.plan.plan.id
        || request.execution_plan_sha256 != history.plan.sha256
        || request.run_id != history.run.id
        || request.process_attempt_id != input.process_attempt_id
        || request.launch_envelope_sha256 != envelope_hash(&history.input.request)
        || request.execution_receipt_sha256 != canonical_hash(receipt)
        || request.output_attestation_sha256 != canonical_hash(&version.result.attestation)
        || request.manifest_sha256 != version.result.manifest.sha256
        || request.runner != input.runner
        || request.target != input.target
    {
        return Err(Error::ExecutionPlanChanged);
    }
    if let Some(accepted) = &value.accepted {
        accepted
            .verify(
                "managed_result_delivery_receipt",
                &request.runner.runner_public_key,
            )
            .map_err(|_| Error::ExecutionPlanChanged)?;
        if accepted.payload.request.payload != *request {
            return Err(Error::ExecutionPlanChanged);
        }
        accepted
            .payload
            .validate(&accepted.payload.request, &binding(version))
            .map_err(|_| Error::ExecutionPlanChanged)?;
    }
    if let Some(retirement) = &value.retirement {
        let accepted = value.accepted.as_ref().ok_or(Error::ExecutionPlanChanged)?;
        retirement
            .verify(
                "managed_result_retirement_receipt",
                &request.runner.runner_public_key,
            )
            .map_err(|_| Error::ExecutionPlanChanged)?;
        retirement
            .payload
            .validate(
                accepted,
                &binding(version),
                &version.result.attestation.payload.physical_record_sha256,
            )
            .map_err(|_| Error::ExecutionPlanChanged)?;
    }
    let valid_state = match value.state {
        ManagedResultDeliveryState::Prepared
        | ManagedResultDeliveryState::Committed
        | ManagedResultDeliveryState::Expired => {
            value.accepted.is_none() && value.retirement.is_none()
        }
        ManagedResultDeliveryState::Accepted => {
            value.accepted.is_some()
                && value
                    .retirement
                    .as_ref()
                    .is_none_or(|r| r.payload.state == ManagedResultRetirementState::Pending)
        }
        ManagedResultDeliveryState::Retired => {
            value.accepted.is_some()
                && value
                    .retirement
                    .as_ref()
                    .is_some_and(|r| r.payload.state == ManagedResultRetirementState::Retired)
        }
        ManagedResultDeliveryState::Unconfirmed => {
            value.accepted.is_some()
                && value
                    .retirement
                    .as_ref()
                    .is_some_and(|r| r.payload.state == ManagedResultRetirementState::Unconfirmed)
        }
    };
    if !valid_state {
        return Err(Error::ExecutionPlanChanged);
    }
    Ok(value)
}

async fn latest(
    tx: &mut Transaction<'_, Any>,
    history: &managed_processes::ManagedProcessHistoryContext,
    version: &ManagedResultVersion,
    origin: &str,
) -> Result<Option<ManagedResultDelivery>> {
    let row = sqlx::query("SELECT * FROM owner_managed_result_deliveries WHERE result_version_id=$1 ORDER BY ordinal DESC LIMIT 1")
        .bind(&version.id).fetch_optional(&mut **tx).await.map_err(storage_error)?;
    row.as_ref()
        .map(|row| checked_delivery(row, history, version, origin))
        .transpose()
}

async fn replace(
    tx: &mut Transaction<'_, Any>,
    old: &ManagedResultDelivery,
    next: &mut ManagedResultDelivery,
) -> Result<()> {
    next.revision = old.revision.checked_add(1).ok_or(Error::Conflict)?;
    let payload = json_string(next)?;
    if payload.len() > MAX_RECORD_BYTES {
        return Err(Error::Validation(
            "managed_result_delivery_record_limit".into(),
        ));
    }
    if sqlx::query("UPDATE owner_managed_result_deliveries SET state=$1,revision=$2,payload_sha256=$3,payload=$4 WHERE id=$5 AND revision=$6")
        .bind(next.state.name()).bind(next.revision).bind(canonical_hash(next)).bind(payload).bind(&next.id).bind(old.revision)
        .execute(&mut **tx).await.map_err(storage_error)?.rows_affected() != 1 { return Err(Error::Conflict); }
    Ok(())
}

impl Store {
    /// Prepare from original SQL provenance and a live, fully verified native
    /// Ready capability held through commit. No renderer supplies request facts.
    pub async fn prepare_managed_result_delivery(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
        origin: &str,
        authorization: &str,
        verified: &OwnedVerifiedManagedResultStage,
    ) -> Result<ManagedResultDelivery> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await.map_err(storage_error)?;
            let context = managed_results::local_owner_context(&mut tx, order, run, attempt, origin).await?;
            if context.authorization_sha256 != authorization { return Err(Error::ExecutionPlanChanged); }
            let version = find(&mut tx, &context, origin).await?.ok_or(Error::NotFound)?;
            if version.state != ManagedResultVersionStatus::Ready || version.custody.as_ref() != Some(verified.record()) { return Err(Error::Forbidden); }
            validate_custody(&version, verified.record())?;
            let (history, current) = original(&mut tx, order, run, attempt, origin).await?;
            if current != version { return Err(Error::ExecutionPlanChanged); }
            let previous = latest(&mut tx, &history, &version, origin).await?;
            if let Some(existing) = &previous
                && existing.state != ManagedResultDeliveryState::Expired {
                tx.commit().await.map_err(storage_error)?;
                return Ok(existing.clone());
            }
            let ordinal = previous.as_ref().map_or(Ok(1), |p| p.ordinal.checked_add(1).ok_or(Error::Conflict))?;
            if ordinal > MAX_MANAGED_DELIVERY_INTENTS_PER_VERSION as i64 { return Err(Error::Validation("managed_result_delivery_intent_limit".into())); }
            let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM owner_managed_result_deliveries")
                .fetch_one(&mut *tx).await.map_err(storage_error)?;
            if total >= MAX_MANAGED_DELIVERY_INTENTS_TOTAL { return Err(Error::Validation("managed_result_delivery_journal_full".into())); }
            let issued_at = now();
            let grant = &context.input.grant.payload;
            let request = ManagedResultDeliveryRequest {
                format_version: 1, request_id: uuid::Uuid::new_v4().to_string(),
                actor_id: context.actor_id.clone(), authorization_sha256: context.authorization_sha256.clone(),
                runtime_instance_sha256: version.result.attestation.payload.runtime_instance_sha256.clone(),
                execution_plan_id: context.plan.plan.id.clone(), execution_plan_sha256: context.plan.sha256.clone(),
                run_id: run.into(), process_attempt_id: attempt.into(), launch_envelope_sha256: envelope_hash(&context.input.request),
                execution_receipt_sha256: canonical_hash(&context.receipt), output_attestation_sha256: canonical_hash(&version.result.attestation),
                manifest_sha256: version.result.manifest.sha256.clone(), recipient_version_id: version.id.clone(),
                runner: grant.runner.clone(), target: grant.target.clone(), issued_at,
                accept_before: issued_at.checked_add(cowork_runner_protocol::MANAGED_RESULT_DELIVERY_ACCEPT_MILLIS).ok_or(Error::Conflict)?,
            };
            request.validate(&binding(&version), &grant.target, &request.runtime_instance_sha256, issued_at)
                .map_err(|_| Error::Forbidden)?;
            let value = ManagedResultDelivery {
                id: request.request_id.clone(), result_version_id: version.id.clone(), result_version_revision: version.revision,
                run_id: run.into(), attempt_id: attempt.into(), plan_id: version.plan_id.clone(), native_origin_sha256: origin.into(),
                physical_completion_sha256: version.result.attestation.payload.physical_record_sha256.clone(),
                ordinal, request, state: ManagedResultDeliveryState::Prepared, accepted: None, retirement: None,
                created_at: issued_at, revision: 1,
            };
            let payload = json_string(&value)?;
            if payload.len() > MAX_RECORD_BYTES { return Err(Error::Validation("managed_result_delivery_record_limit".into())); }
            sqlx::query("INSERT INTO owner_managed_result_deliveries(id,result_version_id,run_id,attempt_id,plan_id,ordinal,state,revision,payload_sha256,payload) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
                .bind(&value.id).bind(&value.result_version_id).bind(run).bind(attempt).bind(&value.plan_id).bind(value.ordinal)
                .bind(value.state.name()).bind(value.revision).bind(canonical_hash(&value)).bind(payload)
                .execute(&mut *tx).await.map_err(storage_error)?;
            tx.commit().await.map_err(storage_error)?;
            Ok(value)
        }).await
    }

    /// Local original history only. This neither contacts the runner nor grants
    /// content access and can preserve a signed acceptance after cancellation.
    pub async fn managed_result_delivery(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
        origin: &str,
    ) -> Result<Option<ManagedResultDelivery>> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await.map_err(storage_error)?;
            let _creation = model_money::lock_creation(&mut tx).await?;
            let (history, version) = original(&mut tx, order, run, attempt, origin).await?;
            let value = latest(&mut tx, &history, &version, origin).await?;
            tx.commit().await.map_err(storage_error)?;
            Ok(value)
        })
        .await
    }

    /// Commit exactly the prepared request before any network effect. An exact
    /// committed replay preserves every original byte, including its deadline.
    pub async fn commit_managed_result_delivery(
        &self,
        confirmation: ManagedResultDeliveryCommit<'_>,
    ) -> Result<ManagedResultDelivery> {
        let ManagedResultDeliveryCommit {
            order,
            run,
            attempt,
            origin,
            id,
            expected_request_sha256,
            authorization,
            verified,
        } = confirmation;
        Box::pin(async move {
            let mut tx = self.pool.begin().await.map_err(storage_error)?;
            let context =
                managed_results::local_owner_context(&mut tx, order, run, attempt, origin).await?;
            if context.authorization_sha256 != authorization {
                return Err(Error::ExecutionPlanChanged);
            }
            let version = find(&mut tx, &context, origin)
                .await?
                .ok_or(Error::NotFound)?;
            if version.state != ManagedResultVersionStatus::Ready
                || version.custody.as_ref() != Some(verified.record())
            {
                return Err(Error::Forbidden);
            }
            validate_custody(&version, verified.record())?;
            let (history, current) = original(&mut tx, order, run, attempt, origin).await?;
            if current != version {
                return Err(Error::ExecutionPlanChanged);
            }
            let old = latest(&mut tx, &history, &version, origin)
                .await?
                .ok_or(Error::NotFound)?;
            if old.id != id || canonical_hash(&old.request) != expected_request_sha256 {
                return Err(Error::Conflict);
            }
            let mut value = old.clone();
            if value.state == ManagedResultDeliveryState::Prepared {
                if value.result_version_revision != version.revision
                    || value.request.authorization_sha256 != authorization
                {
                    return Err(Error::ExecutionPlanChanged);
                }
                if now() >= value.request.accept_before {
                    value.state = ManagedResultDeliveryState::Expired;
                    replace(&mut tx, &old, &mut value).await?;
                    tx.commit().await.map_err(storage_error)?;
                    return Err(Error::Validation(
                        "managed_result_delivery_confirmation_expired".into(),
                    ));
                }
                value
                    .request
                    .validate(
                        &binding(&version),
                        &value.request.target,
                        &value.request.runtime_instance_sha256,
                        now(),
                    )
                    .map_err(|_| Error::Forbidden)?;
                value.state = ManagedResultDeliveryState::Committed;
                replace(&mut tx, &old, &mut value).await?;
            } else if value.state == ManagedResultDeliveryState::Expired {
                return Err(Error::Conflict);
            }
            tx.commit().await.map_err(storage_error)?;
            Ok(value)
        })
        .await
    }

    /// Trusted transport calls this only for an authenticated, explicit native
    /// never-accepted expiry. Timeout, unavailable or an absent journal cannot
    /// mark the original committed intent expired or prepare a replacement.
    pub async fn expire_unaccepted_managed_result_delivery(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
        origin: &str,
        id: &str,
        expected_request_sha256: &str,
    ) -> Result<ManagedResultDelivery> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await.map_err(storage_error)?;
            let _creation = model_money::lock_creation(&mut tx).await?;
            let (history, version) = original(&mut tx, order, run, attempt, origin).await?;
            let old = latest(&mut tx, &history, &version, origin)
                .await?
                .ok_or(Error::NotFound)?;
            if old.id != id
                || canonical_hash(&old.request) != expected_request_sha256
                || old.accepted.is_some()
                || !matches!(
                    old.state,
                    ManagedResultDeliveryState::Committed | ManagedResultDeliveryState::Expired
                )
                || now() < old.request.accept_before
            {
                return Err(Error::Conflict);
            }
            let mut value = old.clone();
            if value.state != ManagedResultDeliveryState::Expired {
                value.state = ManagedResultDeliveryState::Expired;
                replace(&mut tx, &old, &mut value).await?;
            }
            tx.commit().await.map_err(storage_error)?;
            Ok(value)
        })
        .await
    }

    /// Preserve the original signed acceptance even if source rights changed
    /// during the reply. This is history only, never a fresh read capability.
    pub async fn retain_managed_result_delivery_receipt(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
        origin: &str,
        id: &str,
        receipt: &Signed<ManagedResultDeliveryReceipt>,
    ) -> Result<ManagedResultDelivery> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await.map_err(storage_error)?;
            let _creation = model_money::lock_creation(&mut tx).await?;
            let (history, version) = original(&mut tx, order, run, attempt, origin).await?;
            let row = sqlx::query("SELECT * FROM owner_managed_result_deliveries WHERE id=$1 AND result_version_id=$2")
                .bind(id).bind(&version.id).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
            let old = checked_delivery(&row, &history, &version, origin)?;
            if old.request != receipt.payload.request.payload || old.state == ManagedResultDeliveryState::Prepared { return Err(Error::Conflict); }
            receipt.verify("managed_result_delivery_receipt", &old.request.runner.runner_public_key).map_err(|_| Error::Forbidden)?;
            receipt.payload.validate(&receipt.payload.request, &binding(&version)).map_err(|_| Error::Forbidden)?;
            let mut value = old.clone();
            match &value.accepted {
                Some(original) if original == receipt => {}
                Some(_) => return Err(Error::Conflict),
                None if value.state == ManagedResultDeliveryState::Committed => {
                    value.accepted = Some(receipt.clone()); value.state = ManagedResultDeliveryState::Accepted;
                    replace(&mut tx, &old, &mut value).await?;
                }
                None => return Err(Error::Conflict),
            }
            tx.commit().await.map_err(storage_error)?;
            Ok(value)
        }).await
    }

    pub async fn retain_managed_result_retirement_receipt(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
        origin: &str,
        id: &str,
        receipt: &Signed<ManagedResultRetirementReceipt>,
    ) -> Result<ManagedResultDelivery> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await.map_err(storage_error)?;
            let _creation = model_money::lock_creation(&mut tx).await?;
            let (history, version) = original(&mut tx, order, run, attempt, origin).await?;
            let row = sqlx::query("SELECT * FROM owner_managed_result_deliveries WHERE id=$1 AND result_version_id=$2")
                .bind(id).bind(&version.id).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
            let old = checked_delivery(&row, &history, &version, origin)?;
            let accepted = old.accepted.as_ref().ok_or(Error::Conflict)?;
            receipt.verify("managed_result_retirement_receipt", &old.request.runner.runner_public_key).map_err(|_| Error::Forbidden)?;
            receipt.payload.validate(accepted, &binding(&version), &version.result.attestation.payload.physical_record_sha256).map_err(|_| Error::Forbidden)?;
            if let Some(previous) = &old.retirement {
                if receipt == previous { tx.commit().await.map_err(storage_error)?; return Ok(old); }
                if receipt.payload.revision <= previous.payload.revision || receipt.payload.recorded_at < previous.payload.recorded_at
                    || previous.payload.state == ManagedResultRetirementState::Retired { return Err(Error::Conflict); }
            }
            let mut value = old.clone();
            value.state = match receipt.payload.state {
                ManagedResultRetirementState::Pending => ManagedResultDeliveryState::Accepted,
                ManagedResultRetirementState::Retired => ManagedResultDeliveryState::Retired,
                ManagedResultRetirementState::Unconfirmed => ManagedResultDeliveryState::Unconfirmed,
            };
            value.retirement = Some(receipt.clone());
            replace(&mut tx, &old, &mut value).await?;
            tx.commit().await.map_err(storage_error)?;
            Ok(value)
        }).await
    }
}
