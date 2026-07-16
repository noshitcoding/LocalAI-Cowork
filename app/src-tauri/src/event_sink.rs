#![allow(dead_code)]

use crate::context::{RequestContext, TenantId, WorkspaceId};
use crate::service_error::ServiceResult;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventId(String);

impl EventId {
    pub fn new() -> Self {
        Self(format!("event_{}", Uuid::new_v4()))
    }

    pub fn from_static(value: &'static str) -> Self {
        Self(value.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for EventId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventSequence(u64);

impl EventSequence {
    pub fn first() -> Self {
        Self(1)
    }

    pub fn from_u64(value: u64) -> Self {
        Self(value.max(1))
    }

    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }

    pub fn value(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct EventReplayMetadata {
    pub cursor: String,
    pub replayable: bool,
}

impl EventReplayMetadata {
    pub fn new(stream: &str, sequence: EventSequence) -> Self {
        Self {
            cursor: format!("{stream}:{}", sequence.value()),
            replayable: true,
        }
    }

    pub fn non_replayable(stream: &str, sequence: EventSequence) -> Self {
        Self {
            cursor: format!("{stream}:{}", sequence.value()),
            replayable: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct EventEnvelope {
    pub event_id: EventId,
    pub sequence: EventSequence,
    pub stream: String,
    pub tenant_id: Option<TenantId>,
    pub workspace_id: Option<WorkspaceId>,
    pub object_type: Option<String>,
    pub object_id: Option<String>,
    pub event_type: String,
    pub created_at: DateTime<Utc>,
    pub payload: Value,
    pub replay: EventReplayMetadata,
}

impl EventEnvelope {
    pub fn new(
        sequence: EventSequence,
        stream: impl Into<String>,
        event_type: impl Into<String>,
        payload: Value,
    ) -> Self {
        let stream = stream.into();
        Self {
            event_id: EventId::new(),
            sequence,
            stream: stream.clone(),
            tenant_id: None,
            workspace_id: None,
            object_type: None,
            object_id: None,
            event_type: event_type.into(),
            created_at: Utc::now(),
            payload,
            replay: EventReplayMetadata::new(&stream, sequence),
        }
    }

    pub fn scoped(
        context: &RequestContext,
        sequence: EventSequence,
        stream: impl Into<String>,
        event_type: impl Into<String>,
        object_type: impl Into<String>,
        object_id: impl Into<String>,
        payload: Value,
    ) -> Self {
        Self::new(sequence, stream, event_type, payload).with_scope_and_object(
            context.tenant_id.clone(),
            context.workspace_id.clone(),
            object_type,
            object_id,
        )
    }

    pub fn legacy_tauri_event(
        context: &RequestContext,
        sequence: EventSequence,
        event_name: impl Into<String>,
        payload: Value,
    ) -> Self {
        let event_name = event_name.into();
        Self::scoped(
            context,
            sequence,
            "legacy_tauri",
            event_name.clone(),
            "tauri_event",
            event_name,
            payload,
        )
    }

    pub fn with_scope_and_object(
        mut self,
        tenant_id: TenantId,
        workspace_id: WorkspaceId,
        object_type: impl Into<String>,
        object_id: impl Into<String>,
    ) -> Self {
        self.tenant_id = Some(tenant_id);
        self.workspace_id = Some(workspace_id);
        self.object_type = Some(object_type.into());
        self.object_id = Some(object_id.into());
        self
    }

    pub fn with_event_id(mut self, event_id: EventId) -> Self {
        self.event_id = event_id;
        self
    }

    pub fn with_created_at(mut self, created_at: DateTime<Utc>) -> Self {
        self.created_at = created_at;
        self
    }
}

pub trait EventSink: Send + Sync {
    fn emit(&self, event: EventEnvelope) -> ServiceResult<()>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopEventSink;

impl EventSink for NoopEventSink {
    fn emit(&self, _event: EventEnvelope) -> ServiceResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{ActorRole, RequestContext};

    #[test]
    fn event_envelope_serializes_with_contract_fields() {
        let context = RequestContext::test_with_role(ActorRole::Operator);
        let event = EventEnvelope::scoped(
            &context,
            EventSequence::from_u64(42),
            "run",
            "run.output.delta",
            "run",
            "run_1",
            serde_json::json!({ "delta": "hello" }),
        )
        .with_event_id(EventId::from_static("event_1"));
        let value = serde_json::to_value(event).expect("event serializes");

        assert_eq!(value["event_id"], "event_1");
        assert_eq!(value["sequence"], 42);
        assert_eq!(value["stream"], "run");
        assert_eq!(value["tenant_id"], "test-tenant");
        assert_eq!(value["workspace_id"], "test-workspace");
        assert_eq!(value["object_type"], "run");
        assert_eq!(value["object_id"], "run_1");
        assert_eq!(value["event_type"], "run.output.delta");
        assert_eq!(value["payload"]["delta"], "hello");
        assert_eq!(value["replay"]["cursor"], "run:42");
        assert_eq!(value["replay"]["replayable"], true);
    }

    #[test]
    fn sequence_helper_is_monotonic() {
        let first = EventSequence::first();
        let second = first.next();
        let third = second.next();

        assert_eq!(first.value(), 1);
        assert_eq!(second.value(), 2);
        assert_eq!(third.value(), 3);
        assert!(third > second);
    }

    #[test]
    fn legacy_tauri_event_keeps_current_event_name() {
        let context = RequestContext::local_default();
        let event = EventEnvelope::legacy_tauri_event(
            &context,
            EventSequence::first(),
            "exec-command-chunk",
            serde_json::json!({ "chunk": "ok" }),
        );

        assert_eq!(event.stream, "legacy_tauri");
        assert_eq!(event.event_type, "exec-command-chunk");
        assert_eq!(event.object_type.as_deref(), Some("tauri_event"));
        assert_eq!(event.object_id.as_deref(), Some("exec-command-chunk"));
    }

    #[test]
    fn noop_event_sink_accepts_envelopes() {
        let context = RequestContext::anonymous_test();
        let sink = NoopEventSink;
        let event = EventEnvelope::legacy_tauri_event(
            &context,
            EventSequence::first(),
            "crew-execution-log",
            serde_json::json!({}),
        );

        assert_eq!(sink.emit(event), Ok(()));
    }
}
