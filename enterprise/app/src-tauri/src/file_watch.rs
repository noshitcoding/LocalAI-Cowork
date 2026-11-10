use chrono::Utc;
use notify::{event::ModifyKind, Event, EventKind};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileWatchEventPayload {
    pub watched_path: String,
    pub event_kind: String,
    pub paths: Vec<String>,
    pub timestamp: String,
}

fn event_kind_label(kind: &EventKind) -> String {
    match kind {
        EventKind::Create(_) => "create".to_string(),
        EventKind::Remove(_) => "remove".to_string(),
        EventKind::Modify(ModifyKind::Name(_)) => "rename".to_string(),
        EventKind::Modify(_) => "modify".to_string(),
        EventKind::Access(_) => "access".to_string(),
        EventKind::Any => "any".to_string(),
        EventKind::Other => "other".to_string(),
    }
}

pub fn to_payload(watched_path: &str, event: &Event) -> FileWatchEventPayload {
    FileWatchEventPayload {
        watched_path: watched_path.to_string(),
        event_kind: event_kind_label(&event.kind),
        paths: event
            .paths
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        timestamp: Utc::now().to_rfc3339(),
    }
}
