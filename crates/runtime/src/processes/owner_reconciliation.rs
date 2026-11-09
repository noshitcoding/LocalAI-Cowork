//! Owner recovery observes one original execution; it never launches or grants
//! access to source/output files. The Owner submits intent, not process evidence.
use super::*;

impl Runtime {
    async fn managed_process_history_origin(&self) -> Result<String> {
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        tokio::task::spawn_blocking(move || files.local_controller_origin())
            .await
            .map_err(storage_error)?
    }

    pub async fn reconcile_owner_managed_process(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
        intent: ReconcileOwnerManagedProcess,
    ) -> Result<OwnerManagedProcessReconciliation> {
        // Keep the composed SQL/history/transport future off the request stack.
        Box::pin(self.reconcile_owner_managed_process_inner(order, run, attempt, intent)).await
    }

    async fn reconcile_owner_managed_process_inner(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
        intent: ReconcileOwnerManagedProcess,
    ) -> Result<OwnerManagedProcessReconciliation> {
        if intent.expected_run_revision <= 0
            || !cowork_domain::processes::valid_sha256(&intent.expected_request_sha256)
        {
            return Err(Error::Validation(
                "Invalid managed process reconciliation intent".into(),
            ));
        }
        let origin = self.managed_process_history_origin().await?;
        let history = self
            .store
            .owner_managed_process_history_context(order, run, attempt, &origin)
            .await?;
        if history.process_attempt.request_sha256 != intent.expected_request_sha256 {
            return Err(Error::Conflict);
        }
        // Only the Store's exact original journaled success can replay. It needs
        // neither a configured/online runner nor a second network observation.
        if history.process_attempt.state != ProcessAttemptState::LaunchCommitted {
            let receipt = history.receipt.as_ref().ok_or(Error::Conflict)?;
            return self
                .store
                .reconcile_owner_managed_process(order, run, attempt, &origin, &intent, receipt)
                .await;
        }
        // Reject active/stale work before asking the transport for any receipt.
        // The same conditions are checked again by the atomic Store mutation.
        let stored = self.store.stored_run(run).await?;
        if canonical_hash(&stored.record) != canonical_hash(&history.run)
            || stored.record.revision != intent.expected_run_revision
            || stored.record.state != RunState::Interrupted
            || stored.record.spec.workflow.is_some()
            || stored.lease.is_some()
            || stored.checkpoint.in_flight.as_ref() != Some(&history.process_attempt.tool)
            || stored.checkpoint.pending_tools.first() != Some(&history.process_attempt.tool)
        {
            return Err(Error::Conflict);
        }
        let receipt = match history.receipt {
            Some(receipt) => receipt,
            None => {
                let runner = self.process_runner.as_ref().ok_or(Error::Forbidden)?;
                // Current target activation/revision is irrelevant to history.
                // The original target remains verified by Store provenance;
                // transport must retain the exact original cryptographic identity.
                if runner.execution_identity() != history.input.grant.payload.runner {
                    return Err(Error::ExecutionPlanChanged);
                }
                // GET of the exact original attempt only: no launch, readiness,
                // lease renewal, environment activation or content read.
                let receipt = runner.access.receipt(attempt).await?;
                receipt
                    .verify(
                        "receipt",
                        &history.input.grant.payload.runner.runner_public_key,
                    )
                    .map_err(|_| Error::Forbidden)?;
                if receipt.payload.request != history.input.request.payload
                    || receipt.payload.request_envelope_sha256
                        != cowork_runner_protocol::envelope_hash(&history.input.request)
                    || receipt.payload.revision != 3
                    || receipt.payload.updated_at < history.input.request.payload.issued_at
                {
                    return Err(Error::UncertainEffect);
                }
                let ExecutionState::Completed { result } = &receipt.payload.state else {
                    return Err(Error::UncertainEffect);
                };
                cowork_domain::processes::validate_process_result(result)?;
                receipt
            }
        };
        if self.managed_process_history_origin().await? != origin {
            return Err(Error::ExecutionPlanChanged);
        }
        self.store
            .reconcile_owner_managed_process(order, run, attempt, &origin, &intent, &receipt)
            .await
    }
}
