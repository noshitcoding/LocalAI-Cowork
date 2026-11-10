//! Original-bound result delivery and separately evidenced resource retirement.
//! These portable records contain no native identities, paths or credentials.
//! A stored receipt is history, never a reconstructed native cleanup permit.
use super::*;
use crate::managed_transport::{identifier, identity, uuid, window};
use cowork_contracts::{ExecutionRunnerIdentity, ExecutionTargetRef};

pub const MANAGED_RESULT_DELIVERY_ACCEPT_MILLIS: i64 = 30_000;
pub const MANAGED_RESULT_RETIREMENT_QUERY_MILLIS: i64 = 30_000;
pub const MANAGED_RESULT_DELIVERY_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManagedResultDeliveryRequest {
    pub format_version: u32,
    pub request_id: String,
    /// Resolved by the controller; these two fields are audit bindings and do
    /// not replace its current actor/content and native Ready-custody checks.
    pub actor_id: String,
    pub authorization_sha256: String,
    pub runtime_instance_sha256: String,
    pub execution_plan_id: String,
    pub execution_plan_sha256: String,
    pub run_id: String,
    pub process_attempt_id: String,
    pub launch_envelope_sha256: String,
    pub execution_receipt_sha256: String,
    pub output_attestation_sha256: String,
    pub manifest_sha256: String,
    pub recipient_version_id: String,
    pub runner: ExecutionRunnerIdentity,
    pub target: ExecutionTargetRef,
    pub issued_at: i64,
    /// First acceptance only. Replaying an already accepted identical request
    /// returns its original receipt after this time, with no new native permit.
    pub accept_before: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManagedResultDeliveryReceipt {
    pub format_version: u32,
    pub request: Signed<ManagedResultDeliveryRequest>,
    pub accepted_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManagedResultRetirementQuery {
    pub format_version: u32,
    pub request_id: String,
    pub delivery_receipt_sha256: String,
    pub process_attempt_id: String,
    pub runner: ExecutionRunnerIdentity,
    pub target: ExecutionTargetRef,
    pub issued_at: i64,
    pub expires_at: i64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManagedResultRetirementState {
    Pending,
    Retired,
    Unconfirmed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManagedResultRetirementReceipt {
    pub format_version: u32,
    pub delivery_receipt_sha256: String,
    pub process_attempt_id: String,
    pub run_id: String,
    pub runtime_instance_sha256: String,
    pub revision: u64,
    pub state: ManagedResultRetirementState,
    pub recorded_at: i64,
    pub physical_completion_sha256: String,
    pub retirement_evidence_sha256: Option<String>,
}

impl ManagedResultDeliveryRequest {
    /// First admission only. The native caller must resolve an existing journal
    /// and its original live instance before classifying expiry as unaccepted.
    pub fn validate(
        &self,
        binding: &Binding,
        target: &ExecutionTargetRef,
        runtime_instance_sha256: &str,
        now: i64,
    ) -> Result<()> {
        identity(&self.runner, binding, target)?;
        if self.format_version != 1
            || !uuid(&self.request_id)
            || !uuid(&self.recipient_version_id)
            || !identifier(&self.actor_id)
            || !identifier(&self.execution_plan_id)
            || !identifier(&self.process_attempt_id)
            || !identifier(&self.run_id)
            || self.target != *target
            || !valid_sha256(runtime_instance_sha256)
            || self.runtime_instance_sha256 != runtime_instance_sha256
            || [
                &self.authorization_sha256,
                &self.execution_plan_sha256,
                &self.launch_envelope_sha256,
                &self.execution_receipt_sha256,
                &self.output_attestation_sha256,
                &self.manifest_sha256,
            ]
            .iter()
            .any(|value| !valid_sha256(value))
            || serde_json::to_vec(self)?.len() > MANAGED_RESULT_DELIVERY_BYTES
        {
            return Err(Error::Authority);
        }
        window(
            self.issued_at,
            self.accept_before,
            MANAGED_RESULT_DELIVERY_ACCEPT_MILLIS,
            now,
        )
    }
}

impl ManagedResultDeliveryReceipt {
    /// Historical validation uses the original acceptance time. It deliberately
    /// has no current-clock parameter and cannot admit cleanup or content reads.
    /// Callers independently verify the outer runner signature before this call.
    pub fn validate(
        &self,
        expected: &Signed<ManagedResultDeliveryRequest>,
        binding: &Binding,
    ) -> Result<()> {
        self.request.verify(
            "managed_result_delivery_request",
            &binding.control_public_key,
        )?;
        self.request.payload.validate(
            binding,
            &expected.payload.target,
            &expected.payload.runtime_instance_sha256,
            self.accepted_at,
        )?;
        if self.format_version != 1
            || self.request != *expected
            || serde_json::to_vec(self)?.len() > MANAGED_RESULT_DELIVERY_BYTES
        {
            return Err(Error::Authority);
        }
        Ok(())
    }
}

impl ManagedResultRetirementQuery {
    pub fn validate(
        &self,
        delivery: &Signed<ManagedResultDeliveryReceipt>,
        binding: &Binding,
        now: i64,
    ) -> Result<()> {
        delivery.verify(
            "managed_result_delivery_receipt",
            &binding.runner_public_key,
        )?;
        delivery
            .payload
            .validate(&delivery.payload.request, binding)?;
        let original = &delivery.payload.request.payload;
        identity(&self.runner, binding, &self.target)?;
        if self.format_version != 1
            || !uuid(&self.request_id)
            || self.delivery_receipt_sha256 != canonical_hash(delivery)
            || self.process_attempt_id != original.process_attempt_id
            || self.runner != original.runner
            || self.target != original.target
        {
            return Err(Error::Authority);
        }
        window(
            self.issued_at,
            self.expires_at,
            MANAGED_RESULT_RETIREMENT_QUERY_MILLIS,
            now,
        )
    }
}

impl ManagedResultRetirementReceipt {
    /// Validate portable history against the original output's physical-record
    /// digest. Neither this digest nor the receipt restores native file handles.
    /// Callers independently verify the outer runner signature before this call.
    pub fn validate(
        &self,
        delivery: &Signed<ManagedResultDeliveryReceipt>,
        binding: &Binding,
        physical_completion_sha256: &str,
    ) -> Result<()> {
        delivery.verify(
            "managed_result_delivery_receipt",
            &binding.runner_public_key,
        )?;
        delivery
            .payload
            .validate(&delivery.payload.request, binding)?;
        let original = &delivery.payload.request.payload;
        if self.format_version != 1
            || self.delivery_receipt_sha256 != canonical_hash(delivery)
            || self.process_attempt_id != original.process_attempt_id
            || self.run_id != original.run_id
            || self.runtime_instance_sha256 != original.runtime_instance_sha256
            || self.revision == 0
            || self.revision > i64::MAX as u64
            || self.recorded_at < delivery.payload.accepted_at
            || !valid_sha256(physical_completion_sha256)
            || self.physical_completion_sha256 != physical_completion_sha256
            || self
                .retirement_evidence_sha256
                .as_ref()
                .is_some_and(|value| !valid_sha256(value))
        {
            return Err(Error::Authority);
        }
        match self.state {
            ManagedResultRetirementState::Pending
                if self.revision == 1 && self.retirement_evidence_sha256.is_none() => {}
            ManagedResultRetirementState::Retired
                if self.revision > 1 && self.retirement_evidence_sha256.is_some() => {}
            ManagedResultRetirementState::Unconfirmed if self.revision > 1 => {}
            _ => return Err(Error::Authority),
        }
        Ok(())
    }
}
