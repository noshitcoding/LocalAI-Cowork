//! Observed original signed completion, never a content-read or dispatch grant.
//! History may survive cancellation/revocation/worker expiry. Future result reads
//! must resolve their own current actor/content authority separately.
use super::*;
use cowork_runner_protocol::{ExecutionState, Receipt, Signed, envelope_hash};
use cowork_workflow_contracts::ExecutionDestinationV2;
use sqlx::{Any, Transaction};

/// Original signed input/plan/attempt provenance, checked at issuance time.
/// It deliberately neither resolves a current target nor reads source content.
pub(super) async fn original_input(
    tx: &mut Transaction<'_, Any>,
    run: &str,
    attempt_id: &str,
) -> Result<(
    ManagedInputTransfer,
    StoredExecutionPlanV2,
    cowork_domain::processes::StoredProcess,
)> {
    let row = sqlx::query(
        "SELECT * FROM owner_managed_input_transfers WHERE attempt_id=$1 AND run_id=$2",
    )
    .bind(attempt_id)
    .bind(run)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?
    .ok_or(Error::NotFound)?;
    let input = managed_inputs::checked(&row)?;
    let r = &input.request.payload;
    let row = sqlx::query("SELECT * FROM owner_execution_plans_v2 WHERE id=$1 AND run_id=$2")
        .bind(&input.grant.payload.execution_plan_id)
        .bind(&r.run_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::ExecutionPlanChanged)?;
    let plan = execution_plans_v2::checked(&row)?;
    let accepted: RunRecord = parse(
        &row.try_get::<Option<String>, _>("receipt")
            .map_err(storage_error)?
            .ok_or(Error::ExecutionPlanChanged)?,
    )?;
    let ExecutionDestinationV2::ManagedRunner { runner, target, .. } = &plan.plan.target else {
        return Err(Error::Forbidden);
    };
    input
        .request
        .verify("launch", &runner.control_public_key)
        .map_err(|_| Error::Forbidden)?;
    input
        .grant
        .verify("managed_input_grant", &runner.control_public_key)
        .map_err(|_| Error::Forbidden)?;
    // Historical shape/signature validation at the original issuance window.
    // This explicitly does not make an expired input/worker lease fresh again.
    input
        .grant
        .payload
        .validate_for_launch(
            &managed_inputs::binding(runner),
            target,
            &input.request.payload,
            input
                .request
                .payload
                .issued_at
                .max(input.grant.payload.issued_at),
        )
        .map_err(|_| Error::Forbidden)?;
    cowork_domain::workspace_transfer::validate(&input.manifest)?;
    verify_workspace_snapshot(&input.snapshot)?;
    let row = sqlx::query(
        "SELECT id,tool_call_id,revision,payload FROM process_attempts WHERE id=$1 AND run_id=$2",
    )
    .bind(&r.process_attempt_id)
    .bind(&r.run_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?
    .ok_or(Error::NotFound)?;
    let attempt = processes::checked_record(&row, &r.run_id)?;
    if r.run_id != run
        || r.process_attempt_id != attempt_id
        || input.grant.payload.execution_plan_sha256 != plan.sha256
        || input.grant.payload.input_manifest_sha256 != input.manifest.sha256
        || input.grant.payload.expires_at > input.request.payload.expires_at
        || accepted.id != r.run_id
        || accepted.spec.order_id != plan.plan.order_id
        || accepted.spec.project_id != plan.plan.project_id
        || canonical_hash(&accepted.spec) != plan.plan.spec_sha256
        || input.snapshot.run_id != r.run_id
        || input.snapshot.order_id != plan.plan.order_id
        || attempt.record.project_id != plan.plan.project_id
        || attempt.record.id != r.process_attempt_id
        || attempt.record.tool.id != r.tool_call_id
        || attempt.record.request_sha256 != r.request_sha256
        || cowork_domain::processes::process_request_hash(&accepted, &attempt.record.tool)
            != r.request_sha256
        || attempt.record.environment != r.environment
        || attempt.record.workspace_resource_id != r.workspace_resource_id
        || cowork_domain::processes::process_invocation(&attempt.record.tool)? != r.invocation
        || attempt.lease_sha256 != r.worker_fence_sha256
    {
        return Err(Error::ExecutionPlanChanged);
    }
    let directories: Vec<_> = input
        .snapshot
        .entries
        .iter()
        .filter(|e| e.sha256.is_none())
        .map(|e| e.path.clone())
        .collect();
    let files: Vec<_> = input
        .snapshot
        .entries
        .iter()
        .filter(|e| e.sha256.is_some())
        .collect();
    if input.snapshot.entries.iter().any(|entry| {
        !plan
            .workspace
            .copies
            .iter()
            .any(|copy| entry.path.split('/').next() == Some(copy.alias.as_str()))
    }) || directories != input.manifest.directories
        || files.len() != input.manifest.files.len()
        || files
            .iter()
            .zip(&input.manifest.files)
            .any(|(entry, file)| {
                entry.path != file.path
                    || entry.size != file.size
                    || entry.sha256.as_ref() != Some(&file.sha256)
            })
    {
        return Err(Error::ExecutionPlanChanged);
    }
    Ok((input, plan, attempt))
}

async fn validate(
    tx: &mut Transaction<'_, Any>,
    receipt: &Signed<Receipt>,
) -> Result<ManagedInputTransfer> {
    let r = &receipt.payload.request;
    let (input, plan, attempt) = original_input(tx, &r.run_id, &r.process_attempt_id).await?;
    let ExecutionDestinationV2::ManagedRunner { runner, .. } = &plan.plan.target else {
        return Err(Error::Forbidden);
    };
    receipt
        .verify("receipt", &runner.runner_public_key)
        .map_err(|_| Error::Forbidden)?;
    let ExecutionState::Completed { result } = &receipt.payload.state else {
        return Err(Error::Forbidden);
    };
    cowork_domain::processes::validate_process_result(result)?;
    if receipt.payload.request != input.request.payload
        || receipt.payload.request_envelope_sha256 != envelope_hash(&input.request)
        || receipt.payload.revision != 3
        || receipt.payload.updated_at < r.issued_at
    {
        return Err(Error::ExecutionPlanChanged);
    }
    match &attempt.record.state {
        ProcessAttemptState::LaunchCommitted => {}
        ProcessAttemptState::Completed { result: old }
        | ProcessAttemptState::ReconciledCompleted { result: old, .. }
            if old == result => {}
        _ => return Err(Error::Conflict),
    }
    Ok(input)
}

pub(super) async fn retain(
    tx: &mut Transaction<'_, Any>,
    receipt: &Signed<Receipt>,
) -> Result<Signed<Receipt>> {
    let input = validate(tx, receipt).await?;
    sqlx::query("INSERT INTO owner_managed_completion_receipts(attempt_id,run_id,plan_id,receipt_sha256,payload) VALUES($1,$2,$3,$4,$5) ON CONFLICT(attempt_id) DO NOTHING")
        .bind(&receipt.payload.request.process_attempt_id).bind(&receipt.payload.request.run_id).bind(&input.grant.payload.execution_plan_id)
        .bind(canonical_hash(receipt)).bind(json_string(receipt)?).execute(&mut **tx).await.map_err(storage_error)?;
    let row = sqlx::query("SELECT * FROM owner_managed_completion_receipts WHERE attempt_id=$1")
        .bind(&receipt.payload.request.process_attempt_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(storage_error)?;
    let original = checked(tx, &row).await?;
    if original != *receipt {
        return Err(Error::Conflict);
    }
    Ok(original)
}

pub(super) async fn checked(
    tx: &mut Transaction<'_, Any>,
    row: &sqlx::any::AnyRow,
) -> Result<Signed<Receipt>> {
    let receipt: Signed<Receipt> =
        parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if canonical_hash(&receipt)
        != row
            .try_get::<String, _>("receipt_sha256")
            .map_err(storage_error)?
        || receipt.payload.request.process_attempt_id
            != row
                .try_get::<String, _>("attempt_id")
                .map_err(storage_error)?
        || receipt.payload.request.run_id
            != row.try_get::<String, _>("run_id").map_err(storage_error)?
    {
        return Err(Error::ExecutionPlanChanged);
    }
    let input = validate(tx, &receipt).await?;
    if input.grant.payload.execution_plan_id
        != row.try_get::<String, _>("plan_id").map_err(storage_error)?
    {
        return Err(Error::ExecutionPlanChanged);
    }
    Ok(receipt)
}

impl Store {
    /// Preserve authenticated execution history for one existing attempt. This
    /// deliberately performs no run/worker transition or file authorization.
    pub async fn retain_managed_completion_receipt(
        &self,
        receipt: &Signed<Receipt>,
    ) -> Result<Signed<Receipt>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let original = retain(&mut tx, receipt).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(original)
    }

    /// Informational signed history only. This is not result-file read access.
    pub async fn managed_completion_receipt(
        &self,
        run: &str,
        attempt: &str,
    ) -> Result<Option<Signed<Receipt>>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let row = sqlx::query(
            "SELECT * FROM owner_managed_completion_receipts WHERE attempt_id=$1 AND run_id=$2",
        )
        .bind(attempt)
        .bind(run)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?;
        let result = match row {
            Some(row) => Some(checked(&mut tx, &row).await?),
            None => None,
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
}
