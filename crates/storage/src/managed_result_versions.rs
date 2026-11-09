//! Existing-controller result reception journal. Original process completion is
//! historical evidence; current content authority is resolved in every SQL tx.
use super::*;
use cowork_broker::{
    ManagedResultStageBinding, ManagedResultStageRecord, OwnedVerifiedManagedResultStage,
};
use cowork_domain::environments::EnvironmentObservation;
use cowork_runner_protocol::ManagedResultManifest;
use serde::{Deserialize, Serialize};
use sqlx::{Any, Transaction};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManagedResultVersionStatus {
    Receiving,
    Ready,
    Retired,
}
#[path = "managed_result_deliveries.rs"]
mod delivery;
pub use delivery::{
    ManagedResultDelivery, ManagedResultDeliveryCommit, ManagedResultDeliveryState,
};

impl ManagedResultVersionStatus {
    fn name(&self) -> &'static str {
        match self {
            Self::Receiving => "receiving",
            Self::Ready => "ready",
            Self::Retired => "retired",
        }
    }
}
/// Local only: custody contains native identities and is never a public DTO.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManagedResultVersion {
    pub id: String,
    pub project_id: String,
    pub order_id: String,
    pub run_id: String,
    pub attempt_id: String,
    pub plan_id: String,
    pub plan_sha256: String,
    pub native_origin_sha256: String,
    pub result: ManagedResultManifest,
    pub custody: Option<ManagedResultStageRecord>,
    pub state: ManagedResultVersionStatus,
    pub created_at: i64,
    pub revision: i64,
}
impl ManagedResultVersion {
    pub fn stage_binding(&self) -> ManagedResultStageBinding {
        ManagedResultStageBinding {
            plan_id: self.plan_id.clone(),
            plan_sha256: self.plan_sha256.clone(),
            run_id: self.run_id.clone(),
            attempt_id: self.attempt_id.clone(),
            input_manifest_sha256: self
                .result
                .attestation
                .payload
                .input_manifest_sha256
                .clone(),
            receipt_sha256: self
                .result
                .attestation
                .payload
                .execution_receipt_sha256
                .clone(),
            attestation_sha256: canonical_hash(&self.result.attestation),
            native_origin_sha256: self.native_origin_sha256.clone(),
        }
    }
}
// This borrowed provenance contains historical facts only. In particular it
// cannot stand in for ManagedResultContext or a native Ready-read capability.
struct ResultProvenance<'a> {
    input: &'a ManagedInputTransfer,
    receipt: &'a cowork_runner_protocol::Signed<cowork_runner_protocol::Receipt>,
    plan: &'a StoredExecutionPlanV2,
    run: &'a RunRecord,
}
impl<'a> ResultProvenance<'a> {
    fn from_context(context: &'a ManagedResultContext) -> Self {
        Self {
            input: &context.input,
            receipt: &context.receipt,
            plan: &context.plan,
            run: &context.run,
        }
    }
    fn from_history(history: &'a managed_processes::ManagedProcessHistoryContext) -> Result<Self> {
        Ok(Self {
            input: &history.input,
            receipt: history.receipt.as_ref().ok_or(Error::NotFound)?,
            plan: &history.plan,
            run: &history.run,
        })
    }
}

