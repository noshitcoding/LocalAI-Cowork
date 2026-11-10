//! Owner recovery of one original managed process, not a new execution or file
//! grant. The signed receipt is supplied by the trusted runtime, never a DTO.
use super::*;
use cowork_domain::processes::{StoredProcess, process_result_summary};
use cowork_runner_protocol::{ExecutionState, Receipt, Signed};
use sqlx::{Any, Transaction};

pub struct ManagedProcessHistoryContext {
    pub plan: StoredExecutionPlanV2,
    pub input: ManagedInputTransfer,
    pub run: RunRecord,
    pub process_attempt: ProcessAttempt,
    pub receipt: Option<Signed<Receipt>>,
}

#[derive(Clone)]
pub(super) struct OwnerReconciliationProof {
    order: String,
    native_origin: String,
    intent: ReconcileOwnerManagedProcess,
    receipt: Signed<Receipt>,
}

// The global accounting writer is acquired by the caller before run/order/
// controller locks, as in mutate. Re-check original SQL provenance, not current
// source files, environment availability, target activation or input TTL.
async fn context(
    tx: &mut Transaction<'_, Any>,
    order: &str,
    run: &str,
    attempt: &str,
    native_origin: &str,
) -> Result<(ManagedProcessHistoryContext, StoredRun, StoredProcess)> {
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
    let row = sqlx::query("SELECT revision,payload FROM runs WHERE id=$1 AND order_id=$2")
        .bind(run)
        .bind(order)
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::NotFound)?;
    let stored: StoredRun = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if stored.record.id != run
        || stored.record.revision != row.try_get::<i64, _>("revision").map_err(storage_error)?
        || stored.record.spec.order_id != order
        || stored.record.spec.workflow.is_some()
    {
        return Err(Error::Forbidden);
    }
    if sqlx::query("UPDATE orders SET revision=revision WHERE id=$1 AND project_id=$2")
        .bind(order)
        .bind(&stored.record.spec.project_id)
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?
        .rows_affected()
        != 1
    {
        return Err(Error::NotFound);
    }
    let payload: String =
        sqlx::query_scalar("SELECT payload FROM orders WHERE id=$1 AND project_id=$2")
            .bind(order)
            .bind(&stored.record.spec.project_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage_error)?;
    let owner: Order = parse(&payload)?;
    if owner.id != order || owner.project_id != stored.record.spec.project_id {
        return Err(Error::Forbidden);
    }
    let (input, plan, process) =
        Box::pin(managed_completions::original_input(tx, run, attempt)).await?;
    if plan.plan.order_id != order
        || plan.plan.project_id != owner.project_id
        || canonical_hash(&stored.record.spec) != plan.plan.spec_sha256
        || execution_controllers::locked(tx, native_origin).await? != *plan.plan.target.controller()
    {
        return Err(Error::ExecutionPlanChanged);
    }
    let row = sqlx::query("SELECT payload,custody_sha256,workspace_id FROM owner_run_workspaces WHERE run_id=$1 AND plan_id=$2")
        .bind(run).bind(&plan.plan.id).fetch_optional(&mut **tx).await.map_err(storage_error)?
        .ok_or(Error::ExecutionPlanChanged)?;
    let workspace: Value = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if canonical_hash(&workspace)
        != row
            .try_get::<String, _>("custody_sha256")
            .map_err(storage_error)?
        || workspace["intent"] != serde_json::to_value(&plan.workspace).map_err(storage_error)?
        || row
            .try_get::<String, _>("workspace_id")
            .map_err(storage_error)?
            != plan.workspace.binding.id
    {
        return Err(Error::ExecutionPlanChanged);
    }
    let row = sqlx::query(
        "SELECT * FROM owner_managed_completion_receipts WHERE attempt_id=$1 AND run_id=$2",
    )
    .bind(attempt)
    .bind(run)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage_error)?;
    let receipt = match row {
        Some(row) => Some(Box::pin(managed_completions::checked(tx, &row)).await?),
        None => None,
    };
    Ok((
        ManagedProcessHistoryContext {
            plan,
            input,
            run: stored.record.clone(),
            process_attempt: process.record.clone(),
            receipt,
        },
        stored,
        process,
    ))
}

/// Read original owner/process custody for delivery receipt history only. The
/// caller acquires the existing creation lock first. No current source read,
/// target activation, worker transition or retirement capability is issued.
pub(super) async fn delivery_history(
    tx: &mut Transaction<'_, Any>,
    order: &str,
    run: &str,
    attempt: &str,
    native_origin: &str,
) -> Result<ManagedProcessHistoryContext> {
    let (history, _, _) = context(tx, order, run, attempt, native_origin).await?;
    Ok(history)
}

