//! Short-lived observations of an original completed result's current fence.
//! Neither the observation nor its digest grants execution or content rights.
use super::*;
use crate::managed_transport::{identifier, identity, uuid, window};
use cowork_contracts::{ExecutionRunnerIdentity, ExecutionTargetRef};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManagedResultContextRequest {
    pub format_version: u32,
    pub request_id: String,
    pub runtime_instance_sha256: String,
    pub process_attempt_id: String,
    pub run_id: String,
    pub launch_envelope_sha256: String,
    pub execution_receipt_sha256: String,
    pub runner: ExecutionRunnerIdentity,
    pub target: ExecutionTargetRef,
    pub issued_at: i64,
    pub expires_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManagedResultReadContext {
    pub format_version: u32,
    pub request: ManagedResultContextRequest,
    pub fence_sha256: String,
    pub observed_at: i64,
    pub expires_at: i64,
}

impl ManagedResultContextRequest {
    pub fn validate(
        &self,
        binding: &Binding,
        target: &ExecutionTargetRef,
        runtime_instance_sha256: &str,
        now: i64,
    ) -> Result<()> {
        identity(&self.runner, binding, target)?;
        window(
            self.issued_at,
            self.expires_at,
            MANAGED_RESULT_READ_MILLIS,
            now,
        )?;
        if self.format_version != 1
            || !uuid(&self.request_id)
            || self.target != *target
            || !valid_sha256(runtime_instance_sha256)
            || self.runtime_instance_sha256 != runtime_instance_sha256
            || !identifier(&self.process_attempt_id)
            || !identifier(&self.run_id)
            || !valid_sha256(&self.launch_envelope_sha256)
            || !valid_sha256(&self.execution_receipt_sha256)
        {
            return Err(Error::Authority);
        }
        Ok(())
    }
}

impl ManagedResultReadContext {
    pub fn validate(
        &self,
        expected: &ManagedResultContextRequest,
        binding: &Binding,
        now: i64,
    ) -> Result<()> {
        self.request.validate(
            binding,
            &expected.target,
            &expected.runtime_instance_sha256,
            now,
        )?;
        window(
            self.observed_at,
            self.expires_at,
            MANAGED_RESULT_READ_MILLIS,
            now,
        )?;
        if self.format_version != 1
            || self.request != *expected
            || !valid_sha256(&self.fence_sha256)
            || self.observed_at < self.request.issued_at
            || self.expires_at > self.request.expires_at
        {
            return Err(Error::Authority);
        }
        Ok(())
    }
}
