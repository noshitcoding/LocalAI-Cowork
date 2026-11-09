//! Fresh local-owner content authority for an already journaled managed result.
//! The original signed history is necessary but does not itself authorize reads.
use super::*;
use cowork_domain::environments::{EnvironmentObservation, EnvironmentPin};
use cowork_runner_protocol::{Receipt, Signed};

/// Private controller context. Native custody and source grants must never be
/// serialized into a renderer response or sent to the managed service.
pub struct ManagedResultContext {
    pub input: ManagedInputTransfer,
    pub receipt: Signed<Receipt>,
    pub plan: StoredExecutionPlanV2,
    pub run: RunRecord,
    pub workspace: Value,
    pub actor_id: String,
    pub authorization_sha256: String,
}

impl Store {
    /// Trusted installation-owner entry only. Team/device requests must resolve
    /// their own actual principal; no caller-supplied actor or digest is accepted.
    pub async fn managed_result_context(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
        observation: &EnvironmentObservation,
        native_origin: &str,
    ) -> Result<ManagedResultContext> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let value = context(&mut tx, order, run, attempt, observation, native_origin).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(value)
    }
}

pub(super) async fn context(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    order: &str,
    run: &str,
    attempt: &str,
    observation: &EnvironmentObservation,
    native_origin: &str,
) -> Result<ManagedResultContext> {
    Box::pin(context_inner(
        tx,
        order,
        run,
        attempt,
        Some(observation),
        native_origin,
    ))
    .await
}

/// Historical result provenance plus current local Owner/controller authority.
/// Local metadata and Ready reads only; acquisition still uses context.
pub(super) async fn local_owner_context(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    order: &str,
    run: &str,
    attempt: &str,
    native_origin: &str,
) -> Result<ManagedResultContext> {
    Box::pin(context_inner(tx, order, run, attempt, None, native_origin)).await
}