fn event_payload(process: &ProcessAttempt, proof: &OwnerReconciliationProof) -> Value {
    json!({"process_attempt_id":process.id,"tool_call_id":process.tool.id,
        "request_sha256":process.request_sha256,"receipt_sha256":canonical_hash(&proof.receipt),
        "owner_intent":proof.intent,"automatic_retry":false})
}

fn result_state(receipt: &Signed<Receipt>) -> Result<ProcessAttemptState> {
    let ExecutionState::Completed { result } = &receipt.payload.state else {
        return Err(Error::Forbidden);
    };
    cowork_domain::processes::validate_process_result(result)?;
    Ok(ProcessAttemptState::ReconciledCompleted {
        result: result.clone(),
        evidence_sha256: canonical_hash(receipt),
    })
}

/// A replay is the exact original successful command at its unchanged result
/// revision, authenticated by the original signed receipt and event journal.
/// A later resume/cancel/run mutation makes an old expected revision stale.
async fn replay(
    tx: &mut Transaction<'_, Any>,
    history: &ManagedProcessHistoryContext,
    stored: &StoredRun,
    proof: &OwnerReconciliationProof,
) -> Result<bool> {
    if history.process_attempt.state == ProcessAttemptState::LaunchCommitted {
        return Ok(false);
    }
    if history.process_attempt.state != result_state(&proof.receipt)?
        || history.receipt.as_ref() != Some(&proof.receipt)
        || stored.record.revision
            != proof
                .intent
                .expected_run_revision
                .checked_add(1)
                .ok_or(Error::Conflict)?
        || stored.lease.is_some()
        || stored.checkpoint.in_flight.is_some()
        || stored.record.state
            != if proof.intent.finish_cancellation {
                RunState::Canceled
            } else {
                RunState::Interrupted
            }
    {
        return Err(Error::Conflict);
    }
    let sequence = stored.record.last_sequence - i64::from(proof.intent.finish_cancellation);
    let row = sqlx::query("SELECT id,sequence,payload FROM events WHERE run_id=$1 AND sequence=$2")
        .bind(&stored.record.id)
        .bind(sequence)
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage_error)?
        .ok_or(Error::Conflict)?;
    let event: RunEvent = parse(&row.try_get::<String, _>("payload").map_err(storage_error)?)?;
    if event.id != row.try_get::<String, _>("id").map_err(storage_error)?
        || event.sequence != row.try_get::<i64, _>("sequence").map_err(storage_error)?
        || event.run_id != stored.record.id
        || event.kind != "owner_managed_process_reconciled"
        || event.payload != event_payload(&history.process_attempt, proof)
    {
        return Err(Error::Conflict);
    }
    Ok(true)
}

/// Called by write_attempt inside the existing run/checkpoint/event commit.
/// Every original signature, SQL binding and local origin is checked again;
/// the original completion is retained atomically with the process settlement.
pub(super) async fn verify_write(
    tx: &mut Transaction<'_, Any>,
    run: &StoredRun,
    write: &ProcessWrite,
    proof: &OwnerReconciliationProof,
) -> Result<()> {
    let process = &write.stored.record;
    let (history, current, original) = Box::pin(context(
        tx,
        &proof.order,
        &run.record.id,
        &process.id,
        &proof.native_origin,
    ))
    .await?;
    if write.expected_revision != Some(1)
        || original.record.state != ProcessAttemptState::LaunchCommitted
        || canonical_hash(&current) != canonical_hash(run)
        || run.record.revision
            != proof
                .intent
                .expected_run_revision
                .checked_add(1)
                .ok_or(Error::Conflict)?
        || run.record.state
            != if proof.intent.finish_cancellation {
                RunState::Canceled
            } else {
                RunState::Interrupted
            }
        || run.lease.is_some()
        || run.checkpoint.in_flight.is_some()
        || process.request_sha256 != proof.intent.expected_request_sha256
        || process.state != result_state(&proof.receipt)?
        || history.input.request.payload != proof.receipt.payload.request
        || history
            .receipt
            .as_ref()
            .is_some_and(|receipt| receipt != &proof.receipt)
    {
        return Err(Error::Conflict);
    }
    Box::pin(managed_completions::retain(tx, &proof.receipt)).await?;
    Ok(())
}

