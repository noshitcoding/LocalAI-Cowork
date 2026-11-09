//! Exact, bounded delivery/history transport on the existing private origin.
use super::*;
use cowork_runner_protocol::{
    MANAGED_RESULT_DELIVERY_BYTES, ManagedResultDeliveryReceipt, ManagedResultDeliveryRequest,
    ManagedResultRetirementQuery, ManagedResultRetirementReceipt,
};
use cowork_runtime::ManagedResultDeliveryResponse;

fn path(attempt: &str, suffix: &str) -> Result<String> {
    if attempt.is_empty()
        || attempt.len() > 128
        || matches!(attempt, "." | "..")
        || !attempt
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
    {
        return Err(Error::Forbidden);
    }
    Ok(format!("v2/managed/results/{attempt}/{suffix}"))
}

async fn bounded<T: DeserializeOwned>(mut response: reqwest::Response) -> Result<T> {
    if response
        .content_length()
        .is_some_and(|n| n > MANAGED_RESULT_DELIVERY_BYTES as u64)
    {
        return Err(Error::UncertainEffect);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| Error::UncertainEffect)? {
        if bytes.len().saturating_add(chunk.len()) > MANAGED_RESULT_DELIVERY_BYTES {
            return Err(Error::UncertainEffect);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| Error::UncertainEffect)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeRejection {
    error: String,
}

impl HttpsRunner {
    pub(in crate::process_runner_config) async fn read_managed_delivery_history(
        &self,
        request: Signed<ManagedResultDeliveryRequest>,
    ) -> Result<Signed<ManagedResultDeliveryReceipt>> {
        request
            .verify(
                "managed_result_delivery_request",
                &self.binding.control_public_key,
            )
            .map_err(|_| Error::Forbidden)?;
        request
            .payload
            .validate(
                &self.binding,
                &request.payload.target,
                &request.payload.runtime_instance_sha256,
                request.payload.issued_at,
            )
            .map_err(|_| Error::Forbidden)?;
        // This endpoint can only look up an existing original ACK. Never fall
        // back to /delivery when history is absent, unsupported or uncertain.
        let response = self
            .managed_post(
                &path(&request.payload.process_attempt_id, "delivery/history")?,
                &request,
                MANAGED_RESULT_DELIVERY_BYTES,
            )
            .await?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(Error::UncertainEffect);
        }
        let receipt: Signed<ManagedResultDeliveryReceipt> = bounded(response).await?;
        receipt
            .verify(
                "managed_result_delivery_receipt",
                &self.binding.runner_public_key,
            )
            .map_err(|_| Error::UncertainEffect)?;
        receipt
            .payload
            .validate(&request, &self.binding)
            .map_err(|_| Error::UncertainEffect)?;
        Ok(receipt)
    }
    pub(in crate::process_runner_config) async fn deliver_managed_result(
        &self,
        request: Signed<ManagedResultDeliveryRequest>,
    ) -> Result<ManagedResultDeliveryResponse> {
        request
            .verify(
                "managed_result_delivery_request",
                &self.binding.control_public_key,
            )
            .map_err(|_| Error::Forbidden)?;
        // Historical shape check only: an unchanged accepted request remains
        // resolvable after expiry. No new window, retry or fallback is minted.
        request
            .payload
            .validate(
                &self.binding,
                &request.payload.target,
                &request.payload.runtime_instance_sha256,
                request.payload.issued_at,
            )
            .map_err(|_| Error::Forbidden)?;
        let response = self
            .managed_post(
                &path(&request.payload.process_attempt_id, "delivery")?,
                &request,
                MANAGED_RESULT_DELIVERY_BYTES,
            )
            .await?;
        match response.status() {
            reqwest::StatusCode::OK => {
                let receipt: Signed<ManagedResultDeliveryReceipt> = bounded(response).await?;
                receipt
                    .verify(
                        "managed_result_delivery_receipt",
                        &self.binding.runner_public_key,
                    )
                    .map_err(|_| Error::UncertainEffect)?;
                receipt
                    .payload
                    .validate(&request, &self.binding)
                    .map_err(|_| Error::UncertainEffect)?;
                Ok(ManagedResultDeliveryResponse::Accepted(Box::new(receipt)))
            }
            reqwest::StatusCode::CONFLICT => {
                let rejection: NativeRejection = bounded(response).await?;
                if rejection.error == "managed_result_delivery_confirmation_expired" {
                    Ok(ManagedResultDeliveryResponse::ConfirmationExpired)
                } else {
                    Err(Error::UncertainEffect)
                }
            }
            _ => Err(Error::UncertainEffect),
        }
    }
    pub(in crate::process_runner_config) async fn observe_managed_retirement(
        &self,
        request: Signed<ManagedResultRetirementQuery>,
    ) -> Result<Signed<ManagedResultRetirementReceipt>> {
        request
            .verify(
                "managed_result_retirement_query",
                &self.binding.control_public_key,
            )
            .map_err(|_| Error::Forbidden)?;
        let response = self
            .managed_post(
                &path(&request.payload.process_attempt_id, "retirement")?,
                &request,
                MANAGED_RESULT_DELIVERY_BYTES,
            )
            .await?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(Error::UncertainEffect);
        }
        let receipt: Signed<ManagedResultRetirementReceipt> = bounded(response).await?;
        receipt
            .verify(
                "managed_result_retirement_receipt",
                &self.binding.runner_public_key,
            )
            .map_err(|_| Error::Forbidden)?;
        if receipt.payload.process_attempt_id != request.payload.process_attempt_id
            || receipt.payload.delivery_receipt_sha256 != request.payload.delivery_receipt_sha256
        {
            return Err(Error::Forbidden);
        }
        // The Runtime additionally checks the original accepted receipt and
        // physical completion before persisting this strictly separate history.
        Ok(receipt)
    }
}
