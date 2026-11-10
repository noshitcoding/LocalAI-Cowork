//! Version-one private managed transport. These bounded wire records contain no
//! host paths/native identities/credentials. Shape/signature checks do not replace
//! live Broker custody, current controller actor/content rights or qualification.
use super::*;
use cowork_contracts::{
    ExecutionRunnerIdentity, ExecutionTargetRef, ReadWorkspaceTransferChunk,
    WorkspaceTransferManifest,
};
use std::collections::BTreeSet;

pub const MANAGED_READY_PATH: &str = "v2/managed/ready";
pub const MANAGED_INPUTS_PATH: &str = "v2/managed/inputs";
pub const MANAGED_BEGIN_BODY_BYTES: usize = 3 * 1024 * 1024;
pub const MANAGED_CHUNK_BODY_BYTES: usize = 6 * 1024 * 1024;
pub const MANAGED_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
pub const MANAGED_READINESS_MILLIS: i64 = 15_000;
pub const MANAGED_RESULT_READ_MILLIS: i64 = 30_000;
pub const MANAGED_RESULT_RETENTION_MILLIS: i64 = 7 * 24 * 60 * 60 * 1000;
pub const MANAGED_MAX_MISSING_CHUNKS: usize = cowork_domain::workspace_transfer::MAX_ENTRIES
    + (cowork_domain::workspace_transfer::MAX_TOTAL_BYTES
        / cowork_domain::workspace_transfer::CHUNK_BYTES) as usize;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManagedReadyRequest {
    pub format_version: u32,
    pub request_id: String,
    pub environment: ExecutionEnvironmentRef,
    pub target: ExecutionTargetRef,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManagedReadiness {
    pub format_version: u32,
    pub request_id: String,
    pub request_sha256: String,
    pub runner: ExecutionRunnerIdentity,
    pub environment: ExecutionEnvironmentRef,
    pub target: ExecutionTargetRef,
    pub runtime_instance_sha256: String,
    pub observed_at: i64,
    pub expires_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManagedBeginRequest {
    pub format_version: u32,
    pub readiness: Signed<ManagedReadiness>,
    pub launch: Signed<LaunchRequest>,
    pub grant: Signed<ManagedInputGrant>,
    pub manifest: WorkspaceTransferManifest,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[allow(
    clippy::large_enum_variant,
    reason = "Bounded transport DTO retains the existing unboxed receipt API."
)]
pub enum ManagedBeginResponse {
    Input { ticket: Signed<ManagedInputTicket> },
    Execution { receipt: Signed<Receipt> },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManagedInputTicket {
    pub format_version: u32,
    pub input_id: String,
    pub launch_envelope_sha256: String,
    pub grant_envelope_sha256: String,
    pub manifest_sha256: String,
    pub readiness_sha256: String,
    pub runtime_instance_sha256: String,
    pub expires_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManagedInputOperation {
    pub ticket: Signed<ManagedInputTicket>,
}
pub type ManagedMissingRequest = ManagedInputOperation;
pub type ManagedStartRequest = ManagedInputOperation;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedMissingResponse {
    pub ticket_sha256: String,
    pub missing: Vec<ReadWorkspaceTransferChunk>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedChunkRequest {
    pub ticket: Signed<ManagedInputTicket>,
    pub chunk: ReadWorkspaceTransferChunk,
    pub bytes_base64: String,
}
/// Projection of the receiver's original immutable ChunkReceipt, never a run
/// receipt, input grant or output-read authority.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManagedChunkAck {
    pub receipt_sha256: String,
    pub path: String,
    pub offset: u64,
    pub size: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManagedOutputAttestation {
    pub format_version: u32,
    pub runtime_instance_sha256: String,
    pub execution_plan_id: String,
    pub execution_plan_sha256: String,
    pub process_attempt_id: String,
    pub run_id: String,
    pub launch_envelope_sha256: String,
    pub input_manifest_sha256: String,
    pub output_manifest_sha256: String,
    pub execution_receipt_sha256: String,
    pub physical_record_sha256: String,
    pub runner: ExecutionRunnerIdentity,
    pub target: ExecutionTargetRef,
    pub produced_at: i64,
    pub retained_until: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManagedResultManifest {
    pub attestation: Signed<ManagedOutputAttestation>,
    pub manifest: WorkspaceTransferManifest,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedResultReadRequest {
    pub format_version: u32,
    pub runtime_instance_sha256: String,
    pub request_id: String,
    pub actor_id: String,
    /// Digest of freshly resolved controller actor/content authorization. This
    /// is audit context, never a substitute for that actual authorization.
    pub authorization_sha256: String,
    pub execution_plan_id: String,
    pub execution_plan_sha256: String,
    pub process_attempt_id: String,
    pub run_id: String,
    pub launch_envelope_sha256: String,
    pub execution_receipt_sha256: String,
    pub runner: ExecutionRunnerIdentity,
    pub target: ExecutionTargetRef,
    pub operation: ManagedResultReadOperation,
    pub issued_at: i64,
    pub expires_at: i64,
    /// V2 observes the current fence independently from the original execution
    /// epoch. Absence preserves the exact serialized bytes of legacy V1 reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Signed<ManagedResultReadContext>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManagedResultReadOperation {
    Manifest,
    Chunk {
        attestation_sha256: String,
        manifest_sha256: String,
        chunk: ReadWorkspaceTransferChunk,
    },
}

impl ManagedReadiness {
    pub fn validate(
        &self,
        expected: &ManagedReadyRequest,
        binding: &Binding,
        now: i64,
    ) -> Result<()> {
        identity(&self.runner, binding, &expected.target)?;
        window(
            self.observed_at,
            self.expires_at,
            MANAGED_READINESS_MILLIS,
            now,
        )?;
        if self.format_version != 1
            || expected.format_version != 1
            || !uuid(&expected.request_id)
            || self.request_id != expected.request_id
            || self.request_sha256 != canonical_hash(expected)
            || self.environment != expected.environment
            || self.target != expected.target
            || !valid_sha256(&self.runtime_instance_sha256)
        {
            return Err(Error::Authority);
        }
        Ok(())
    }
}
impl ManagedInputTicket {
    pub fn validate(&self, expected: &ManagedBeginRequest, now: i64) -> Result<()> {
        if self.format_version != 1
            || expected.format_version != 1
            || !uuid(&self.input_id)
            || self.launch_envelope_sha256 != envelope_hash(&expected.launch)
            || self.grant_envelope_sha256 != canonical_hash(&expected.grant)
            || self.manifest_sha256 != expected.manifest.sha256
            || self.readiness_sha256 != canonical_hash(&expected.readiness)
            || self.runtime_instance_sha256 != expected.readiness.payload.runtime_instance_sha256
            || !valid_sha256(&self.runtime_instance_sha256)
            || self.expires_at
                != expected
                    .launch
                    .payload
                    .expires_at
                    .min(expected.grant.payload.expires_at)
            || now >= self.expires_at
            || now < expected.launch.payload.issued_at
        {
            return Err(Error::Authority);
        }
        Ok(())
    }
}
impl ManagedMissingResponse {
    pub fn validate(
        &self,
        ticket: &Signed<ManagedInputTicket>,
        manifest: &WorkspaceTransferManifest,
    ) -> Result<()> {
        cowork_domain::workspace_transfer::validate(manifest).map_err(|_| Error::Authority)?;
        if self.ticket_sha256 != canonical_hash(ticket)
            || manifest.sha256 != ticket.payload.manifest_sha256
            || self.missing.len() > MANAGED_MAX_MISSING_CHUNKS
        {
            return Err(Error::Authority);
        }
        let expected = manifest
            .files
            .iter()
            .flat_map(|file| {
                file.chunks
                    .iter()
                    .map(|chunk| (file.path.as_str(), chunk.offset, chunk.sha256.as_str()))
            })
            .collect::<BTreeSet<_>>();
        let mut seen = BTreeSet::new();
        for chunk in &self.missing {
            let key = (
                chunk.path.as_str(),
                chunk.offset,
                chunk.expected_sha256.as_str(),
            );
            if !expected.contains(&key) || !seen.insert(key) {
                return Err(Error::Authority);
            }
        }
        Ok(())
    }
}
impl ManagedOutputAttestation {
    pub fn validate(
        &self,
        input: &ManagedInputGrant,
        launch: &Signed<LaunchRequest>,
        receipt: &Signed<Receipt>,
        manifest: &WorkspaceTransferManifest,
        runtime_instance_sha256: &str,
        now: i64,
    ) -> Result<()> {
        cowork_domain::workspace_transfer::validate(manifest).map_err(|_| Error::Authority)?;
        if !valid_sha256(runtime_instance_sha256)
            || self.runtime_instance_sha256 != runtime_instance_sha256
            || self.format_version != 1
            || self.execution_plan_id != input.execution_plan_id
            || self.execution_plan_sha256 != input.execution_plan_sha256
            || self.process_attempt_id != input.process_attempt_id
            || self.run_id != input.run_id
            || self.runner != input.runner
            || self.target != input.target
            || self.launch_envelope_sha256 != envelope_hash(launch)
            || self.input_manifest_sha256 != input.input_manifest_sha256
            || self.output_manifest_sha256 != manifest.sha256
            || self.execution_receipt_sha256 != canonical_hash(receipt)
            || !valid_sha256(&self.physical_record_sha256)
            || receipt.payload.request != launch.payload
            || receipt.payload.request_envelope_sha256 != self.launch_envelope_sha256
            || !matches!(receipt.payload.state, ExecutionState::Completed { .. })
        {
            return Err(Error::Authority);
        }
        window(
            self.produced_at,
            self.retained_until,
            MANAGED_RESULT_RETENTION_MILLIS,
            now,
        )
    }
}
impl ManagedResultReadRequest {
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
        if !valid_sha256(runtime_instance_sha256)
            || self.runtime_instance_sha256 != runtime_instance_sha256
            || !matches!(self.format_version, 1 | 2)
            || !uuid(&self.request_id)
            || self.target != *target
            || [
                &self.actor_id,
                &self.execution_plan_id,
                &self.process_attempt_id,
                &self.run_id,
            ]
            .iter()
            .any(|s| !identifier(s))
            || [
                &self.authorization_sha256,
                &self.execution_plan_sha256,
                &self.launch_envelope_sha256,
                &self.execution_receipt_sha256,
            ]
            .iter()
            .any(|s| !valid_sha256(s))
        {
            return Err(Error::Authority);
        }
        match (self.format_version, &self.context) {
            (1, None) => {}
            (2, Some(context)) => {
                context.verify("managed_result_context", &binding.runner_public_key)?;
                let actual = &context.payload;
                actual.validate(&actual.request, binding, now)?;
                let original = &actual.request;
                if original.runtime_instance_sha256 != self.runtime_instance_sha256
                    || original.process_attempt_id != self.process_attempt_id
                    || original.run_id != self.run_id
                    || original.launch_envelope_sha256 != self.launch_envelope_sha256
                    || original.execution_receipt_sha256 != self.execution_receipt_sha256
                    || original.runner != self.runner
                    || original.target != self.target
                    || self.issued_at < actual.observed_at
                    || self.expires_at > actual.expires_at
                {
                    return Err(Error::Authority);
                }
            }
            _ => return Err(Error::Authority),
        }
        if let ManagedResultReadOperation::Chunk {
            attestation_sha256,
            manifest_sha256,
            chunk,
        } = &self.operation
            && (!valid_sha256(attestation_sha256)
                || !valid_sha256(manifest_sha256)
                || !valid_sha256(&chunk.expected_sha256)
                || !chunk
                    .offset
                    .is_multiple_of(cowork_domain::workspace_transfer::CHUNK_BYTES)
                || cowork_domain::workspace_transfer::validate_path(&chunk.path).is_err())
        {
            return Err(Error::Authority);
        }
        Ok(())
    }
}
pub(crate) fn identity(
    identity: &ExecutionRunnerIdentity,
    binding: &Binding,
    target: &ExecutionTargetRef,
) -> Result<()> {
    binding.validate()?;
    if identity.runner_id != binding.runner_id
        || identity.generation != binding.generation
        || identity.adapter_build_sha256 != binding.adapter_build_sha256
        || identity.control_public_key != binding.control_public_key
        || identity.runner_public_key != binding.runner_public_key
        || target.revision <= 0
        || cowork_domain::execution_targets::execution_target_id(identity)
            .map_err(|_| Error::Authority)?
            != target.target_id
        || cowork_domain::execution_targets::runner_identity_hash(identity)
            .map_err(|_| Error::Authority)?
            != target.identity_sha256
    {
        return Err(Error::Authority);
    }
    Ok(())
}
pub(crate) fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
}
pub(crate) fn uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value)
}
pub(crate) fn window(issued: i64, expires: i64, maximum: i64, now: i64) -> Result<()> {
    if issued <= 0
        || now < issued
        || now >= expires
        || expires
            .checked_sub(issued)
            .is_none_or(|n| n <= 0 || n > maximum)
    {
        return Err(Error::Expired);
    }
    Ok(())
}

#[cfg(test)]
#[path = "managed_transport_tests.rs"]
mod tests;