impl Store {
    /// Private controller history. This does not authorize file reads or dispatch.
    pub async fn owner_managed_process_history_context(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
        native_origin: &str,
    ) -> Result<ManagedProcessHistoryContext> {
        Box::pin(async {
            let mut tx = self.pool.begin().await.map_err(storage_error)?;
            let _creation = model_money::lock_creation(&mut tx).await?;
            let (history, _, _) = context(&mut tx, order, run, attempt, native_origin).await?;
            tx.commit().await.map_err(storage_error)?;
            Ok(history)
        })
        .await
    }

    /// Trusted runtime only: receipt comes from original retained history or an
    /// authenticated observation of the original launch, never from the Owner DTO.
    pub async fn reconcile_owner_managed_process(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
        native_origin: &str,
        intent: &ReconcileOwnerManagedProcess,
        receipt: &Signed<Receipt>,
    ) -> Result<OwnerManagedProcessReconciliation> {
        Box::pin(async {
            if intent.expected_run_revision <= 0
                || !cowork_domain::processes::valid_sha256(&intent.expected_request_sha256)
            {
                return Err(Error::Validation(
                    "Invalid managed process reconciliation intent".into(),
                ));
            }
            let proof = OwnerReconciliationProof {
                order: order.into(),
                native_origin: native_origin.into(),
                intent: intent.clone(),
                receipt: receipt.clone(),
            };
            let mut tx = self.pool.begin().await.map_err(storage_error)?;
            let _creation = model_money::lock_creation(&mut tx).await?;
            let (history, stored, mut process) =
                context(&mut tx, order, run, attempt, native_origin).await?;
            if history.process_attempt.request_sha256 != intent.expected_request_sha256
                || history.input.request.payload != receipt.payload.request
            {
                return Err(Error::Conflict);
            }
            if replay(&mut tx, &history, &stored, &proof).await? {
                tx.commit().await.map_err(storage_error)?;
                return Ok(OwnerManagedProcessReconciliation {
                    run: history.run,
                    process_attempt: history.process_attempt,
                });
            }
            if stored.record.revision != intent.expected_run_revision
                || stored.record.state != RunState::Interrupted
                || stored.lease.is_some()
            {
                return Err(Error::Conflict);
            }
            processes::in_flight(&stored, &process.record)?;
            // No journal mutation here: the proof is validated and retained only
            // inside the subsequent run mutation's single commit transaction.
            tx.commit().await.map_err(storage_error)?;
            process.record.state = result_state(receipt)?;
            process.record.revision = 2;
            process.record.updated_at = now().max(process.record.updated_at);
            process.validate()?;
            let commit = self
                .mutate(run, None, |r| {
                    if r.record.revision != intent.expected_run_revision
                        || r.record.state != RunState::Interrupted
                        || r.lease.is_some()
                        || r.record.spec.workflow.is_some()
                        || r.record.spec.order_id != order
                    {
                        return Err(Error::Conflict);
                    }
                    processes::in_flight(r, &process.record)?;
                    let ExecutionState::Completed { result } = &proof.receipt.payload.state else {
                        return Err(Error::Forbidden);
                    };
                    processes::settle_checkpoint(
                        r,
                        &process.record.tool,
                        process_result_summary(result),
                    );
                    r.record.error_code = Some("executor_interrupted".into());
                    let mut change = Change::event(
                        "owner_managed_process_reconciled",
                        event_payload(&process.record, &proof),
                    );
                    if intent.finish_cancellation {
                        r.transition(RunState::Canceled)?;
                        r.lease = None;
                        change
                            .events
                            .push(("run_canceled".into(), json!({"state":"canceled"})));
                    }
                    change.process_write = Some(ProcessWrite {
                        stored: process.clone(),
                        expected_revision: Some(1),
                        owner_reconciliation: Some(proof.clone()),
                    });
                    Ok(change)
                })
                .await;
            // A lost commit acknowledgement is observation only. Recheck the
            // exact original success; never settle or contact a runner twice.
            match commit {
                Ok(saved) => Ok(OwnerManagedProcessReconciliation {
                    run: saved.record,
                    process_attempt: process.record,
                }),
                Err(error) => {
                    let mut tx = self.pool.begin().await.map_err(storage_error)?;
                    let _creation = model_money::lock_creation(&mut tx).await?;
                    let (history, saved, _) =
                        context(&mut tx, order, run, attempt, native_origin).await?;
                    if replay(&mut tx, &history, &saved, &proof)
                        .await
                        .unwrap_or(false)
                    {
                        tx.commit().await.map_err(storage_error)?;
                        Ok(OwnerManagedProcessReconciliation {
                            run: history.run,
                            process_attempt: history.process_attempt,
                        })
                    } else {
                        Err(error)
                    }
                }
            }
        })
        .await
    }
}
