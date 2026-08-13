#![allow(dead_code)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const LOCAL_TENANT_ID: &str = "local";
pub const LOCAL_WORKSPACE_ID: &str = "local";
pub const LOCAL_ACTOR_ID: &str = "local-user";

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(String);

impl RequestId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    pub fn from_static(value: &'static str) -> Self {
        Self(value.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for RequestId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TenantId(String);

impl TenantId {
    pub fn local() -> Self {
        Self(LOCAL_TENANT_ID.to_string())
    }

    pub fn from_static(value: &'static str) -> Self {
        Self(value.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkspaceId(String);

impl WorkspaceId {
    pub fn local() -> Self {
        Self(LOCAL_WORKSPACE_ID.to_string())
    }

    pub fn from_static(value: &'static str) -> Self {
        Self(value.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ServerId(String);

impl ServerId {
    pub fn from_static(value: &'static str) -> Self {
        Self(value.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ActorId(String);

impl ActorId {
    pub fn local() -> Self {
        Self(LOCAL_ACTOR_ID.to_string())
    }

    pub fn from_static(value: &'static str) -> Self {
        Self(value.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ActorKind {
    LocalUser,
    ServiceAccount,
    Anonymous,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ActorRole {
    Owner,
    Admin,
    Operator,
    Viewer,
    Anonymous,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RequestOrigin {
    LocalDesktop,
    LocalSidecar,
    RemoteServer,
    Browser,
    Mobile,
    DesktopAgent,
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClientPlatform {
    TauriDesktop,
    Browser,
    Mobile,
    ServerWorker,
    DesktopAgent,
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Actor {
    pub kind: ActorKind,
    pub id: Option<ActorId>,
    pub display_name: Option<String>,
    pub roles: Vec<ActorRole>,
}

impl Actor {
    pub fn local_owner() -> Self {
        Self {
            kind: ActorKind::LocalUser,
            id: Some(ActorId::local()),
            display_name: Some("Local user".to_string()),
            roles: vec![ActorRole::Owner],
        }
    }

    pub fn anonymous() -> Self {
        Self {
            kind: ActorKind::Anonymous,
            id: None,
            display_name: None,
            roles: vec![ActorRole::Anonymous],
        }
    }

    pub fn test_with_role(role: ActorRole) -> Self {
        if role == ActorRole::Anonymous {
            return Self::anonymous();
        }

        let id = match role {
            ActorRole::Owner => "test-owner",
            ActorRole::Admin => "test-admin",
            ActorRole::Operator => "test-operator",
            ActorRole::Viewer => "test-viewer",
            ActorRole::Anonymous => unreachable!("anonymous role returns early"),
        };

        Self {
            kind: ActorKind::LocalUser,
            id: Some(ActorId::from_static(id)),
            display_name: Some(id.to_string()),
            roles: vec![role],
        }
    }

    pub fn has_role(&self, role: &ActorRole) -> bool {
        self.roles.iter().any(|candidate| candidate == role)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestContext {
    pub request_id: RequestId,
    pub tenant_id: TenantId,
    pub workspace_id: WorkspaceId,
    pub server_id: Option<ServerId>,
    pub origin: RequestOrigin,
    pub platform: ClientPlatform,
    pub actor: Actor,
    pub created_at: DateTime<Utc>,
}

impl RequestContext {
    pub fn local_default() -> Self {
        Self {
            request_id: RequestId::new(),
            tenant_id: TenantId::local(),
            workspace_id: WorkspaceId::local(),
            server_id: None,
            origin: RequestOrigin::LocalDesktop,
            platform: ClientPlatform::TauriDesktop,
            actor: Actor::local_owner(),
            created_at: Utc::now(),
        }
    }

    pub fn test_with_role(role: ActorRole) -> Self {
        Self {
            request_id: RequestId::from_static("test-request"),
            tenant_id: TenantId::from_static("test-tenant"),
            workspace_id: WorkspaceId::from_static("test-workspace"),
            server_id: Some(ServerId::from_static("test-server")),
            origin: RequestOrigin::Test,
            platform: ClientPlatform::Test,
            actor: Actor::test_with_role(role),
            created_at: DateTime::<Utc>::from_timestamp(0, 0).expect("valid Unix epoch"),
        }
    }

    pub fn anonymous_test() -> Self {
        Self {
            request_id: RequestId::from_static("test-anonymous-request"),
            tenant_id: TenantId::from_static("test-tenant"),
            workspace_id: WorkspaceId::from_static("test-workspace"),
            server_id: Some(ServerId::from_static("test-server")),
            origin: RequestOrigin::Test,
            platform: ClientPlatform::Test,
            actor: Actor::anonymous(),
            created_at: DateTime::<Utc>::from_timestamp(0, 0).expect("valid Unix epoch"),
        }
    }

    pub fn is_local_default_scope(&self) -> bool {
        self.tenant_id.as_str() == LOCAL_TENANT_ID
            && self.workspace_id.as_str() == LOCAL_WORKSPACE_ID
            && self.server_id.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_default_context_needs_no_network_config() {
        let context = RequestContext::local_default();

        assert!(context.is_local_default_scope());
        assert_eq!(context.origin, RequestOrigin::LocalDesktop);
        assert_eq!(context.platform, ClientPlatform::TauriDesktop);
        assert!(context.actor.has_role(&ActorRole::Owner));
        assert_eq!(
            context.actor.id.as_ref().map(ActorId::as_str),
            Some(LOCAL_ACTOR_ID)
        );
        assert!(!context.request_id.as_str().is_empty());
    }

    #[test]
    fn test_fixtures_cover_expected_roles() {
        let cases = [
            ActorRole::Owner,
            ActorRole::Admin,
            ActorRole::Operator,
            ActorRole::Viewer,
            ActorRole::Anonymous,
        ];

        for role in cases {
            let context = RequestContext::test_with_role(role.clone());
            assert_eq!(context.origin, RequestOrigin::Test);
            assert_eq!(context.platform, ClientPlatform::Test);
            assert!(context.actor.has_role(&role));
            assert_eq!(context.tenant_id.as_str(), "test-tenant");
            assert_eq!(context.workspace_id.as_str(), "test-workspace");
        }
    }

    #[test]
    fn anonymous_fixture_has_no_actor_id() {
        let context = RequestContext::anonymous_test();

        assert_eq!(context.actor.kind, ActorKind::Anonymous);
        assert_eq!(context.actor.id, None);
        assert!(context.actor.has_role(&ActorRole::Anonymous));
    }

    #[test]
    fn context_serializes_with_camel_case_fields() {
        let context = RequestContext::test_with_role(ActorRole::Admin);
        let value = serde_json::to_value(context).expect("context serializes");

        assert_eq!(value["requestId"], "test-request");
        assert_eq!(value["tenantId"], "test-tenant");
        assert_eq!(value["workspaceId"], "test-workspace");
        assert_eq!(value["origin"], "test");
        assert_eq!(value["platform"], "test");
        assert_eq!(value["actor"]["roles"][0], "admin");
    }
}
