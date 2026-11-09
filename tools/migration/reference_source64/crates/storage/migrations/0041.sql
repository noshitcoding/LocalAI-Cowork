CREATE TABLE IF NOT EXISTS audit_personal_legacy_events(event_id TEXT PRIMARY KEY);
INSERT INTO audit_personal_legacy_events(event_id) SELECT id FROM events;
