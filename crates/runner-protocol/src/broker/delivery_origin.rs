//! Read-only original provenance for a separately reserved native delivery slot.
//! This observation grants neither cleanup nor a new execution/content lease.
use super::*;

impl Broker {
    /// Check the original completed row and signed output before a native caller
    /// classifies a never-accepted request's live acceptance window. The caller
    /// must additionally hold the original published native objects and journal;
    /// this integer is only a durable clock floor, never a retirement capability.
    pub async fn validate_managed_result_delivery_origin(
        &self,
        request: &Signed<ManagedResultDeliveryRequest>,
        original: &Signed<Receipt>,
        output: &Signed<ManagedOutputAttestation>,
    ) -> Result<i64> {
        request.verify(
            "managed_result_delivery_request",
            &self.binding.control_public_key,
        )?;
        original.verify("receipt", &self.binding.runner_public_key)?;
        original.payload.validate(&self.binding)?;
        output.verify("managed_output", &self.binding.runner_public_key)?;
        let wanted = &request.payload;
        let provenance = &output.payload;
        if !matches!(original.payload.state, ExecutionState::Completed { .. })
            || wanted.runtime_instance_sha256 != self.instance_fingerprint()
            || provenance.runtime_instance_sha256 != wanted.runtime_instance_sha256
            || wanted.process_attempt_id != original.payload.request.process_attempt_id
            || wanted.run_id != original.payload.request.run_id
            || wanted.launch_envelope_sha256 != original.payload.request_envelope_sha256
            || wanted.execution_receipt_sha256 != canonical_hash(original)
            || wanted.output_attestation_sha256 != canonical_hash(output)
            || provenance.format_version != 1
            || provenance.execution_plan_id != wanted.execution_plan_id
            || provenance.execution_plan_sha256 != wanted.execution_plan_sha256
            || provenance.process_attempt_id != wanted.process_attempt_id
            || provenance.run_id != wanted.run_id
            || provenance.launch_envelope_sha256 != wanted.launch_envelope_sha256
            || provenance.execution_receipt_sha256 != wanted.execution_receipt_sha256
            || provenance.output_manifest_sha256 != wanted.manifest_sha256
            || provenance.runner != wanted.runner
            || provenance.target != wanted.target
            || provenance.produced_at != original.payload.updated_at
            || wanted.issued_at < provenance.produced_at
            || provenance.retained_until <= provenance.produced_at
            || provenance
                .retained_until
                .checked_sub(provenance.produced_at)
                .is_none_or(|duration| duration > MANAGED_RESULT_RETENTION_MILLIS)
            || !valid_sha256(&provenance.input_manifest_sha256)
            || !valid_sha256(&provenance.physical_record_sha256)
        {
            return Err(Error::Authority);
        }
        // Check its bounded shape at its declared issue time, never renew it.
        // Expiry is classified only later against live time by the native owner.
        // Malformed provenance must not become a "never accepted / expired" hint.
        wanted
            .validate(
                &self.binding,
                &provenance.target,
                &self.instance_fingerprint(),
                wanted.issued_at,
            )
            .map_err(|_| Error::Authority)?;
        let mut tx = self.pool.begin().await?;
        let binding = sqlx::query("SELECT payload,high_water FROM runner_binding WHERE id=1")
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(Error::Uncertain)?;
        let retained: String = binding.try_get("payload")?;
        let high_water: i64 = binding.try_get("high_water")?;
        if retained != serde_json::to_string(&self.binding)? || high_water < 0 {
            return Err(Error::Uncertain);
        }
        let row = sqlx::query("SELECT * FROM runner_executions WHERE attempt_id=$1")
            .bind(&wanted.process_attempt_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(Error::Uncertain)?;
        let receipt = self.checked(row)?;
        if receipt != *original
            || !matches!(receipt.payload.state, ExecutionState::Completed { .. })
        {
            return Err(Error::Authority);
        }
        tx.commit().await?;
        Ok(high_water)
    }
}
