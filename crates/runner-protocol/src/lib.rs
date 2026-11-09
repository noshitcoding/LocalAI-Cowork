//! Authenticated, durable runner admission. Platform adapters remain responsible for
//! qualified isolation and actual physical termination, never inferred from a signature.
mod broker;
mod managed_input;
mod managed_result_context;
mod managed_result_delivery;
mod managed_transport;
pub use broker::{
    Admission, Broker, ClockRollbackObservation, DispatchedLaunch, InputTransferPermit,
    LaunchPermit, ManagedResultReadPermit,
};
pub use managed_input::ManagedInputGrant;
pub use managed_result_context::*;
pub use managed_result_delivery::*;
pub use managed_transport::*;

use cowork_contracts::{ExecutionEnvironmentRef, ProcessInvocation, ProcessResult, ToolCall};
use cowork_domain::{
    canonical_hash,
    processes::{process_invocation, valid_sha256, validate_process_result},
};
use ring::{
    rand::SystemRandom,
    signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Runner authority denied")]
    Authority,
    #[error("Runner request conflicts with retained execution")]
    Conflict,
    #[error("Runner clock authority moved backwards")]
    ClockRollback,
    #[error("Runner authority expired")]
    Expired,
    #[error("Runner result remains uncertain")]
    Uncertain,
    #[error("Runner record not found")]
    NotFound,
    #[error("Runner database error")]
    Database(#[from] sqlx::Error),
    #[error("Runner encoding error")]
    Encoding(#[from] serde_json::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

/// Private keys are never Debug, Serialize, API payloads or journal contents.
#[derive(Clone)]
pub struct SigningKey(Arc<Ed25519KeyPair>);
impl SigningKey {
    pub fn from_pkcs8(bytes: &[u8]) -> Result<Self> {
        Ok(Self(Arc::new(
            Ed25519KeyPair::from_pkcs8(bytes).map_err(|_| Error::Authority)?,
        )))
    }
    pub fn generate_pkcs8() -> Result<Vec<u8>> {
        Ok(Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
            .map_err(|_| Error::Authority)?
            .as_ref()
            .to_vec())
    }
    pub fn public_key(&self) -> Vec<u8> {
        self.0.public_key().as_ref().to_vec()
    }
    pub fn sign<T: Serialize>(&self, purpose: &str, payload: T) -> Result<Signed<T>> {
        let message = signing_message(purpose, &payload)?;
        Ok(Signed {
            payload,
            signature: self.0.sign(&message).as_ref().to_vec(),
        })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Signed<T> {
    pub payload: T,
    pub signature: Vec<u8>,
}
impl<T: Serialize> Signed<T> {
    pub fn verify(&self, purpose: &str, key: &[u8]) -> Result<()> {
        if key.len() != 32 || self.signature.len() != 64 {
            return Err(Error::Authority);
        }
        UnparsedPublicKey::new(&ED25519, key)
            .verify(&signing_message(purpose, &self.payload)?, &self.signature)
            .map_err(|_| Error::Authority)
    }
}
fn signing_message<T: Serialize>(purpose: &str, value: &T) -> Result<Vec<u8>> {
    if !matches!(
        purpose,
        "launch"
            | "fence"
            | "receipt"
            | "environment_observation"
            | "execution_lease"
            | "execution_lease_receipt"
            | "environment_install"
            | "environment_qualification"
            | "physical_outcome"
            | "windows_orphan_plan"
            | "windows_orphan_confirmation"
            | "windows_orphan_receipt"
            | "sandbox_lifecycle"
            | "sandbox_lifecycle_receipt"
            | "sandbox_preview_request"
            | "sandbox_preview"
            | "managed_input_grant"
            | "input_custody"
            | "managed_readiness"
            | "managed_input_ticket"
            | "managed_output"
            | "managed_result_read"
            | "managed_result_context_request"
            | "managed_result_context"
            | "managed_result_delivery_request"
            | "managed_result_delivery_receipt"
            | "managed_result_retirement_query"
            | "managed_result_retirement_receipt"
    ) {
        return Err(Error::Authority);
    }
    let data = serde_json::to_vec(value)?;
    if data.len() > 2 * 1024 * 1024 {
        return Err(Error::Authority);
    }
    let mut out = format!("localai-cowork/v2/runner/1/{purpose}\0").into_bytes();
    out.extend(data);
    Ok(out)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub runner_id: String,
    pub generation: u64,
    pub adapter_build_sha256: String,
    pub control_public_key: Vec<u8>,
    pub runner_public_key: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentInstall {
    pub environment_id: String,
    pub version_id: String,
    pub definition: cowork_domain::environments::EnvironmentDefinition,
}

pub use cowork_contracts::{SandboxLifecycleOperation, SandboxLifecycleState};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SandboxLifecycleRequest {
    pub request_id: String,
    pub runner_id: String,
    pub runner_generation: u64,
    pub adapter_build_sha256: String,
    pub project_id: String,
    pub workspace_resource_id: String,
    pub environment: ExecutionEnvironmentRef,
    pub operation: SandboxLifecycleOperation,
    pub expected_sandbox_uid: Option<String>,
    pub issued_at: i64,
    pub expires_at: i64,
}
impl SandboxLifecycleRequest {
    pub fn validate(&self, binding: &Binding, now: i64) -> Result<()> {
        binding.validate()?;
        for value in [
            &self.request_id,
            &self.project_id,
            &self.workspace_resource_id,
            &self.environment.environment_id,
            &self.environment.version_id,
        ] {
            text(value)?;
        }
        if self.runner_id != binding.runner_id
            || self.runner_generation != binding.generation
            || self.adapter_build_sha256 != binding.adapter_build_sha256
            || !valid_sha256(&self.environment.definition_sha256)
            || !valid_sha256(&self.environment.qualification_sha256)
            || self.issued_at > now
            || self.expires_at <= now
            || self
                .expires_at
                .checked_sub(self.issued_at)
                .is_none_or(|ms| !(1..=30_000).contains(&ms))
            || self
                .expected_sandbox_uid
                .as_ref()
                .is_some_and(|uid| uid.is_empty() || uid.len() > 128)
        {
            return Err(Error::Authority);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SandboxLifecycleReceipt {
    pub request: SandboxLifecycleRequest,
    pub sandbox_name: String,
    pub sandbox_uid: Option<String>,
    pub pod_uid: Option<String>,
    pub workspace_pvc: String,
    pub state: SandboxLifecycleState,
    pub observed_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SandboxPreviewRequest {
    pub request_id: String,
    pub runner_id: String,
    pub runner_generation: u64,
    pub adapter_build_sha256: String,
    pub project_id: String,
    pub workspace_resource_id: String,
    pub environment: ExecutionEnvironmentRef,
    pub expected_session_uid: String,
    pub port: u16,
    pub issued_at: i64,
    pub expires_at: i64,
}
impl SandboxPreviewRequest {
    pub fn validate(&self, binding: &Binding, now: i64) -> Result<()> {
        SandboxLifecycleRequest {
            request_id: self.request_id.clone(),
            runner_id: self.runner_id.clone(),
            runner_generation: self.runner_generation,
            adapter_build_sha256: self.adapter_build_sha256.clone(),
            project_id: self.project_id.clone(),
            workspace_resource_id: self.workspace_resource_id.clone(),
            environment: self.environment.clone(),
            operation: SandboxLifecycleOperation::Inspect,
            expected_sandbox_uid: Some(self.expected_session_uid.clone()),
            issued_at: self.issued_at,
            expires_at: self.expires_at,
        }
        .validate(binding, now)?;
        if self.expected_session_uid.is_empty()
            || self.port == 0
            || matches!(self.port, 8080 | 9090)
        {
            return Err(Error::Authority);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SandboxPreviewGrant {
    pub preview_id: String,
    pub runner_id: String,
    pub runner_generation: u64,
    pub project_id: String,
    pub workspace_resource_id: String,
    pub environment: ExecutionEnvironmentRef,
    pub sandbox_uid: String,
    pub pod_uid: String,
    pub namespace: String,
    pub service_dns: String,
    pub port: u16,
    pub preview_host: String,
    pub issued_at: i64,
    pub expires_at: i64,
}
impl Binding {
    fn validate(&self) -> Result<()> {
        text(&self.runner_id)?;
        if self.generation == 0
            || self.generation > i64::MAX as u64
            || !valid_sha256(&self.adapter_build_sha256)
            || self.control_public_key.len() != 32
            || self.runner_public_key.len() != 32
        {
            return Err(Error::Authority);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LaunchRequest {
    pub format_version: u32,
    pub runner_id: String,
    pub runner_generation: u64,
    pub adapter_build_sha256: String,
    pub process_attempt_id: String,
    pub run_id: String,
    pub tool_call_id: String,
    pub request_sha256: String,
    pub worker_fence_sha256: String,
    pub lease_epoch: u64,
    pub environment: ExecutionEnvironmentRef,
    pub workspace_resource_id: String,
    pub invocation: ProcessInvocation,
    pub issued_at: i64,
    /// Launch authority lifetime, distinct from the original execution deadline.
    pub expires_at: i64,
    pub execution_deadline: i64,
}
impl LaunchRequest {
    fn validate(&self, binding: &Binding) -> Result<()> {
        for id in [
            &self.process_attempt_id,
            &self.run_id,
            &self.tool_call_id,
            &self.workspace_resource_id,
            &self.environment.environment_id,
            &self.environment.version_id,
        ] {
            text(id)?;
        }
        if self.format_version != 1
            || self.runner_id != binding.runner_id
            || self.runner_generation != binding.generation
            || self.adapter_build_sha256 != binding.adapter_build_sha256
            || self.lease_epoch == 0
            || self.lease_epoch > i64::MAX as u64
            || [
                &self.request_sha256,
                &self.worker_fence_sha256,
                &self.environment.definition_sha256,
                &self.environment.qualification_sha256,
            ]
            .into_iter()
            .any(|v| !valid_sha256(v))
            || self.issued_at <= 0
            || self.expires_at <= self.issued_at
            || self
                .expires_at
                .checked_sub(self.issued_at)
                .is_none_or(|d| d > 30_000)
            || self.execution_deadline < self.expires_at
            || self
                .execution_deadline
                .checked_sub(self.issued_at)
                .is_none_or(|d| d > 86_400_000)
        {
            return Err(Error::Authority);
        }
        let tool = ToolCall {
            id: self.tool_call_id.clone(),
            name: "execute_program".into(),
            arguments: serde_json::to_value(&self.invocation)?,
        };
        process_invocation(&tool).map_err(|_| Error::Authority)?;
        Ok(())
    }
    fn assert_fresh(&self, now: i64) -> Result<()> {
        if self.issued_at > now || now >= self.expires_at || now >= self.execution_deadline {
            return Err(Error::Expired);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FenceRequest {
    pub runner_id: String,
    pub runner_generation: u64,
    pub run_id: String,
    pub lease_epoch: u64,
    pub revoked: bool,
}

/// Short, controller-signed execution authority for one already-dispatched launch.
/// The revision is the controller's monotonically increasing worker lease revision;
/// an exact replay retains its original expiry and never grants a dispatch permit.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionLeaseRenewal {
    pub runner_id: String,
    pub runner_generation: u64,
    pub adapter_build_sha256: String,
    pub process_attempt_id: String,
    pub run_id: String,
    pub request_envelope_sha256: String,
    pub worker_fence_sha256: String,
    pub lease_epoch: u64,
    pub revision: u64,
    pub issued_at: i64,
    pub expires_at: i64,
}
impl ExecutionLeaseRenewal {
    fn validate(&self, binding: &Binding, receipt: &Receipt) -> Result<()> {
        let launch = &receipt.request;
        if self.runner_id != binding.runner_id
            || self.runner_generation != binding.generation
            || self.adapter_build_sha256 != binding.adapter_build_sha256
            || self.process_attempt_id != launch.process_attempt_id
            || self.run_id != launch.run_id
            || self.request_envelope_sha256 != receipt.request_envelope_sha256
            || self.worker_fence_sha256 != launch.worker_fence_sha256
            || self.lease_epoch != launch.lease_epoch
            || self.revision <= launch.lease_epoch
            || self.revision > i64::MAX as u64
            || self.issued_at < launch.issued_at
            || self.expires_at <= self.issued_at
            || self.expires_at > launch.execution_deadline
            || self
                .expires_at
                .checked_sub(self.issued_at)
                .is_none_or(|duration| duration > 30_000)
        {
            return Err(Error::Authority);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutionState {
    Admitted,
    /// Permission consumed once. A crash here is uncertain, never a new permit.
    DispatchCommitted,
    Completed {
        result: ProcessResult,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub request: LaunchRequest,
    pub request_envelope_sha256: String,
    pub state: ExecutionState,
    pub revision: i64,
    pub updated_at: i64,
}
impl Receipt {
    fn validate(&self, binding: &Binding) -> Result<()> {
        self.request.validate(binding)?;
        if !valid_sha256(&self.request_envelope_sha256) || self.updated_at < self.request.issued_at
        {
            return Err(Error::Authority);
        }
        match &self.state {
            ExecutionState::Admitted if self.revision == 1 => {}
            ExecutionState::DispatchCommitted if self.revision == 2 => {}
            ExecutionState::Completed { result } if self.revision == 3 => {
                validate_process_result(result).map_err(|_| Error::Authority)?;
            }
            _ => return Err(Error::Authority),
        }
        Ok(())
    }
}
fn text(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 128 || value.contains('\0') {
        return Err(Error::Authority);
    }
    Ok(())
}
pub fn envelope_hash(request: &Signed<LaunchRequest>) -> String {
    canonical_hash(request)
}

#[cfg(test)]
mod postgres_tests;
#[cfg(test)]
mod sandbox_tests;
#[cfg(test)]
mod tests;
