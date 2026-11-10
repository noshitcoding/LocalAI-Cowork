//! Explicit delivery confirmation over a durable local Ready version. Delivery
//! and retirement history are separate from content/worker/launch authority.
use super::*;
use crate::ManagedResultDeliveryResponse;
use cowork_broker::OwnedVerifiedManagedResultStage;
use cowork_runner_protocol::{
    MANAGED_RESULT_RETIREMENT_QUERY_MILLIS, ManagedResultRetirementQuery,
};
use cowork_storage::{
    ManagedResultDelivery, ManagedResultDeliveryCommit, ManagedResultDeliveryState,
};

impl Runtime {
    async fn delivery_origin(&self) -> Result<String> {
        let files = self.files.clone().ok_or(Error::Forbidden)?;
        tokio::task::spawn_blocking(move || files.local_controller_origin())
            .await
            .map_err(storage_error)?
    }
    async fn fresh_delivery_ready(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
    ) -> Result<(
        LocalReadyAuthority,
        Arc<ProcessRunnerConfig>,
        OwnedVerifiedManagedResultStage,
    )> {
        let authority = self
            .local_ready_result_authority(order, run, attempt)
            .await?
            .ok_or(Error::NotFound)?;
        let runner = self.process_runner.clone().ok_or(Error::Forbidden)?;
        if runner.execution_identity() != authority.context.input.grant.payload.runner {
            return Err(Error::ExecutionPlanChanged);
        }
        // No remote read or output-retention renewal. We verify the actual
        // local pack and retain its real descriptor/custody through the commit.
        let stage = self.open_result_version_stage(&authority.version).await?;
        let verified = tokio::task::spawn_blocking(move || stage.verify_owned())
            .await
            .map_err(storage_error)??;
        self.recheck_local_ready_result_authority(&authority)
            .await?;
        Ok((authority, runner, verified))
    }

    /// Trusted Owner entry; the future renderer projection contains only the
    /// prepared identifier/hash and display summary, never this private record.
    pub async fn prepare_owner_managed_result_delivery(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
    ) -> Result<ManagedResultDelivery> {
        let _permit = self.begin_upload_transfer()?;
        let (authority, _, verified) = self.fresh_delivery_ready(order, run, attempt).await?;
        let delivery = self
            .store
            .prepare_managed_result_delivery(
                order,
                run,
                attempt,
                authority.guard.origin(),
                &authority.context.authorization_sha256,
                &verified,
            )
            .await?;
        self.recheck_local_ready_result_authority(&authority)
            .await?;
        Ok(delivery)
    }

    /// Commit exactly the displayed request before one transport call. Lost
    /// responses preserve Committed for explicit identical retry; no new nonce
    /// or timestamp is silently substituted.
    pub async fn confirm_owner_managed_result_delivery(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
        id: &str,
        expected_request_sha256: &str,
    ) -> Result<ManagedResultDelivery> {
        let _permit = self.begin_upload_transfer()?;
        let (authority, runner, verified) = self.fresh_delivery_ready(order, run, attempt).await?;
        let origin = authority.guard.origin().to_owned();
        let committed = self
            .store
            .commit_managed_result_delivery(ManagedResultDeliveryCommit {
                order,
                run,
                attempt,
                origin: &origin,
                id,
                expected_request_sha256,
                authorization: &authority.context.authorization_sha256,
                verified: &verified,
            })
            .await?;
        if let Err(error) = self.recheck_local_ready_result_authority(&authority).await {
            return Err(
                if committed.state == ManagedResultDeliveryState::Committed {
                    Error::UncertainEffect
                } else {
                    error
                },
            );
        }
        if committed.state != ManagedResultDeliveryState::Committed {
            return Ok(committed);
        }
        // Content/source locks do not span the remote cleanup. The verified
        // local pack capability remains alive; accepted history is saved even
        // if a source revocation or business cancellation races the response.
        drop(authority);
        let response = runner
            .deliver_result(committed.request.clone())
            .await
            .map_err(|_| Error::UncertainEffect)?;
        let value = match response {
            ManagedResultDeliveryResponse::Accepted(receipt) => {
                self.store
                    .retain_managed_result_delivery_receipt(
                        order,
                        run,
                        attempt,
                        &origin,
                        &committed.id,
                        &receipt,
                    )
                    .await
            }
            ManagedResultDeliveryResponse::ConfirmationExpired => {
                self.store
                    .expire_unaccepted_managed_result_delivery(
                        order,
                        run,
                        attempt,
                        &origin,
                        &committed.id,
                        expected_request_sha256,
                    )
                    .await
            }
        }
        .map_err(|_| Error::UncertainEffect)?;
        // Explicit lifetime: no retirement request outlives the verified local
        // Ready capability that justified the original delivery confirmation.
        drop(verified);
        Ok(value)
    }

    /// Read local original delivery history; neither a runner call nor a new
    /// confirmation is made, including after cancellation/source revocation.
    pub async fn owner_managed_result_delivery(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
    ) -> Result<Option<ManagedResultDelivery>> {
        let origin = self.delivery_origin().await?;
        self.store
            .managed_result_delivery(order, run, attempt, &origin)
            .await
    }

    /// Read an existing original ACK for a previously Committed request, or
    /// refresh signed retirement history of an already accepted delivery.
    /// Neither path can first accept a delivery, even after rights revocation.
    pub async fn refresh_owner_managed_result_retirement(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
    ) -> Result<ManagedResultDelivery> {
        let origin = self.delivery_origin().await?;
        let delivery = self
            .store
            .managed_result_delivery(order, run, attempt, &origin)
            .await?
            .ok_or(Error::NotFound)?;
        if delivery.state == ManagedResultDeliveryState::Retired {
            return Ok(delivery);
        }
        if delivery.accepted.is_none() && delivery.state != ManagedResultDeliveryState::Committed {
            return Ok(delivery);
        }
        let runner = self.process_runner.clone().ok_or(Error::Forbidden)?;
        if runner.execution_identity() != delivery.request.runner {
            return Err(Error::ExecutionPlanChanged);
        }
        let Some(accepted) = &delivery.accepted else {
            // Historical custody was checked by Store above. Only the read-only
            // endpoint may be used here: no Ready/content capability, no new ACK.
            let receipt = runner
                .result_delivery_history(delivery.request.clone())
                .await?;
            return self
                .store
                .retain_managed_result_delivery_receipt(
                    order,
                    run,
                    attempt,
                    &origin,
                    &delivery.id,
                    &receipt,
                )
                .await;
        };
        let issued_at = now();
        let query = ManagedResultRetirementQuery {
            format_version: 1,
            request_id: uuid::Uuid::new_v4().to_string(),
            delivery_receipt_sha256: canonical_hash(accepted),
            process_attempt_id: attempt.into(),
            runner: delivery.request.runner.clone(),
            target: delivery.request.target.clone(),
            issued_at,
            expires_at: issued_at
                .checked_add(MANAGED_RESULT_RETIREMENT_QUERY_MILLIS)
                .ok_or(Error::Forbidden)?,
        };
        let receipt = runner
            .result_retirement(query, accepted, &delivery.physical_completion_sha256)
            .await?;
        self.store
            .retain_managed_result_retirement_receipt(
                order,
                run,
                attempt,
                &origin,
                &delivery.id,
                &receipt,
            )
            .await
    }
}
