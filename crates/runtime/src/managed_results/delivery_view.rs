//! Owner display projection; portable hashes do not grant content or cleanup rights.
use super::*;

fn delivery_view(
    order: &str,
    run: &str,
    attempt: &str,
    value: Option<cowork_storage::ManagedResultDelivery>,
) -> OwnerManagedResultDeliveryView {
    use cowork_storage::ManagedResultDeliveryState as Private;
    OwnerManagedResultDeliveryView {
        order_id: order.into(),
        run_id: run.into(),
        process_attempt_id: attempt.into(),
        delivery: value.map(|value| OwnerManagedResultDelivery {
            id: value.id,
            result_version_id: value.result_version_id,
            request_sha256: canonical_hash(&value.request),
            manifest_sha256: value.request.manifest_sha256,
            state: match value.state {
                Private::Prepared => OwnerManagedResultDeliveryState::Prepared,
                Private::Committed => OwnerManagedResultDeliveryState::Committed,
                Private::Expired => OwnerManagedResultDeliveryState::Expired,
                Private::Accepted => OwnerManagedResultDeliveryState::Accepted,
                Private::Retired => OwnerManagedResultDeliveryState::Retired,
                Private::Unconfirmed => OwnerManagedResultDeliveryState::Unconfirmed,
            },
            created_at: value.created_at,
            accept_before: value.request.accept_before,
            revision: value.revision,
            accepted_at: value
                .accepted
                .as_ref()
                .map(|receipt| receipt.payload.accepted_at),
            delivery_receipt_sha256: value.accepted.as_ref().map(canonical_hash),
            retirement_revision: value
                .retirement
                .as_ref()
                .map(|receipt| receipt.payload.revision),
            retirement_recorded_at: value
                .retirement
                .as_ref()
                .map(|receipt| receipt.payload.recorded_at),
            retirement_receipt_sha256: value.retirement.as_ref().map(canonical_hash),
        }),
    }
}

impl Runtime {
    pub async fn owner_managed_result_delivery_view(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
    ) -> Result<OwnerManagedResultDeliveryView> {
        let value = self
            .owner_managed_result_delivery(order, run, attempt)
            .await?;
        Ok(delivery_view(order, run, attempt, value))
    }
    pub async fn prepare_owner_managed_result_delivery_view(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
    ) -> Result<OwnerManagedResultDeliveryView> {
        let value = self
            .prepare_owner_managed_result_delivery(order, run, attempt)
            .await?;
        Ok(delivery_view(order, run, attempt, Some(value)))
    }
    pub async fn confirm_owner_managed_result_delivery_view(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
        input: &ConfirmOwnerManagedResultDelivery,
    ) -> Result<OwnerManagedResultDeliveryView> {
        let value = self
            .confirm_owner_managed_result_delivery(
                order,
                run,
                attempt,
                &input.id,
                &input.request_sha256,
            )
            .await?;
        Ok(delivery_view(order, run, attempt, Some(value)))
    }
    pub async fn refresh_owner_managed_result_retirement_view(
        &self,
        order: &str,
        run: &str,
        attempt: &str,
    ) -> Result<OwnerManagedResultDeliveryView> {
        let value = self
            .refresh_owner_managed_result_retirement(order, run, attempt)
            .await?;
        Ok(delivery_view(order, run, attempt, Some(value)))
    }
}
