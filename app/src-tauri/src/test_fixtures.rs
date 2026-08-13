#![cfg(test)]
#![allow(dead_code)]

use crate::audit_sink::NoopAuditSink;
use crate::context::{ActorRole, RequestContext};
use crate::db::Database;
use crate::event_sink::NoopEventSink;
use std::sync::Arc;

pub fn local_owner_context() -> RequestContext {
    RequestContext::local_default()
}

pub fn tenant_admin_context() -> RequestContext {
    RequestContext::test_with_role(ActorRole::Admin)
}

pub fn operator_context() -> RequestContext {
    RequestContext::test_with_role(ActorRole::Operator)
}

pub fn viewer_context() -> RequestContext {
    RequestContext::test_with_role(ActorRole::Viewer)
}

pub fn anonymous_context() -> RequestContext {
    RequestContext::anonymous_test()
}

pub fn denied_context() -> RequestContext {
    viewer_context()
}

pub fn noop_audit_sink() -> NoopAuditSink {
    NoopAuditSink
}

pub fn noop_event_sink() -> NoopEventSink {
    NoopEventSink
}

pub fn in_memory_database() -> Database {
    Database::open_in_memory().expect("in-memory database opens")
}

pub fn shared_in_memory_database() -> Arc<Database> {
    Arc::new(in_memory_database())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit_sink::{AuditEvent, AuditRiskClass};
    use crate::audit_sink::{AuditSink, AuditTarget};
    use crate::event_sink::{EventEnvelope, EventSequence, EventSink};

    #[test]
    fn test_context_fixtures_do_not_require_tauri_runtime() {
        let owner = local_owner_context();
        let admin = tenant_admin_context();
        let operator = operator_context();
        let viewer = viewer_context();
        let anonymous = anonymous_context();
        let denied = denied_context();

        assert!(owner.actor.has_role(&ActorRole::Owner));
        assert!(admin.actor.has_role(&ActorRole::Admin));
        assert!(operator.actor.has_role(&ActorRole::Operator));
        assert!(viewer.actor.has_role(&ActorRole::Viewer));
        assert!(anonymous.actor.has_role(&ActorRole::Anonymous));
        assert!(denied.actor.has_role(&ActorRole::Viewer));
    }

    #[test]
    fn noop_sink_fixtures_accept_service_events() {
        let context = tenant_admin_context();
        let audit_event = AuditEvent::success(
            &context,
            "fixture.audit",
            AuditTarget::workspace(&context.workspace_id),
            AuditRiskClass::Low,
        );
        let event = EventEnvelope::legacy_tauri_event(
            &context,
            EventSequence::first(),
            "crew-execution-log",
            serde_json::json!({}),
        );

        assert_eq!(noop_audit_sink().emit(audit_event), Ok(()));
        assert_eq!(noop_event_sink().emit(event), Ok(()));
    }

    #[test]
    fn in_memory_database_fixture_uses_existing_database_path() {
        let db = shared_in_memory_database();

        assert!(db.list_projects().is_ok());
    }
}
