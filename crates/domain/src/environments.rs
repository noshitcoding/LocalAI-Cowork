//! Private runner/control-plane records. None of these inputs is a renderer or MCP authority.
use crate::{Error, Result, canonical_hash, valid_text};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
#[path = "environments/sandbox_tests.rs"]
mod sandbox_tests;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EnvironmentArtifact {
    LinuxImage {
        reference: String,
    },
    /// Linux tooling whose qualification requires a hardware-isolated Kata guest.
    /// Kept distinct from the existing gVisor image so old pins keep their meaning.
    LinuxKataImage {
        reference: String,
    },
    /// Explicit single-owner local Docker profile. Storage is a monitored budget,
    /// not the hard disk quota required of server/gVisor environment pins.
    LocalLinuxImage {
        reference: String,
    },
    /// App-owned containerd/gVisor package with sealed installed trees. Kept
    /// separate from Docker/Kata images; no old qualification can become this
    /// profile through a label or client-selected runtime name.
    ManagedLinuxPackage {
        package_sha256: String,
        rootfs_manifest_sha256: String,
        gvisor_manifest_sha256: String,
        maximum_files: u32,
        output_bytes: u64,
    },
    WindowsBundle {
        sha256: String,
    },
}
impl EnvironmentArtifact {
    pub fn sha256(&self) -> &str {
        match self {
            Self::LinuxImage { reference } | Self::LinuxKataImage { reference } => {
                reference.rsplit_once("@sha256:").map_or("", |p| p.1)
            }
            Self::LocalLinuxImage { reference } => reference
                .strip_prefix("sha256:")
                .unwrap_or_else(|| reference.rsplit_once("@sha256:").map_or("", |p| p.1)),
            Self::WindowsBundle { sha256 } => sha256,
            Self::ManagedLinuxPackage { package_sha256, .. } => package_sha256,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentLimits {
    pub cpu_millis: u32,
    pub memory_bytes: u64,
    pub processes: u32,
    pub scratch_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentPackage {
    pub name: String,
    pub version: String,
    /// Hash of the installer/source artifact; not a URL containing credentials.
    pub source_sha256: String,
}

/// Operator-defined egress policy identity, never a model-selected URL or credential.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EnvironmentNetwork {
    Denied,
    WebProxy { policy_id: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentDefinition {
    pub format_version: u32,
    pub artifact: EnvironmentArtifact,
    /// Package/image-relative executable paths. Host paths and shell command strings are forbidden.
    pub programs: BTreeMap<String, String>,
    pub packages: Vec<EnvironmentPackage>,
    pub limits: EnvironmentLimits,
    /// Absent on historical format 1; omitted serialization preserves existing pins.
    /// Format 2 requires an explicit policy, which a qualified adapter must enforce.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<EnvironmentNetwork>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum IsolationCapability {
    FilesystemBoundary,
    NetworkDenied,
    NetworkRestricted,
    CredentialsIsolated,
    ProcessTreeContainment,
    ResourceLimits,
    WindowsRestrictedIdentity,
    WindowsPrivateDesktop,
    WindowsAclRecovery,
    WindowsAclSerialization,
    LinuxGvisor,
    LinuxKata,
    LinuxLocalContainer,
    LocalContainerResourceLimits,
}

/// A report from a trusted, authenticated, qualified adapter. A hash is provenance,
/// not authentication; never construct this from client/MCP capability claims.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentQualification {
    pub runner_id: String,
    pub runner_generation: u64,
    pub adapter_build_sha256: String,
    pub artifact_sha256: String,
    pub definition_sha256: String,
    pub report_sha256: String,
    pub capabilities: BTreeSet<IsolationCapability>,
    pub verified_at: i64,
}

/// Fresh trusted observation, not a cached preflight response. Launch must recheck it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentObservation {
    pub runner_id: String,
    pub runner_generation: u64,
    pub adapter_build_sha256: String,
    pub artifact_sha256: String,
    pub definition_sha256: String,
    pub capabilities: BTreeSet<IsolationCapability>,
    pub observed_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum EnvironmentVersionState {
    Staged,
    Rejected {
        reason_code: String,
    },
    Qualified {
        qualification: EnvironmentQualification,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentVersion {
    pub id: String,
    pub definition: EnvironmentDefinition,
    pub sha256: String,
    pub created_at: i64,
    pub state: EnvironmentVersionState,
    pub revoked: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentReceipt {
    pub key: String,
    pub request_sha256: String,
    pub revision: i64,
    pub kind: String,
    pub version_id: Option<String>,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentRecord {
    pub format_version: u32,
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub revision: i64,
    pub active_version_id: Option<String>,
    pub revoked: bool,
    pub versions: Vec<EnvironmentVersion>,
    /// Each confirmed state transition and its idempotency receipt share one DB commit.
    pub receipts: Vec<EnvironmentReceipt>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum EnvironmentChange {
    Stage {
        definition: EnvironmentDefinition,
    },
    Qualify {
        version_id: String,
        qualification: EnvironmentQualification,
    },
    Reject {
        version_id: String,
        reason_code: String,
    },
    Activate {
        version_id: String,
        observation: EnvironmentObservation,
    },
    RevokeVersion {
        version_id: String,
    },
    Revoke,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentPin {
    pub environment_id: String,
    pub project_id: String,
    pub version_id: String,
    pub definition_sha256: String,
    pub qualification_sha256: String,
    pub definition: EnvironmentDefinition,
}
impl EnvironmentPin {
    pub fn reference(&self) -> cowork_contracts::ExecutionEnvironmentRef {
        cowork_contracts::ExecutionEnvironmentRef {
            environment_id: self.environment_id.clone(),
            version_id: self.version_id.clone(),
            definition_sha256: self.definition_sha256.clone(),
            qualification_sha256: self.qualification_sha256.clone(),
        }
    }
}

fn invalid(message: &str) -> Error {
    Error::Validation(message.into())
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
}
fn relative_program(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && !value.starts_with('/')
        && value.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.ends_with('.')
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                && !matches!(
                    part.split('.')
                        .next()
                        .unwrap_or("")
                        .to_ascii_uppercase()
                        .as_str(),
                    "CON"
                        | "PRN"
                        | "AUX"
                        | "NUL"
                        | "COM1"
                        | "COM2"
                        | "COM3"
                        | "COM4"
                        | "COM5"
                        | "COM6"
                        | "COM7"
                        | "COM8"
                        | "COM9"
                        | "LPT1"
                        | "LPT2"
                        | "LPT3"
                        | "LPT4"
                        | "LPT5"
                        | "LPT6"
                        | "LPT7"
                        | "LPT8"
                        | "LPT9"
                )
        })
}

impl EnvironmentDefinition {
    pub fn validate(&self) -> Result<()> {
        let denied_format = matches!(
            (self.format_version, &self.network),
            (1, None) | (2, Some(EnvironmentNetwork::Denied))
        );
        let proxy_format = matches!((self.format_version, &self.artifact, &self.network),
                    (2, EnvironmentArtifact::LinuxKataImage { .. },
                        Some(EnvironmentNetwork::WebProxy { policy_id })) if key(policy_id));
        let managed_format = matches!(
            (self.format_version, &self.artifact, &self.network),
            (
                3,
                EnvironmentArtifact::ManagedLinuxPackage { .. },
                Some(EnvironmentNetwork::Denied)
            )
        );
        if !digest(self.artifact.sha256())
            || !(denied_format || proxy_format || managed_format)
            || matches!(
                self.artifact,
                EnvironmentArtifact::ManagedLinuxPackage { .. }
            ) != managed_format
        {
            return Err(invalid("Unknown environment format or unpinned artifact"));
        }
        if let EnvironmentArtifact::ManagedLinuxPackage {
            rootfs_manifest_sha256,
            gvisor_manifest_sha256,
            maximum_files,
            output_bytes,
            ..
        } = &self.artifact
            && (!digest(rootfs_manifest_sha256)
                || !digest(gvisor_manifest_sha256)
                || !(128..=32768).contains(maximum_files)
                || !maximum_files.is_power_of_two()
                || !(1..=128 * 1024 * 1024).contains(output_bytes)
                || !(16..=1024).contains(&self.limits.processes)
                || self.limits.memory_bytes > 64 * 1024 * 1024 * 1024
                || self.limits.scratch_bytes > 8 * 1024 * 1024 * 1024
                || !self.limits.scratch_bytes.is_multiple_of(1024 * 1024))
        {
            return Err(invalid(
                "Managed runtime package manifests or hard limits are invalid",
            ));
        }
        if let EnvironmentArtifact::LinuxImage { reference }
        | EnvironmentArtifact::LinuxKataImage { reference }
        | EnvironmentArtifact::LocalLinuxImage { reference } = &self.artifact
        {
            let local_id = matches!(&self.artifact, EnvironmentArtifact::LocalLinuxImage { .. })
                && reference.starts_with("sha256:");
            let name = if local_id {
                "local-image"
            } else {
                reference
                    .rsplit_once("@sha256:")
                    .ok_or_else(|| invalid("Image digest missing"))?
                    .0
            };
            if name.is_empty()
                || name.len() > 255
                || name.starts_with('-')
                || name
                    .split('/')
                    .any(|p| p.is_empty() || p == "." || p == "..")
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b".-_/:".contains(&b))
            {
                return Err(invalid(
                    "Image identity must be an explicit digest reference",
                ));
            }
        }
        let l = &self.limits;
        if !(100..=64_000).contains(&l.cpu_millis)
            || !(64 * 1024 * 1024..=256 * 1024 * 1024 * 1024).contains(&l.memory_bytes)
            || !(8..=4096).contains(&l.processes)
            || !(16 * 1024 * 1024..=1024 * 1024 * 1024 * 1024).contains(&l.scratch_bytes)
        {
            return Err(invalid("Environment resource limits are out of range"));
        }
        if self.programs.is_empty()
            || self.programs.len() > 128
            || self
                .programs
                .iter()
                .any(|(k, v)| !key(k) || !relative_program(v))
        {
            return Err(invalid("Invalid environment program map"));
        }
        let mut names = BTreeSet::new();
        if self.packages.len() > 128
            || self.packages.iter().any(|p| {
                !key(&p.name)
                    || !key(&p.version)
                    || !digest(&p.source_sha256)
                    || !names.insert(p.name.to_ascii_lowercase())
            })
        {
            return Err(invalid(
                "Packages must have unique names, exact versions and source hashes",
            ));
        }
        Ok(())
    }
    pub fn required_capabilities(&self) -> BTreeSet<IsolationCapability> {
        use IsolationCapability::*;
        let mut required = BTreeSet::from([
            FilesystemBoundary,
            CredentialsIsolated,
            ProcessTreeContainment,
        ]);
        required.insert(
            if matches!(self.artifact, EnvironmentArtifact::LocalLinuxImage { .. }) {
                LocalContainerResourceLimits
            } else {
                ResourceLimits
            },
        );
        required.insert(match self.network {
            Some(EnvironmentNetwork::WebProxy { .. }) => NetworkRestricted,
            _ => NetworkDenied,
        });
        match self.artifact {
            EnvironmentArtifact::LinuxImage { .. }
            | EnvironmentArtifact::ManagedLinuxPackage { .. } => {
                required.insert(LinuxGvisor);
            }
            EnvironmentArtifact::LinuxKataImage { .. } => {
                required.insert(LinuxKata);
            }
            EnvironmentArtifact::LocalLinuxImage { .. } => {
                required.insert(LinuxLocalContainer);
            }
            EnvironmentArtifact::WindowsBundle { .. } => {
                required.extend([
                    WindowsRestrictedIdentity,
                    WindowsPrivateDesktop,
                    WindowsAclRecovery,
                    WindowsAclSerialization,
                ]);
            }
        }
        required
    }
    fn check_qualification(&self, q: &EnvironmentQualification) -> Result<()> {
        self.validate()?;
        if !key(&q.runner_id)
            || q.runner_generation == 0
            || !digest(&q.adapter_build_sha256)
            || !digest(&q.report_sha256)
            || q.verified_at <= 0
            || q.definition_sha256 != canonical_hash(self)
            || q.artifact_sha256 != self.artifact.sha256()
            || !self.required_capabilities().is_subset(&q.capabilities)
        {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
}

impl EnvironmentRecord {
    pub fn validate(&self) -> Result<()> {
        if self.format_version != 1
            || !key(&self.id)
            || !key(&self.project_id)
            || self.revision != self.receipts.len() as i64 + 1
            || self.versions.len() > 256
            || self.receipts.len() > 2049
            || (self.receipts.len() == 2049 && !self.revoked)
        {
            return Err(invalid("Invalid environment registry record"));
        }
        valid_text(&self.name, 200)?;
        let mut ids = BTreeSet::new();
        for v in &self.versions {
            v.definition.validate()?;
            if !key(&v.id)
                || !ids.insert(&v.id)
                || v.sha256 != canonical_hash(&v.definition)
                || v.created_at <= 0
            {
                return Err(invalid("Invalid immutable environment version"));
            }
            match &v.state {
                EnvironmentVersionState::Qualified { qualification } => {
                    v.definition.check_qualification(qualification)?;
                    if qualification.verified_at < v.created_at {
                        return Err(Error::Forbidden);
                    }
                }
                EnvironmentVersionState::Rejected { reason_code } if !key(reason_code) => {
                    return Err(Error::Forbidden);
                }
                _ => {}
            }
        }
        if let Some(active) = &self.active_version_id
            && !matches!(
                self.version(active)?.state,
                EnvironmentVersionState::Qualified { .. }
            )
        {
            return Err(invalid("Active environment has no qualification"));
        }
        let mut keys = BTreeSet::new();
        let mut states = BTreeMap::<&str, (&str, bool)>::new();
        let mut active = None;
        let mut revoked = false;
        for (i, r) in self.receipts.iter().enumerate() {
            if !key(&r.key)
                || !keys.insert(&r.key)
                || !digest(&r.request_sha256)
                || r.revision != i as i64 + 2
                || r.created_at <= 0
                || !matches!(
                    r.kind.as_str(),
                    "staged"
                        | "qualified"
                        | "rejected"
                        | "activated"
                        | "version_revoked"
                        | "revoked"
                )
                || r.version_id.as_ref().is_some_and(|v| !ids.contains(v))
            {
                return Err(invalid("Invalid environment transition receipt"));
            }
            if revoked {
                return Err(invalid("A revoked environment has later transitions"));
            }
            if r.kind == "revoked" {
                if r.version_id.is_some() {
                    return Err(Error::Forbidden);
                }
                revoked = true;
                continue;
            }
            let id = r.version_id.as_deref().ok_or(Error::Forbidden)?;
            match r.kind.as_str() {
                "staged" => {
                    if states.insert(id, ("staged", false)).is_some()
                        || self.version(id)?.created_at != r.created_at
                    {
                        return Err(Error::Forbidden);
                    }
                }
                "qualified" | "rejected" => {
                    let state = states.get_mut(id).ok_or(Error::Forbidden)?;
                    if *state != ("staged", false) {
                        return Err(Error::Forbidden);
                    }
                    state.0 = &r.kind;
                    if let EnvironmentVersionState::Qualified { qualification } =
                        &self.version(id)?.state
                        && qualification.verified_at > r.created_at
                    {
                        return Err(Error::Forbidden);
                    }
                }
                "activated" => {
                    if states.get(id) != Some(&("qualified", false)) {
                        return Err(Error::Forbidden);
                    }
                    active = Some(id);
                }
                "version_revoked" => states.get_mut(id).ok_or(Error::Forbidden)?.1 = true,
                _ => return Err(Error::Forbidden),
            }
        }
        if active != self.active_version_id.as_deref()
            || revoked != self.revoked
            || states.len() != self.versions.len()
        {
            return Err(invalid(
                "Environment state disagrees with transition receipts",
            ));
        }
        for v in &self.versions {
            let state = match &v.state {
                EnvironmentVersionState::Staged => "staged",
                EnvironmentVersionState::Qualified { .. } => "qualified",
                EnvironmentVersionState::Rejected { .. } => "rejected",
            };
            if states.get(v.id.as_str()) != Some(&(state, v.revoked)) {
                return Err(Error::Forbidden);
            }
        }
        Ok(())
    }
    pub fn version(&self, id: &str) -> Result<&EnvironmentVersion> {
        self.versions
            .iter()
            .find(|v| v.id == id)
            .ok_or(Error::NotFound)
    }
    pub fn apply(
        &mut self,
        expected_revision: i64,
        request_key: &str,
        change: &EnvironmentChange,
        version_id: &str,
        now: i64,
    ) -> Result<EnvironmentReceipt> {
        self.validate()?;
        if !key(request_key) || now <= 0 {
            return Err(invalid("Invalid environment request"));
        }
        let request_sha256 = canonical_hash(&(expected_revision, change));
        if let Some(receipt) = self.receipts.iter().find(|r| r.key == request_key) {
            return if receipt.request_sha256 == request_sha256 {
                Ok(receipt.clone())
            } else {
                Err(Error::Conflict)
            };
        }
        if self.revision != expected_revision {
            return Err(Error::Conflict);
        }
        if self.revoked {
            return Err(Error::Forbidden);
        }
        if !matches!(change, EnvironmentChange::Revoke)
            && self.receipts.last().is_some_and(|r| now < r.created_at)
        {
            return Err(Error::Forbidden);
        }
        // Keep one terminal revocation slot even when ordinary history capacity is full.
        if self.receipts.len() >= 2048 && !matches!(change, EnvironmentChange::Revoke) {
            return Err(invalid("Environment transition capacity exhausted"));
        }
        // Work on a copy so validation failure never leaves a partially changed caller value.
        let mut next = self.clone();
        let (kind, affected) = match change {
            EnvironmentChange::Stage { definition } => {
                definition.validate()?;
                if next.versions.len() >= 256
                    || !key(version_id)
                    || next.versions.iter().any(|v| v.id == version_id)
                {
                    return Err(invalid("Invalid or exhausted environment version identity"));
                }
                next.versions.push(EnvironmentVersion {
                    id: version_id.into(),
                    definition: definition.clone(),
                    sha256: canonical_hash(definition),
                    created_at: now,
                    state: EnvironmentVersionState::Staged,
                    revoked: false,
                });
                ("staged", Some(version_id))
            }
            EnvironmentChange::Qualify {
                version_id,
                qualification,
            } => {
                let v = next
                    .versions
                    .iter_mut()
                    .find(|v| v.id == *version_id)
                    .ok_or(Error::NotFound)?;
                if v.revoked
                    || v.state != EnvironmentVersionState::Staged
                    || qualification.verified_at > now
                    || qualification.verified_at < v.created_at
                {
                    return Err(Error::Forbidden);
                }
                v.definition.check_qualification(qualification)?;
                v.state = EnvironmentVersionState::Qualified {
                    qualification: qualification.clone(),
                };
                ("qualified", Some(version_id.as_str()))
            }
            EnvironmentChange::Reject {
                version_id,
                reason_code,
            } => {
                let v = next
                    .versions
                    .iter_mut()
                    .find(|v| v.id == *version_id)
                    .ok_or(Error::NotFound)?;
                if v.revoked || v.state != EnvironmentVersionState::Staged || !key(reason_code) {
                    return Err(Error::Forbidden);
                }
                v.state = EnvironmentVersionState::Rejected {
                    reason_code: reason_code.clone(),
                };
                ("rejected", Some(version_id.as_str()))
            }
            EnvironmentChange::Activate {
                version_id,
                observation,
            } => {
                next.check_observation(version_id, observation, now)?;
                next.active_version_id = Some(version_id.clone());
                ("activated", Some(version_id.as_str()))
            }
            EnvironmentChange::RevokeVersion { version_id } => {
                let v = next
                    .versions
                    .iter_mut()
                    .find(|v| v.id == *version_id)
                    .ok_or(Error::NotFound)?;
                v.revoked = true;
                ("version_revoked", Some(version_id.as_str()))
            }
            EnvironmentChange::Revoke => {
                next.revoked = true;
                ("revoked", None)
            }
        };
        let receipt = EnvironmentReceipt {
            key: request_key.into(),
            request_sha256,
            revision: self.revision + 1,
            kind: kind.into(),
            version_id: affected.map(str::to_owned),
            created_at: now,
        };
        next.revision += 1;
        next.receipts.push(receipt.clone());
        next.validate()?;
        *self = next;
        Ok(receipt)
    }
    fn check_observation(
        &self,
        version: &str,
        observed: &EnvironmentObservation,
        now: i64,
    ) -> Result<&EnvironmentQualification> {
        if self.revoked {
            return Err(Error::Forbidden);
        }
        let v = self.version(version)?;
        let EnvironmentVersionState::Qualified { qualification: q } = &v.state else {
            return Err(Error::Forbidden);
        };
        if v.revoked
            || observed.runner_id != q.runner_id
            || observed.runner_generation != q.runner_generation
            || observed.adapter_build_sha256 != q.adapter_build_sha256
            || observed.artifact_sha256 != q.artifact_sha256
            || observed.definition_sha256 != v.sha256
            || observed.observed_at < q.verified_at
            || observed.observed_at > now
            || now.saturating_sub(observed.observed_at) > 30_000
            || !v
                .definition
                .required_capabilities()
                .is_subset(&observed.capabilities)
        {
            return Err(Error::Forbidden);
        }
        Ok(q)
    }
    pub fn pin(&self, observation: &EnvironmentObservation, now: i64) -> Result<EnvironmentPin> {
        self.validate()?;
        let id = self.active_version_id.as_ref().ok_or(Error::Forbidden)?;
        let q = self.check_observation(id, observation, now)?;
        let v = self.version(id)?;
        Ok(EnvironmentPin {
            environment_id: self.id.clone(),
            project_id: self.project_id.clone(),
            version_id: v.id.clone(),
            definition_sha256: v.sha256.clone(),
            qualification_sha256: canonical_hash(q),
            definition: v.definition.clone(),
        })
    }
    /// A newer active version does not rewrite old pins; revocation and changed runners still deny them.
    pub fn resolve_pin(
        &self,
        pin: &EnvironmentPin,
        observation: &EnvironmentObservation,
        now: i64,
    ) -> Result<()> {
        self.validate()?;
        if pin.environment_id != self.id || pin.project_id != self.project_id {
            return Err(Error::Forbidden);
        }
        let q = self.check_observation(&pin.version_id, observation, now)?;
        let v = self.version(&pin.version_id)?;
        if pin.definition != v.definition
            || pin.definition_sha256 != v.sha256
            || pin.qualification_sha256 != canonical_hash(q)
        {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
}