fn validate_output(
    context: &ManagedResultContext,
    result: &ManagedResultManifest,
    instance: &str,
    at: i64,
) -> Result<()> {
    validate_output_provenance(
        &ResultProvenance::from_context(context),
        result,
        instance,
        at,
    )
}
fn validate_output_provenance(
    context: &ResultProvenance<'_>,
    result: &ManagedResultManifest,
    instance: &str,
    at: i64,
) -> Result<()> {
    result
        .attestation
        .verify(
            "managed_output",
            &context.input.grant.payload.runner.runner_public_key,
        )
        .map_err(|_| Error::Forbidden)?;
    result
        .attestation
        .payload
        .validate(
            &context.input.grant.payload,
            &context.input.request,
            context.receipt,
            &result.manifest,
            instance,
            at,
        )
        .map_err(|_| Error::Forbidden)?;
    for path in result
        .manifest
        .directories
        .iter()
        .chain(result.manifest.files.iter().map(|f| &f.path))
    {
        if !context
            .plan
            .workspace
            .copies
            .iter()
            .any(|copy| path.split('/').next() == Some(copy.alias.as_str()))
        {
            return Err(Error::Forbidden);
        }
    }
    Ok(())
}
fn validate_custody(
    version: &ManagedResultVersion,
    custody: &ManagedResultStageRecord,
) -> Result<()> {
    let binding = version.stage_binding();
    if custody.binding() != &binding
        || custody.manifest() != &version.result.manifest
        || custody.id()
            != canonical_hash(
                &json!({"format":"owner-managed-result-stage/1","binding":binding,"manifest_sha256":version.result.manifest.sha256}),
            )
    {
        return Err(Error::ExecutionPlanChanged);
    }
    Ok(())
}
fn checked(
    row: &sqlx::any::AnyRow,
    context: &ManagedResultContext,
    origin: &str,
) -> Result<ManagedResultVersion> {
    checked_provenance(row, &ResultProvenance::from_context(context), origin)
}
fn checked_history(
    row: &sqlx::any::AnyRow,
    history: &managed_processes::ManagedProcessHistoryContext,
    origin: &str,
) -> Result<ManagedResultVersion> {
    checked_provenance(row, &ResultProvenance::from_history(history)?, origin)
}
fn checked_provenance(
    row: &sqlx::any::AnyRow,
    context: &ResultProvenance<'_>,
    origin: &str,
) -> Result<ManagedResultVersion> {
    let value: ManagedResultVersion =
        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if uuid::Uuid::parse_str(&value.id).is_err()
        || canonical_hash(&value)
            != row
                .try_get::<String, _>("payload_sha256")
                .map_err(storage_error)?
        || value.id != row.try_get::<String, _>("id").map_err(storage_error)?
        || value.project_id
            != row
                .try_get::<String, _>("project_id")
                .map_err(storage_error)?
        || value.order_id
            != row
                .try_get::<String, _>("order_id")
                .map_err(storage_error)?
        || value.run_id != row.try_get::<String, _>("run_id").map_err(storage_error)?
        || value.attempt_id
            != row
                .try_get::<String, _>("attempt_id")
                .map_err(storage_error)?
        || value.plan_id != row.try_get::<String, _>("plan_id").map_err(storage_error)?
        || value.state.name() != row.try_get::<String, _>("state").map_err(storage_error)?
        || value.revision != row.try_get::<i64, _>("revision").map_err(storage_error)?
        || value.revision <= 0
        || value.project_id != context.plan.plan.project_id
        || value.order_id != context.plan.plan.order_id
        || value.run_id != context.run.id
        || value.attempt_id != context.input.request.payload.process_attempt_id
        || value.plan_id != context.plan.plan.id
        || value.plan_sha256 != context.plan.sha256
        || value.native_origin_sha256 != origin
        || value.created_at <= 0
        || value.state == ManagedResultVersionStatus::Ready && value.custody.is_none()
    {
        return Err(Error::ExecutionPlanChanged);
    }
    // Historical acquisition validation, not a renewal of remote retention.
    validate_output_provenance(
        context,
        &value.result,
        &value.result.attestation.payload.runtime_instance_sha256,
        value.created_at,
    )?;
    if let Some(custody) = &value.custody {
        validate_custody(&value, custody)?;
    }
    Ok(value)
}
async fn current(
    tx: &mut Transaction<'_, Any>,
    order: &str,
    run: &str,
    attempt: &str,
    observation: &EnvironmentObservation,
    origin: &str,
    expected_authorization: &str,
) -> Result<ManagedResultContext> {
    let context = Box::pin(managed_results::context(
        tx,
        order,
        run,
        attempt,
        observation,
        origin,
    ))
    .await?;
    if context.authorization_sha256 != expected_authorization {
        return Err(Error::ExecutionPlanChanged);
    }
    Ok(context)
}
async fn find(
    tx: &mut Transaction<'_, Any>,
    context: &ManagedResultContext,
    origin: &str,
) -> Result<Option<ManagedResultVersion>> {
    let row = sqlx::query(
        "SELECT * FROM owner_managed_result_versions WHERE attempt_id=$1 AND run_id=$2",
    )
    .bind(&context.input.request.payload.process_attempt_id)
    .bind(&context.run.id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?;
    row.as_ref()
        .map(|row| checked(row, context, origin))
        .transpose()
}
async fn write(
    tx: &mut Transaction<'_, Any>,
    old: &ManagedResultVersion,
    new: &mut ManagedResultVersion,
) -> Result<()> {
    new.revision = old.revision.checked_add(1).ok_or(Error::Conflict)?;
    if sqlx::query("UPDATE owner_managed_result_versions SET state=$1,revision=$2,payload_sha256=$3,payload=$4 WHERE id=$5 AND revision=$6")
        .bind(new.state.name()).bind(new.revision).bind(canonical_hash(new)).bind(json_string(new)?).bind(&new.id).bind(old.revision)
        .execute(&mut **tx).await.map_err(storage_error)?.rows_affected()!=1 {return Err(Error::Conflict);}
    Ok(())
}
impl Store {
    /// Local metadata only, including an unfinished receiving intent. This is
    /// deliberately separate from the Ready-only content reader below.
    pub async fn local_managed_result_metadata(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
        origin: &str,
    ) -> Result<(ManagedResultContext, Option<ManagedResultVersion>)> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await.map_err(storage_error)?;
            let context =
                managed_results::local_owner_context(&mut tx, order, run, attempt, origin).await?;
            let version = find(&mut tx, &context, origin).await?;
            if version
                .as_ref()
                .is_some_and(|v| v.state == ManagedResultVersionStatus::Retired)
            {
                return Err(Error::NotFound);
            }
            tx.commit().await.map_err(storage_error)?;
            Ok((context, version))
        })
        .await
    }

    /// Candidates come only from checked original completion history. The
    /// bounded list is complete or fails explicitly; it never truncates.
    pub async fn local_managed_result_candidates(
        &self,
        order: &str,
        run: &str,
        origin: &str,
    ) -> Result<Vec<(ManagedResultContext, Option<ManagedResultVersion>)>> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await.map_err(storage_error)?;
            let _creation = model_money::lock_creation(&mut tx).await?;
            managed_results::local_run_scope(&mut tx, order, run).await?;
            let attempts: Vec<String> = sqlx::query_scalar(
                "SELECT attempt_id FROM owner_managed_completion_receipts WHERE run_id=$1 ORDER BY attempt_id LIMIT 129",
            ).bind(run).fetch_all(&mut *tx).await.map_err(storage_error)?;
            if attempts.len() > 128 {
                return Err(Error::Validation("managed_result_list_limit_exceeded".into()));
            }
            let mut values = Vec::with_capacity(attempts.len());
            for attempt in attempts {
                let context = managed_results::local_owner_context(&mut tx, order, run, &attempt, origin).await?;
                let version = find(&mut tx, &context, origin).await?;
                if version.as_ref().is_some_and(|v| v.state == ManagedResultVersionStatus::Retired) {
                    return Err(Error::NotFound);
                }
                values.push((context, version));
            }
            tx.commit().await.map_err(storage_error)?;
            Ok(values)
        }).await
    }

    /// Resolve an opaque local version within its original order/run, then
    /// validate its complete provenance. No hash or ID acts as read authority.
    pub async fn local_ready_managed_result_version_by_id(
        &self,
        order: &str,
        run: &str,
        id: &str,
        origin: &str,
    ) -> Result<(ManagedResultContext, ManagedResultVersion)> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await.map_err(storage_error)?;
            let row = sqlx::query("SELECT * FROM owner_managed_result_versions WHERE id=$1 AND order_id=$2 AND run_id=$3")
                .bind(id).bind(order).bind(run).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or(Error::NotFound)?;
            let attempt: String = row.try_get("attempt_id").map_err(storage_error)?;
            let context = managed_results::local_owner_context(&mut tx, order, run, &attempt, origin).await?;
            let version = checked(&row, &context, origin)?;
            if version.state != ManagedResultVersionStatus::Ready {
                return Err(Error::NotFound);
            }
            tx.commit().await.map_err(storage_error)?;
            Ok((context, version))
        }).await
    }

    /// Trusted installation-Owner entry for already published local bytes only.
    /// This resolves no remote readiness and cannot prepare, attach or publish.
    pub async fn local_ready_managed_result_version(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
        origin: &str,
    ) -> Result<Option<(ManagedResultContext, ManagedResultVersion)>> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await.map_err(storage_error)?;
            let context =
                managed_results::local_owner_context(&mut tx, order, run, attempt, origin).await?;
            let version = find(&mut tx, &context, origin).await?;
            tx.commit().await.map_err(storage_error)?;
            Ok(match version {
                Some(version) if version.state == ManagedResultVersionStatus::Ready => {
                    Some((context, version))
                }
                Some(version) if version.state == ManagedResultVersionStatus::Retired => {
                    return Err(Error::NotFound);
                }
                _ => None,
            })
        })
        .await
    }

    pub async fn managed_result_version(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
        observation: &EnvironmentObservation,
        origin: &str,
        authorization: &str,
    ) -> Result<Option<ManagedResultVersion>> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await.map_err(storage_error)?;
            let context = current(
                &mut tx,
                order,
                run,
                attempt,
                observation,
                origin,
                authorization,
            )
            .await?;
            let version = find(&mut tx, &context, origin).await?;
            tx.commit().await.map_err(storage_error)?;
            Ok(version)
        })
        .await
    }
    /// Immutable receiving intent is durable before native allocation begins.
    #[expect(
        clippy::too_many_arguments,
        reason = "prepare_managed_result_version: Keep original order/run/attempt bindings, current environment observation and result custody authorization explicit"
    )]
    pub async fn prepare_managed_result_version(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
        result: &ManagedResultManifest,
        instance: &str,
        observation: &EnvironmentObservation,
        origin: &str,
        authorization: &str,
    ) -> Result<ManagedResultVersion> {
        Box::pin(async move {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let context = current(
            &mut tx,
            order,
            run,
            attempt,
            observation,
            origin,
            authorization,
        )
        .await?;
        if let Some(old) = find(&mut tx, &context, origin).await? {
            if old.result != *result || old.state == ManagedResultVersionStatus::Retired {
                return Err(Error::Conflict);
            }
            tx.commit().await.map_err(storage_error)?;
            return Ok(old);
        }
        validate_output(&context, result, instance, now())?;
        // This SQL reservation also bounds manifests for requests that never
        // reach native creation. Native orphan reservations stay charged too.
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM owner_managed_result_versions WHERE state <> 'retired'",
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
        if count >= 8 {
            return Err(Error::Validation(
                "managed_result_version_capacity_exceeded".into(),
            ));
        }
        let version = ManagedResultVersion {
            id: Uuid::new_v4().to_string(),
            project_id: context.plan.plan.project_id.clone(),
            order_id: order.into(),
            run_id: run.into(),
            attempt_id: attempt.into(),
            plan_id: context.plan.plan.id.clone(),
            plan_sha256: context.plan.sha256.clone(),
            native_origin_sha256: origin.into(),
            result: result.clone(),
            custody: None,
            state: ManagedResultVersionStatus::Receiving,
            created_at: now(),
            revision: 1,
        };
        validate_output(&context, &version.result, instance, version.created_at)?;
        sqlx::query("INSERT INTO owner_managed_result_versions(id,project_id,order_id,run_id,attempt_id,plan_id,state,revision,payload_sha256,payload) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
            .bind(&version.id).bind(&version.project_id).bind(&version.order_id).bind(&version.run_id).bind(&version.attempt_id).bind(&version.plan_id).bind(version.state.name()).bind(version.revision).bind(canonical_hash(&version)).bind(json_string(&version)?).execute(&mut *tx).await.map_err(storage_error)?;
        tx.commit().await.map_err(storage_error)?;
        Ok(version)
        }).await
    }
    /// NULL to exact native preparation receipt only; no custody rebinding.
    #[expect(
        clippy::too_many_arguments,
        reason = "attach_managed_result_custody: Keep original order/run/attempt bindings, current environment observation and result custody authorization explicit"
    )]
    pub async fn attach_managed_result_custody(
        &self,
        id: &str,
        custody: &ManagedResultStageRecord,
        order: &str,
        run: &str,
        attempt: &str,
        observation: &EnvironmentObservation,
        origin: &str,
        authorization: &str,
    ) -> Result<ManagedResultVersion> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await.map_err(storage_error)?;
            let context = current(
                &mut tx,
                order,
                run,
                attempt,
                observation,
                origin,
                authorization,
            )
            .await?;
            let old = find(&mut tx, &context, origin)
                .await?
                .ok_or(Error::NotFound)?;
            if old.id != id || old.state == ManagedResultVersionStatus::Retired {
                return Err(Error::Conflict);
            }
            validate_custody(&old, custody)?;
            let mut version = old.clone();
            match &old.custody {
                Some(original) if original == custody => {}
                Some(_) => return Err(Error::Conflict),
                None => {
                    version.custody = Some(custody.clone());
                    write(&mut tx, &old, &mut version).await?;
                }
            }
            tx.commit().await.map_err(storage_error)?;
            Ok(version)
        })
        .await
    }
    /// The native full-hash capability and original descriptor/lock survive
    /// through this SQL commit. No native operation is invoked inside the tx.
    #[expect(
        clippy::too_many_arguments,
        reason = "publish_managed_result_version: Keep original order/run/attempt bindings, current environment observation and result custody authorization explicit"
    )]
    pub async fn publish_managed_result_version(
        &self,
        id: &str,
        verified: &OwnedVerifiedManagedResultStage,
        order: &str,
        run: &str,
        attempt: &str,
        observation: &EnvironmentObservation,
        origin: &str,
        authorization: &str,
    ) -> Result<ManagedResultVersion> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await.map_err(storage_error)?;
            let context = current(
                &mut tx,
                order,
                run,
                attempt,
                observation,
                origin,
                authorization,
            )
            .await?;
            let old = find(&mut tx, &context, origin)
                .await?
                .ok_or(Error::NotFound)?;
            if old.id != id
                || old.state == ManagedResultVersionStatus::Retired
                || old.custody.as_ref() != Some(verified.record())
            {
                return Err(Error::Conflict);
            }
            validate_custody(&old, verified.record())?;
            let mut version = old.clone();
            if version.state != ManagedResultVersionStatus::Ready {
                version.state = ManagedResultVersionStatus::Ready;
                write(&mut tx, &old, &mut version).await?;
            }
            tx.commit().await.map_err(storage_error)?;
            Ok(version)
        })
        .await
    }
}
