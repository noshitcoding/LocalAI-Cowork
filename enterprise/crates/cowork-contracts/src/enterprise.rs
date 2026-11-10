//! Enterprise contracts. These do not widen the legacy per-instance API.
use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupportAdministrator {
    pub user_id: Uuid,
    pub display_name: String,
    pub email: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InstanceContentAccess {
    pub managed: bool,
    pub organisation_id: Option<Uuid>,
    pub instance_id: Option<Uuid>,
    pub policy_revision: Option<u64>,
    pub content_access: Option<ContentAccess>,
    pub read_only: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub instance_viewer: bool,
    pub support_administrator: bool,
    pub automatic_content_access: bool,
    pub administrators: Vec<SupportAdministrator>,
}

impl InstanceContentAccess {
    pub fn local_membership_allowed(&self) -> bool {
        !self.read_only || self.instance_viewer
    }
}
fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceMemberRecord {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub organisation_role: OrganisationMembershipRole,
}

pub const MANAGEMENT_SCHEMA_VERSION: u16 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrganisationRole {
    Admin,
    Member,
    Auditor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrganisationMembershipRole {
    Admin,
    Member,
    Auditor,
    Suspended,
}
impl From<OrganisationRole> for OrganisationMembershipRole {
    fn from(role: OrganisationRole) -> Self {
        match role {
            OrganisationRole::Admin => Self::Admin,
            OrganisationRole::Member => Self::Member,
            OrganisationRole::Auditor => Self::Auditor,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SsoGroupMapping {
    pub group: String,
    pub role: OrganisationRole,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrganisationSsoProvider {
    pub id: String,
    pub label: String,
    pub issuer: String,
    pub groups_claim: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrganisationSsoSettings {
    pub organisation_id: Uuid,
    pub revision: u64,
    pub enabled: bool,
    pub available: bool,
    pub provider_id: Option<String>,
    pub auto_provision: bool,
    pub group_mappings: Vec<SsoGroupMapping>,
    pub providers: Vec<OrganisationSsoProvider>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateOrganisationSso {
    pub expected_revision: u64,
    pub enabled: bool,
    pub provider_id: Option<String>,
    pub auto_provision: bool,
    pub group_mappings: Vec<SsoGroupMapping>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrganisationSsoPublic {
    pub enabled: bool,
    pub organisation_id: Uuid,
    pub organisation_name: String,
    pub provider_label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateOrganisationInvitation {
    pub email: String,
    pub role: OrganisationRole,
    pub expires_at: Option<DateTime<Utc>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrganisationInvitationSecret {
    pub id: Uuid,
    pub organisation_id: Uuid,
    pub email: String,
    pub role: OrganisationRole,
    pub token: String,
    pub expires_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrganisationInvitation {
    pub id: Uuid,
    pub organisation_id: Uuid,
    pub email: String,
    pub role: OrganisationRole,
    pub invited_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub state: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrganisationInvitationToken {
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthenticationAssurance {
    pub required: bool,
    pub totp_enabled: bool,
    pub passkey_registered: bool,
    pub session_verified: bool,
    pub enrollment_allowed: bool,
    pub reauthentication_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_verified: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_verification_expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oidc_reauthentication_available: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oidc_organisation_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organisation_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organisation_policy_revision: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_expires_at: Option<DateTime<Utc>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrganisationInvitationPreview {
    pub id: Uuid,
    pub organisation_id: Uuid,
    pub organisation_name: String,
    pub email: String,
    pub role: OrganisationRole,
    pub expires_at: DateTime<Utc>,
    pub account_exists: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrganisationInvitationAccepted {
    pub organisation_id: Uuid,
    pub user_id: Uuid,
    pub role: OrganisationRole,
    pub membership_created: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterOrganisationInvitation {
    pub token: String,
    pub display_name: String,
    pub password: String,
    pub device_id: Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionPolicy {
    Catalog,
    ApprovalRequired,
    Unrestricted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentAccess {
    SupportGrant,
    OrganisationAdmin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Isolation {
    Container,
    VirtualMachine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentBackend {
    Compose,
    Kubernetes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentBackend {
    Docker,
    Kubernetes,
    Libvirt,
    Kubevirt,
    HyperV,
    OpenShell,
    Attached,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResourceLimits {
    pub cpu_millis: u32,
    pub memory_mib: u32,
    pub storage_gib: u32,
    pub concurrent_runs: u32,
    pub instances: u32,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            cpu_millis: 4000,
            memory_mib: 8192,
            storage_gib: 50,
            concurrent_runs: 2,
            instances: 3,
        }
    }
}

impl ResourceLimits {
    pub fn validate(&self) -> Result<(), String> {
        if self.cpu_millis < 100
            || self.memory_mib < 128
            || self.storage_gib == 0
            || self.concurrent_runs == 0
            || self.instances == 0
        {
            return Err(
                "resource limits must be positive; minimum CPU is 100m and memory 128 MiB".into(),
            );
        }
        Ok(())
    }
    pub fn fits(&self, ceiling: &Self) -> bool {
        self.cpu_millis <= ceiling.cpu_millis
            && self.memory_mib <= ceiling.memory_mib
            && self.storage_gib <= ceiling.storage_gib
            && self.concurrent_runs <= ceiling.concurrent_runs
            && self.instances <= ceiling.instances
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct PolicyRevision {
    pub revision: u64,
    pub extensions: ExtensionPolicy,
    pub content_access: ContentAccess,
    pub minimum_isolation: Isolation,
    pub allow_laptop_full_access: bool,
    pub allow_offline_continuation: bool,
    pub allowed_network_origins: BTreeSet<String>,
    pub limits: ResourceLimits,
    pub audit_retention_days: u32,
    pub content_logging: bool,
}

impl Default for PolicyRevision {
    fn default() -> Self {
        Self {
            revision: 1,
            extensions: ExtensionPolicy::Catalog,
            content_access: ContentAccess::SupportGrant,
            minimum_isolation: Isolation::Container,
            allow_laptop_full_access: false,
            allow_offline_continuation: true,
            allowed_network_origins: BTreeSet::new(),
            limits: ResourceLimits::default(),
            audit_retention_days: 365,
            content_logging: false,
        }
    }
}

impl PolicyRevision {
    pub fn validate(&self) -> Result<(), String> {
        self.limits.validate()?;
        if self.revision == 0 || !(1..=3650).contains(&self.audit_retention_days) {
            return Err("invalid policy revision or audit retention".into());
        }
        for origin in &self.allowed_network_origins {
            let host = origin
                .strip_prefix("https://")
                .ok_or("network grants must be HTTPS origins")?;
            let host = host.strip_suffix(":443").unwrap_or(host);
            if host.is_empty()
                || host.len() > 253
                || !host
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b".-".contains(&b))
                || host.split('.').any(|label| {
                    label.is_empty()
                        || label.len() > 63
                        || label.starts_with('-')
                        || label.ends_with('-')
                })
            {
                return Err("network grants must name an exact HTTPS origin".into());
            }
        }
        Ok(())
    }

    /// A lower scope may restrict but cannot widen the parent's policy.
    pub fn narrows(&self, parent: &Self) -> bool {
        let rank = |p| match p {
            ExtensionPolicy::Catalog => 0,
            ExtensionPolicy::ApprovalRequired => 1,
            ExtensionPolicy::Unrestricted => 2,
        };
        self.minimum_isolation >= parent.minimum_isolation
            && (!self.allow_laptop_full_access || parent.allow_laptop_full_access)
            && (!self.allow_offline_continuation || parent.allow_offline_continuation)
            && self
                .allowed_network_origins
                .is_subset(&parent.allowed_network_origins)
            && self.limits.fits(&parent.limits)
            && rank(self.extensions) <= rank(parent.extensions)
            && (self.content_access == ContentAccess::SupportGrant
                || parent.content_access == ContentAccess::OrganisationAdmin)
            && (!self.content_logging || parent.content_logging)
    }

    /// Intersection used when an ancestor is tightened; widening an ancestor
    /// never grants additional rights to an existing descendant.
    pub fn restricted_by(&self, parent: &Self) -> Self {
        let mut value = self.clone();
        value.minimum_isolation = value.minimum_isolation.max(parent.minimum_isolation);
        value.allow_laptop_full_access &= parent.allow_laptop_full_access;
        value.allow_offline_continuation &= parent.allow_offline_continuation;
        value.allowed_network_origins = value
            .allowed_network_origins
            .intersection(&parent.allowed_network_origins)
            .cloned()
            .collect();
        value.limits.cpu_millis = value.limits.cpu_millis.min(parent.limits.cpu_millis);
        value.limits.memory_mib = value.limits.memory_mib.min(parent.limits.memory_mib);
        value.limits.storage_gib = value.limits.storage_gib.min(parent.limits.storage_gib);
        value.limits.concurrent_runs = value
            .limits
            .concurrent_runs
            .min(parent.limits.concurrent_runs);
        value.limits.instances = value.limits.instances.min(parent.limits.instances);
        value.extensions = match (value.extensions, parent.extensions) {
            (ExtensionPolicy::Catalog, _) | (_, ExtensionPolicy::Catalog) => {
                ExtensionPolicy::Catalog
            }
            (ExtensionPolicy::ApprovalRequired, _) | (_, ExtensionPolicy::ApprovalRequired) => {
                ExtensionPolicy::ApprovalRequired
            }
            _ => ExtensionPolicy::Unrestricted,
        };
        if parent.content_access == ContentAccess::SupportGrant {
            value.content_access = ContentAccess::SupportGrant;
        }
        value.content_logging &= parent.content_logging;
        value
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Organisation {
    pub id: Uuid,
    pub name: String,
    pub policy: PolicyRevision,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceState {
    Pending,
    Provisioning,
    Ready,
    Stopped,
    Suspended,
    Deleting,
    Deleted,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceAction {
    Provision,
    Start,
    Stop,
    Suspend,
    Resume,
    Backup,
    Restore,
    Upgrade,
    Rollback,
    EnrollExecution,
    Delete,
}

impl InstanceState {
    pub fn accepts(self, action: InstanceAction) -> bool {
        use InstanceAction as A;
        use InstanceState as S;
        matches!(
            (self, action),
            (S::Pending, A::Provision)
                | (S::Failed, A::Provision)
                | (S::Stopped, A::Start)
                | (S::Ready, A::Stop)
                | (S::Ready | S::Stopped, A::Suspend)
                | (S::Suspended, A::Resume)
                | (S::Ready | S::Stopped, A::Backup | A::Upgrade)
                | (S::Stopped, A::Rollback)
                | (S::Stopped, A::EnrollExecution)
                | (S::Stopped | S::Failed, A::Restore)
                | (
                    S::Ready | S::Stopped | S::Suspended | S::Failed | S::Pending,
                    A::Delete
                )
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    pub id: Uuid,
    pub organisation_id: Uuid,
    pub owner_user_id: Uuid,
    pub name: String,
    pub backend: DeploymentBackend,
    pub state: InstanceState,
    pub policy: PolicyRevision,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub current_release: Option<String>,
    #[serde(default)]
    pub provisioning_release: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateInstance {
    pub name: String,
    pub backend: DeploymentBackend,
    pub policy: Option<PolicyRevision>,
    pub release: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvisioningRelease {
    pub id: String,
    pub name: String,
}

/// An operator-approved transition for the requested instance; no credentials.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceRelease {
    pub id: String,
    pub name: String,
    pub source_release: String,
    pub qualified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceRollback {
    pub operation_id: Uuid,
    pub backup_id: Uuid,
    pub source_release: String,
    pub release: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RolloutSelection {
    pub instance_id: Uuid,
    pub expected_revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRollout {
    pub name: String,
    pub release: String,
    pub batch_size: i32,
    pub targets: Vec<RolloutSelection>,
    pub idempotency_key: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RolloutControl {
    Pause,
    Resume,
    Cancel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlRollout {
    pub action: RolloutControl,
    pub expected_revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RolloutState {
    Running,
    Paused,
    Canceling,
    Canceled,
    Succeeded,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RolloutTargetState {
    Waiting,
    Active,
    Succeeded,
    Failed,
    Canceled,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolloutTarget {
    pub organisation_id: Uuid,
    pub rollout_id: Uuid,
    pub instance_id: Uuid,
    pub instance_name: String,
    pub ordinal: i32,
    pub source_release: String,
    pub initial_state: InstanceState,
    pub expected_revision: i64,
    pub state: RolloutTargetState,
    pub next_action: Option<InstanceAction>,
    pub operation_id: Option<Uuid>,
    pub error_code: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rollout {
    pub id: Uuid,
    pub organisation_id: Uuid,
    pub actor_user_id: Uuid,
    pub name: String,
    pub release: String,
    pub batch_size: i32,
    pub state: RolloutState,
    pub error_code: Option<String>,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub targets: Vec<RolloutTarget>,
}

/// Metadata only; archive paths, contents and recovery keys stay with the operator.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceBackup {
    pub id: Uuid,
    pub organisation_id: Uuid,
    pub instance_id: Uuid,
    pub operation_id: Uuid,
    pub sha256: String,
    pub size_bytes: u64,
    pub manifest_version: u32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationRecoveryMode {
    StopIfUnproven,
    ContinueDeletion,
    ContinueEnrollment,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationRecoveryRequest {
    pub mode: OperationRecoveryMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmation_name: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub purge_backups: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentSpec {
    pub instance_id: Uuid,
    pub name: String,
    pub backend: EnvironmentBackend,
    pub isolation: Isolation,
    pub template_id: Uuid,
    pub template_revision: u64,
    pub persistent: bool,
    pub shared_workspace: bool,
    pub limits: ResourceLimits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaptopAccess {
    None,
    Selected,
    Sandbox,
    Full,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatExecutionPolicy {
    pub revision: u64,
    pub environments: BTreeSet<Uuid>,
    pub device_grants: BTreeSet<Uuid>,
    pub transfer_grants: BTreeSet<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceGrant {
    pub id: Uuid,
    pub organisation_id: Uuid,
    pub instance_id: Uuid,
    pub user_id: Uuid,
    pub device_id: Uuid,
    pub chat_id: Uuid,
    pub access: LaptopAccess,
    pub read_roots: Vec<String>,
    pub write_roots: Vec<String>,
    pub capabilities: BTreeSet<String>,
    pub policy_revision: u64,
    pub locally_confirmed_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExecutionDestination {
    Environment { id: Uuid },
    Device { id: Uuid, grant_id: Uuid },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferGrant {
    pub id: Uuid,
    pub organisation_id: Uuid,
    pub instance_id: Uuid,
    pub chat_id: Uuid,
    pub source: ExecutionDestination,
    pub destination: ExecutionDestination,
    pub relative_paths: Vec<String>,
    pub writable: bool,
    pub max_bytes: u64,
}

pub fn safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with(['/', '\\'])
        && !path.contains([':', '\\', '\0'])
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    Pending,
    Ready,
    Running,
    OfflineRunning,
    WaitingInput,
    Completed,
    Interrupted,
    Failed,
    Canceled,
}

impl StepState {
    pub fn can_reassign(self) -> bool {
        matches!(self, Self::Pending | Self::Ready)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionStep {
    pub id: Uuid,
    pub destination: ExecutionDestination,
    pub depends_on: BTreeSet<Uuid>,
    pub state: StepState,
    pub idempotency_key: String,
    pub required_capabilities: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRun {
    pub id: Uuid,
    pub organisation_id: Uuid,
    pub instance_id: Uuid,
    pub chat_id: Uuid,
    pub policy: ChatExecutionPolicy,
    pub steps: Vec<ExecutionStep>,
}

impl WorkflowRun {
    pub fn validate(&self) -> Result<(), String> {
        if self.steps.is_empty() || self.steps.len() > 256 {
            return Err("a workflow needs 1 to 256 steps".into());
        }
        let nodes: BTreeMap<_, _> = self.steps.iter().map(|step| (step.id, step)).collect();
        if nodes.len() != self.steps.len() {
            return Err("duplicate step ID".into());
        }
        let mut keys = BTreeSet::new();
        for step in &self.steps {
            if step.idempotency_key.is_empty() || !keys.insert(&step.idempotency_key) {
                return Err("step idempotency keys must be nonempty and unique".into());
            }
            if step.state != StepState::Pending {
                return Err("new steps must be pending".into());
            }
            match step.destination {
                ExecutionDestination::Environment { id }
                    if !self.policy.environments.contains(&id) =>
                {
                    return Err("environment not allowed by chat policy".into())
                }
                ExecutionDestination::Device { grant_id, .. }
                    if !self.policy.device_grants.contains(&grant_id) =>
                {
                    return Err("device grant not allowed by chat policy".into())
                }
                _ => {}
            }
            if step.depends_on.iter().any(|id| !nodes.contains_key(id)) {
                return Err("unknown dependency".into());
            }
        }
        let mut visited = BTreeSet::new();
        loop {
            let before = visited.len();
            for step in &self.steps {
                if step.depends_on.is_subset(&visited) {
                    visited.insert(step.id);
                }
            }
            if visited.len() == self.steps.len() {
                return Ok(());
            }
            if visited.len() == before {
                return Err("cyclic workflow".into());
            }
        }
    }

    pub fn runnable_steps(&self) -> Vec<Uuid> {
        let completed = self
            .steps
            .iter()
            .filter(|s| s.state == StepState::Completed)
            .map(|s| s.id)
            .collect();
        self.steps
            .iter()
            .filter(|s| s.state == StepState::Pending && s.depends_on.is_subset(&completed))
            .map(|s| s.id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn policy_intersection_never_widens_either_scope() {
        let mut child = PolicyRevision {
            extensions: ExtensionPolicy::Unrestricted,
            allow_laptop_full_access: true,
            ..Default::default()
        };
        child.allowed_network_origins =
            ["https://one.example".into(), "https://two.example".into()].into();
        let mut parent = PolicyRevision {
            minimum_isolation: Isolation::VirtualMachine,
            ..Default::default()
        };
        parent.allowed_network_origins = ["https://two.example".into()].into();
        let result = child.restricted_by(&parent);
        assert!(result.narrows(&child));
        assert!(result.narrows(&parent));
        assert_eq!(
            result.allowed_network_origins,
            parent.allowed_network_origins
        );
        assert_eq!(result.extensions, ExtensionPolicy::Catalog);
    }
    #[test]
    fn network_grants_reject_proxy_directives_and_ambiguous_origins() {
        for origin in [
            "http://example.com",
            "https://*.example.com",
            "https://a\nhttp_access allow all",
            "https://a:8443",
            "https://a.",
            "https://a..b",
            "https://a/path",
            "https://[::1]",
        ] {
            let policy = PolicyRevision {
                allowed_network_origins: [origin.into()].into(),
                ..Default::default()
            };
            assert!(policy.validate().is_err(), "{origin}");
        }
    }
    #[test]
    fn child_policy_cannot_weaken_vm_or_enable_laptop() {
        let parent = PolicyRevision {
            minimum_isolation: Isolation::VirtualMachine,
            ..Default::default()
        };
        assert!(!PolicyRevision::default().narrows(&parent));
        let mut child = parent.clone();
        child.allow_laptop_full_access = true;
        assert!(!child.narrows(&parent));
        child.allow_laptop_full_access = false;
        child.limits.memory_mib /= 2;
        assert!(child.narrows(&parent));
    }
    #[test]
    fn path_grants_reject_platform_escapes() {
        for path in [
            "../secret",
            "/etc/passwd",
            "C:/Windows",
            "a\\b",
            "a/../b",
            "a//b",
            "a\0b",
        ] {
            assert!(!safe_relative_path(path), "{path}");
        }
        assert!(safe_relative_path("reports/result.docx"));
    }
    #[test]
    fn offline_steps_never_reassign() {
        assert!(!StepState::OfflineRunning.can_reassign());
        assert!(!StepState::Interrupted.can_reassign());
    }
    #[test]
    fn lifecycle_rejects_upgrade_during_provisioning_and_start_after_delete() {
        assert!(!InstanceState::Provisioning.accepts(InstanceAction::Upgrade));
        assert!(!InstanceState::Deleted.accepts(InstanceAction::Start));
        assert!(InstanceState::Stopped.accepts(InstanceAction::Restore));
    }
    #[test]
    fn workflows_detect_cycles_and_wait_for_offline_dependencies() {
        let env = Uuid::new_v4();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let step = |id, deps| ExecutionStep {
            id,
            destination: ExecutionDestination::Environment { id: env },
            depends_on: deps,
            state: StepState::Pending,
            idempotency_key: id.to_string(),
            required_capabilities: BTreeSet::new(),
        };
        let mut workflow = WorkflowRun {
            id: Uuid::new_v4(),
            organisation_id: Uuid::new_v4(),
            instance_id: Uuid::new_v4(),
            chat_id: Uuid::new_v4(),
            policy: ChatExecutionPolicy {
                revision: 1,
                environments: [env].into(),
                device_grants: BTreeSet::new(),
                transfer_grants: BTreeSet::new(),
            },
            steps: vec![step(a, BTreeSet::new()), step(b, [a].into())],
        };
        assert!(workflow.validate().is_ok());
        assert_eq!(workflow.runnable_steps(), vec![a]);
        workflow.steps[0].depends_on.insert(b);
        assert!(workflow.validate().is_err());
        workflow.steps[0].depends_on.clear();
        workflow.steps[0].state = StepState::OfflineRunning;
        assert!(workflow.runnable_steps().is_empty());
        workflow.steps[0].state = StepState::Completed;
        assert_eq!(workflow.runnable_steps(), vec![b]);
    }
}
