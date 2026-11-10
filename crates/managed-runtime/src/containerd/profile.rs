//! Binds installed immutable trees to the existing signed environment contract.
//! Only the runner's actual qualification key is accepted; metadata alone never
//! qualifies a profile. There is no renderer/client construction route.
use super::*;
use super::{
    cgroups::HardLimits, deadline_process::FenceExecutable, gvisor::GvisorTools,
    rootfs::SealedRootfs, runsc_driver::RunscDriverExecutable,
};
use cowork_contracts::{ExecutionEnvironmentRef, ExecutionRunnerIdentity, ExecutionTargetRef};
use cowork_domain::{
    canonical_hash,
    environments::{EnvironmentArtifact, EnvironmentDefinition, EnvironmentQualification},
};
use cowork_runner_protocol::{Binding, Signed};

pub struct ManagedProfile {
    pub(super) binding: Binding,
    pub(super) target: ExecutionTargetRef,
    pub(super) reference: ExecutionEnvironmentRef,
    pub(super) definition: EnvironmentDefinition,
    pub(super) tools: GvisorTools,
    pub(super) rootfs: SealedRootfs,
    pub(super) fence: FenceExecutable,
    pub(super) driver: RunscDriverExecutable,
    pub(super) output_bytes: u64,
    pub(super) maximum_files: u32,
}
/// All executable inputs are installed/pinned by trusted package bootstrap.
/// In particular the independent fence is mandatory; no thread-only fallback.
pub struct RuntimeArtifacts {
    pub tools: GvisorTools,
    pub rootfs: SealedRootfs,
    pub fence: FenceExecutable,
    pub driver: RunscDriverExecutable,
}
impl ManagedProfile {
    pub fn from_qualification(
        binding: Binding,
        target: ExecutionTargetRef,
        reference: ExecutionEnvironmentRef,
        definition: EnvironmentDefinition,
        qualification: Signed<EnvironmentQualification>,
        artifacts: RuntimeArtifacts,
    ) -> Result<Self> {
        let RuntimeArtifacts {
            tools,
            rootfs,
            fence,
            driver,
        } = artifacts;
        definition.validate().map_err(|_| Error::TaskAdmission)?;
        qualification
            .verify("environment_qualification", &binding.runner_public_key)
            .map_err(|_| Error::TaskAdmission)?;
        let identity = ExecutionRunnerIdentity {
            runner_id: binding.runner_id.clone(),
            generation: binding.generation,
            adapter_build_sha256: binding.adapter_build_sha256.clone(),
            control_public_key: binding.control_public_key.clone(),
            runner_public_key: binding.runner_public_key.clone(),
        };
        let q = &qualification.payload;
        if target.revision <= 0
            || target.target_id
                != cowork_domain::execution_targets::execution_target_id(&identity)
                    .map_err(|_| Error::TaskAdmission)?
            || target.identity_sha256
                != cowork_domain::execution_targets::runner_identity_hash(&identity)
                    .map_err(|_| Error::TaskAdmission)?
            || q.runner_id != binding.runner_id
            || q.runner_generation != binding.generation
            || q.adapter_build_sha256 != binding.adapter_build_sha256
            || q.verified_at <= 0
            || !crate::valid_hash(&q.report_sha256)
            || q.definition_sha256 != canonical_hash(&definition)
            || q.artifact_sha256 != definition.artifact.sha256()
            || !definition
                .required_capabilities()
                .is_subset(&q.capabilities)
            || reference.definition_sha256 != q.definition_sha256
            || reference.qualification_sha256 != canonical_hash(q)
            || reference.environment_id.is_empty()
            || reference.version_id.is_empty()
        {
            return Err(Error::TaskAdmission);
        }
        let EnvironmentArtifact::ManagedLinuxPackage {
            rootfs_manifest_sha256,
            gvisor_manifest_sha256,
            output_bytes,
            maximum_files,
            ..
        } = &definition.artifact
        else {
            return Err(Error::TaskAdmission);
        };
        if rootfs.manifest_sha256() != rootfs_manifest_sha256
            || tools.manifest_sha256() != gvisor_manifest_sha256
        {
            return Err(Error::TaskAdmission);
        }
        let output_bytes = *output_bytes;
        let maximum_files = *maximum_files;
        let profile = Self {
            binding,
            target,
            reference,
            definition,
            tools,
            rootfs,
            fence,
            driver,
            output_bytes,
            maximum_files,
        };
        profile.verify()?;
        Ok(profile)
    }
    pub fn verify(&self) -> Result<()> {
        self.tools.verify()?;
        self.rootfs.verify()?;
        self.rootfs.executable("bin/cowork-pause")?;
        self.fence.verify()?;
        self.driver.verify()?;
        self.limits().validate()
    }
    pub fn limits(&self) -> HardLimits {
        HardLimits {
            cpu_millis: self.definition.limits.cpu_millis,
            memory_bytes: self.definition.limits.memory_bytes,
            processes: self.definition.limits.processes,
        }
    }
    pub fn reference(&self) -> &ExecutionEnvironmentRef {
        &self.reference
    }
}
