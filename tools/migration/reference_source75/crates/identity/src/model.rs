use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendKind {
    Sqlite,
    Postgres,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub id: Uuid,
    pub username: String,
    pub display_name: String,
    pub active: bool,
    pub platform_admin: bool,
    pub revision: i64,
}

/// Only a verified persisted session can create this value. It is deliberately
/// neither deserializable nor constructible from a daemon token or user ID.
#[derive(Clone, Debug)]
pub struct Principal {
    pub(crate) identity_id: Uuid,
    pub(crate) session_id: Uuid,
    pub(crate) session_revision: i64,
}
impl Principal {
    pub fn identity_id(&self) -> Uuid {
        self.identity_id
    }
    pub fn session_id(&self) -> Uuid {
        self.session_id
    }
}

pub struct SessionIssued {
    pub token: String,
    pub identity: Identity,
    pub expires_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountSession {
    pub id: Uuid,
    pub current: bool,
    pub expires_at: i64,
    pub revoked: bool,
    pub revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountDirectoryPage {
    pub items: Vec<Identity>,
    pub next_cursor: Option<Uuid>,
    pub has_more: bool,
}
impl std::fmt::Debug for SessionIssued {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionIssued")
            .field("token", &"[redacted]")
            .field("identity", &self.identity)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrganizationState {
    Active,
    Suspended,
    Closed,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrganizationRole {
    Owner,
    Admin,
    Member,
    Viewer,
}
impl OrganizationRole {
    pub(crate) fn manages_members(self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }
    pub(crate) fn allows(self, action: ContentAction) -> bool {
        action == ContentAction::Read || self != Self::Viewer
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Organization {
    pub id: Uuid,
    pub name: String,
    pub state: OrganizationState,
    pub revision: i64,
    pub authorization_epoch: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Membership {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub identity_id: Uuid,
    pub role: OrganizationRole,
    pub active: bool,
    pub revision: i64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Project,
    Order,
    Artifact,
    Workflow,
    Knowledge,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContentAction {
    Read,
    Write,
    Execute,
    Share,
}
impl ContentAction {
    pub(crate) fn bit(self) -> i64 {
        match self {
            Self::Read => 1,
            Self::Write => 2,
            Self::Execute => 4,
            Self::Share => 8,
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct Permissions(i64);
impl Permissions {
    pub fn from_actions(actions: &[ContentAction]) -> Self {
        Self(actions.iter().fold(0, |bits, action| bits | action.bit()))
    }
    pub fn read_only() -> Self {
        Self(1)
    }
    pub fn all() -> Self {
        Self(15)
    }
    pub fn permits(self, action: ContentAction) -> bool {
        self.0 & action.bit() != 0
    }
    pub(crate) fn bits(self) -> i64 {
        self.0
    }
    pub(crate) fn valid(self) -> bool {
        self.0 > 0 && self.0 <= 15 && self.0 & 1 == 1
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContentResource {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub kind: ContentKind,
    pub revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContentGrant {
    pub resource_id: Uuid,
    pub membership_id: Uuid,
    pub permissions: Permissions,
    pub active: bool,
    pub revision: i64,
}
/// Content authority only: not a filesystem/network/tool permission. It must
/// be checked in the same transaction as each tenant data effect, and the
/// existing global policy, approval, executor and budget gates remain required.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct PermitData {
    pub identity_id: Uuid,
    pub session_id: Uuid,
    pub session_revision: i64,
    pub organization_id: Uuid,
    pub authorization_epoch: i64,
    pub membership_id: Uuid,
    pub membership_revision: i64,
    pub resource_id: Uuid,
    pub resource_revision: i64,
    pub grant_revision: i64,
    pub action: ContentAction,
}
#[derive(Clone, Debug)]
pub struct ContentPermit(pub(crate) PermitData);
impl ContentPermit {
    pub fn organization_id(&self) -> Uuid {
        self.0.organization_id
    }
    pub fn resource_id(&self) -> Uuid {
        self.0.resource_id
    }
    pub fn action(&self) -> ContentAction {
        self.0.action
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QuotaAmounts {
    pub model_tokens: i64,
    pub cost_microunits: i64,
    pub storage_bytes: i64,
    pub concurrent_runs: i64,
}
impl QuotaAmounts {
    pub(crate) fn valid(self) -> bool {
        self.values().iter().all(|n| *n >= 0)
    }
    pub(crate) fn values(self) -> [i64; 4] {
        [
            self.model_tokens,
            self.cost_microunits,
            self.storage_bytes,
            self.concurrent_runs,
        ]
    }
    pub(crate) fn from_values(v: [i64; 4]) -> Self {
        Self {
            model_tokens: v[0],
            cost_microunits: v[1],
            storage_bytes: v[2],
            concurrent_runs: v[3],
        }
    }
    pub(crate) fn add(self, other: Self) -> Option<Self> {
        let a = self.values();
        let b = other.values();
        Some(Self::from_values([
            a[0].checked_add(b[0])?,
            a[1].checked_add(b[1])?,
            a[2].checked_add(b[2])?,
            a[3].checked_add(b[3])?,
        ]))
    }
    pub(crate) fn subtract(self, other: Self) -> Option<Self> {
        let a = self.values();
        let b = other.values();
        let value = Self::from_values([
            a[0].checked_sub(b[0])?,
            a[1].checked_sub(b[1])?,
            a[2].checked_sub(b[2])?,
            a[3].checked_sub(b[3])?,
        ]);
        value.valid().then_some(value)
    }
    pub(crate) fn fits(self, limit: Self) -> bool {
        self.values()
            .iter()
            .zip(limit.values())
            .all(|(used, maximum)| *used <= maximum)
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuotaScope {
    Platform,
    Organization(Uuid),
}
impl QuotaScope {
    pub(crate) fn key(self) -> String {
        match self {
            Self::Platform => "platform".into(),
            Self::Organization(id) => id.to_string(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QuotaStatus {
    pub scope: QuotaScope,
    pub revision: i64,
    pub limit: QuotaAmounts,
    pub held: QuotaAmounts,
    pub consumed: QuotaAmounts,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReservationState {
    Prepared,
    Activated,
    Settled,
    Aborted,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QuotaReservation {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub resource_id: Uuid,
    pub identity_id: Uuid,
    pub request_sha256: String,
    pub held: QuotaAmounts,
    pub consumed: Option<QuotaAmounts>,
    pub state: ReservationState,
    pub created_at: i64,
    pub expires_at: i64,
    pub completion_evidence_sha256: Option<String>,
}
#[derive(Clone, Debug)]
pub struct QuotaExecutionPermit {
    pub(crate) reservation: QuotaReservation,
    pub(crate) content: ContentPermit,
}
impl QuotaExecutionPermit {
    pub fn reservation(&self) -> &QuotaReservation {
        &self.reservation
    }
    pub fn content(&self) -> &ContentPermit {
        &self.content
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuditEntry {
    /// Stable global cursor: timestamps can tie, so pagination never uses time.
    pub sequence: i64,
    pub id: Uuid,
    pub organization_id: Option<Uuid>,
    pub actor_id: Uuid,
    pub action: String,
    pub target_id: Uuid,
    pub observed_at: i64,
}
