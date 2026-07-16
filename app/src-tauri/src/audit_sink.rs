#![allow(dead_code)]

use crate::context::{Actor, RequestContext, RequestId, TenantId, WorkspaceId};
use crate::service_error::ServiceResult;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AuditId(String);

impl AuditId {
    pub fn new() -> Self {
        Self(format!("audit_{}", Uuid::new_v4()))
    }

    pub fn from_static(value: &'static str) -> Self {
        Self(value.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for AuditId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Success,
    Denied,
    Failure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditRiskClass {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AuditTarget {
    pub target_type: String,
    pub target_id: Option<String>,
}

impl AuditTarget {
    pub fn new(target_type: impl Into<String>, target_id: Option<String>) -> Self {
        Self {
            target_type: target_type.into(),
            target_id,
        }
    }

    pub fn workspace(workspace_id: &WorkspaceId) -> Self {
        Self {
            target_type: "workspace".to_string(),
            target_id: Some(workspace_id.as_str().to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AuditEvent {
    pub audit_id: AuditId,
    pub tenant_id: TenantId,
    pub workspace_id: Option<WorkspaceId>,
    pub actor: Actor,
    pub target: AuditTarget,
    pub action: String,
    pub outcome: AuditOutcome,
    pub risk_class: AuditRiskClass,
    pub request_id: RequestId,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, Value>,
}

impl AuditEvent {
    pub fn new(
        context: &RequestContext,
        action: impl Into<String>,
        target: AuditTarget,
        outcome: AuditOutcome,
        risk_class: AuditRiskClass,
    ) -> Self {
        Self {
            audit_id: AuditId::new(),
            tenant_id: context.tenant_id.clone(),
            workspace_id: Some(context.workspace_id.clone()),
            actor: context.actor.clone(),
            target,
            action: action.into(),
            outcome,
            risk_class,
            request_id: context.request_id.clone(),
            created_at: Utc::now(),
            details: BTreeMap::new(),
        }
    }

    pub fn success(
        context: &RequestContext,
        action: impl Into<String>,
        target: AuditTarget,
        risk_class: AuditRiskClass,
    ) -> Self {
        Self::new(context, action, target, AuditOutcome::Success, risk_class)
    }

    pub fn denied(
        context: &RequestContext,
        action: impl Into<String>,
        target: AuditTarget,
        risk_class: AuditRiskClass,
    ) -> Self {
        Self::new(context, action, target, AuditOutcome::Denied, risk_class)
    }

    pub fn with_detail(mut self, key: impl Into<String>, value: Value) -> Self {
        self.details.insert(key.into(), value);
        self
    }
}

pub trait AuditSink: Send + Sync {
    fn emit(&self, event: AuditEvent) -> ServiceResult<()>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopAuditSink;

impl AuditSink for NoopAuditSink {
    fn emit(&self, _event: AuditEvent) -> ServiceResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{ActorRole, RequestContext};

    #[test]
    fn success_event_contains_request_actor_and_scope() {
        let context = RequestContext::test_with_role(ActorRole::Owner);
        let event = AuditEvent::success(
            &context,
            "workspace.update",
            AuditTarget::workspace(&context.workspace_id),
            AuditRiskClass::Medium,
        );

        assert_eq!(event.tenant_id.as_str(), "test-tenant");
        assert_eq!(
            event.workspace_id.as_ref().map(WorkspaceId::as_str),
            Some("test-workspace")
        );
        assert!(event.actor.has_role(&ActorRole::Owner));
        assert_eq!(event.request_id.as_str(), "test-request");
        assert_eq!(event.outcome, AuditOutcome::Success);
    }

    #[test]
    fn denial_event_construction_preserves_high_risk_result() {
        let context = RequestContext::test_with_role(ActorRole::Viewer);
        let event = AuditEvent::denied(
            &context,
            "desktop.input.request",
            AuditTarget::new("desktop_agent", Some("agent_1".to_string())),
            AuditRiskClass::Critical,
        )
        .with_detail("reason", Value::String("missing consent".to_string()));

        assert_eq!(event.outcome, AuditOutcome::Denied);
        assert_eq!(event.risk_class, AuditRiskClass::Critical);
        assert_eq!(
            event.details.get("reason"),
            Some(&Value::String("missing consent".to_string()))
        );
    }

    #[test]
    fn noop_audit_sink_accepts_events() {
        let context = RequestContext::anonymous_test();
        let sink = NoopAuditSink;
        let event = AuditEvent::denied(
            &context,
            "workspace.read",
            AuditTarget::workspace(&context.workspace_id),
            AuditRiskClass::Low,
        );

        assert_eq!(sink.emit(event), Ok(()));
    }

    #[test]
    fn audit_event_serializes_with_snake_case_contract_fields() {
        let context = RequestContext::test_with_role(ActorRole::Admin);
        let event = AuditEvent::success(
            &context,
            "sync.import",
            AuditTarget::new("sync_bundle", Some("sync_1".to_string())),
            AuditRiskClass::High,
        );
        let value = serde_json::to_value(event).expect("audit event serializes");

        assert_eq!(value["tenant_id"], "test-tenant");
        assert_eq!(value["workspace_id"], "test-workspace");
        assert_eq!(value["request_id"], "test-request");
        assert_eq!(value["outcome"], "success");
        assert_eq!(value["risk_class"], "high");
        assert_eq!(value["target"]["target_type"], "sync_bundle");
    }
}