async fn context_inner(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    order: &str,
    run: &str,
    attempt: &str,
    observation: Option<&EnvironmentObservation>,
    native_origin: &str,
) -> Result<ManagedResultContext> {
    let _creation = model_money::lock_creation(tx).await?;
    let (stored, owner) = local_run_scope(tx, order, run).await?;
    let row = sqlx::query(
        "SELECT * FROM owner_managed_completion_receipts WHERE attempt_id=$1 AND run_id=$2",
    )
    .bind(attempt)
    .bind(run)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?
    .ok_or(Error::NotFound)?;
    let receipt = managed_completions::checked(tx, &row).await?;
    let row = sqlx::query(
        "SELECT * FROM owner_managed_input_transfers WHERE attempt_id=$1 AND run_id=$2",
    )
    .bind(attempt)
    .bind(run)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    let input = managed_inputs::checked(&row)?;
    let row = sqlx::query("SELECT * FROM owner_execution_plans_v2 WHERE id=$1 AND run_id=$2")
        .bind(&input.grant.payload.execution_plan_id)
        .bind(run)
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
    let plan = execution_plans_v2::checked(&row)?;
    if row.try_get::<i64, _>("retired").map_err(storage_error)? != 0
        || canonical_hash(&stored.record.spec) != plan.plan.spec_sha256
        || plan.plan.order_id != order
        || plan.plan.project_id != owner.project_id
    {
        return Err(Error::ExecutionPlanChanged);
    }
    let row = sqlx::query(
        "SELECT id,tool_call_id,revision,payload FROM process_attempts WHERE id=$1 AND run_id=$2",
    )
    .bind(attempt)
    .bind(run)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage_error)?;
    let process = processes::checked_record(&row, run)?;
    if !matches!(
        process.record.state,
        ProcessAttemptState::Completed { .. } | ProcessAttemptState::ReconciledCompleted { .. }
    ) {
        return Err(Error::Forbidden);
    }
    if let Some(observation) = observation {
        execution_controllers::require_current(tx, &stored.record.spec, Some(native_origin))
            .await?;
        let reference = stored
            .record
            .spec
            .environment
            .as_ref()
            .ok_or(Error::Forbidden)?;
        let row =
            sqlx::query("SELECT revision,payload FROM environments WHERE project_id=$1 AND id=$2")
                .bind(&owner.project_id)
                .bind(&reference.environment_id)
                .fetch_one(&mut **tx)
                .await
                .map_err(storage_error)?;
        let environment =
            environments::checked(&row, &owner.project_id, &reference.environment_id)?;
        let pin = EnvironmentPin {
            environment_id: reference.environment_id.clone(),
            project_id: owner.project_id.clone(),
            version_id: reference.version_id.clone(),
            definition_sha256: reference.definition_sha256.clone(),
            qualification_sha256: reference.qualification_sha256.clone(),
            definition: environment
                .version(&reference.version_id)?
                .definition
                .clone(),
        };
        environment.resolve_pin(&pin, observation, now())?;
    } else if execution_controllers::locked(tx, native_origin).await?
        != *plan.plan.target.controller()
    {
        // Keep the exact original local controller and native origin. Only the
        // remote execution target/environment readiness is irrelevant to bytes
        // already fully acquired by this controller.
        return Err(Error::ExecutionPlanChanged);
    }
    let row=sqlx::query("SELECT payload,custody_sha256,workspace_id FROM owner_run_workspaces WHERE run_id=$1 AND plan_id=$2").bind(run).bind(&plan.plan.id)
            .fetch_one(&mut **tx).await.map_err(storage_error)?;
    let workspace: Value = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    let custody_sha = row
        .try_get::<String, _>("custody_sha256")
        .map_err(storage_error)?;
    if canonical_hash(&workspace) != custody_sha
        || workspace["intent"] != serde_json::to_value(&plan.workspace).map_err(storage_error)?
        || row
            .try_get::<String, _>("workspace_id")
            .map_err(storage_error)?
            != plan.workspace.binding.id
    {
        return Err(Error::ExecutionPlanChanged);
    }
    let actor_id = format!("installation-owner:{}", plan.plan.target.controller().id);
    let authorization_sha256 = canonical_hash(
        &json!({"format":if observation.is_some(){"managed_result_owner_authority/1"}else{"managed_local_ready_owner_authority/1"},"actor_id":actor_id,
            "plan_sha256":plan.sha256,"run_spec_sha256":plan.plan.spec_sha256,"process":process.record,
            "receipt_sha256":canonical_hash(&receipt),"input_grant_sha256":canonical_hash(&input.grant),
            "workspace_custody_sha256":custody_sha,"target":plan.plan.target,"source_grants":plan.source_spec.folders}),
    );
    Ok(ManagedResultContext {
        input,
        receipt,
        plan,
        run: stored.record,
        workspace,
        actor_id,
        authorization_sha256,
    })
}

/// Scope empty candidate lists too, without issuing content or execution rights.
pub(super) async fn local_run_scope(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    order: &str,
    run: &str,
) -> Result<(StoredRun, Order)> {
    // Serialize with cancellation before reading its state. A new read
    // cannot authorize against a pre-cancellation snapshot of this row.
    if sqlx::query("UPDATE runs SET revision=revision WHERE id=$1")
        .bind(run)
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?
        .rows_affected()
        != 1
    {
        return Err(Error::NotFound);
    }
    let row = sqlx::query("SELECT revision,payload FROM runs WHERE id=$1")
        .bind(run)
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
    let stored: StoredRun = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if stored.record.id != run
        || stored.record.revision != row.try_get::<i64, _>("revision").map_err(storage_error)?
        || stored.record.spec.order_id != order
        || matches!(
            stored.record.state,
            RunState::CancelRequested | RunState::Canceled
        )
        || stored.record.spec.workflow.is_some()
    {
        return Err(Error::Forbidden);
    }
    let row = sqlx::query("SELECT payload FROM orders WHERE id=$1 AND project_id=$2")
        .bind(order)
        .bind(&stored.record.spec.project_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
    let owner: Order = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if owner.id != order || owner.project_id != stored.record.spec.project_id {
        return Err(Error::Forbidden);
    }
    Ok((stored, owner))
}
