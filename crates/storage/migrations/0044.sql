-- V1 operation receipts are scoped to the real Android session identity
CREATE TABLE android_wire_session_devices (session_id TEXT PRIMARY KEY REFERENCES identity_sessions(id), owner_id TEXT NOT NULL REFERENCES identity_subjects(id), device_id TEXT NOT NULL);
CREATE TABLE android_wire_sync_clock (id BIGINT PRIMARY KEY CHECK(id=1), sequence BIGINT NOT NULL);
INSERT INTO android_wire_sync_clock(id,sequence) SELECT 1,sequence FROM team_sync_clock WHERE id=1;
CREATE TABLE android_wire_sync_current (owner_id TEXT NOT NULL REFERENCES identity_subjects(id), entity_type TEXT NOT NULL, entity_id TEXT NOT NULL, team_id TEXT, project_id TEXT, revision BIGINT NOT NULL, cursor BIGINT NOT NULL, digest TEXT NOT NULL, payload TEXT NOT NULL, tombstone BIGINT NOT NULL CHECK(tombstone IN(0,1)), PRIMARY KEY(owner_id,entity_type,entity_id));
CREATE TABLE android_wire_sync_changes (cursor BIGINT PRIMARY KEY, owner_id TEXT NOT NULL REFERENCES identity_subjects(id), entity_type TEXT NOT NULL, entity_id TEXT NOT NULL, team_id TEXT, project_id TEXT, revision BIGINT NOT NULL, payload TEXT NOT NULL, tombstone BIGINT NOT NULL CHECK(tombstone IN(0,1)), created_at BIGINT NOT NULL);
CREATE TABLE android_wire_sync_receipts (owner_id TEXT NOT NULL REFERENCES identity_subjects(id), operation_id TEXT NOT NULL, request_sha256 TEXT NOT NULL, receipt TEXT NOT NULL, PRIMARY KEY(owner_id,operation_id));
CREATE INDEX android_wire_sync_owner_cursor ON android_wire_sync_changes(owner_id,cursor);
CREATE TABLE android_personal_run_intents (owner_id TEXT NOT NULL REFERENCES identity_subjects(id), creation_key TEXT NOT NULL, project_id TEXT NOT NULL REFERENCES android_personal_projects(id), request_sha256 TEXT NOT NULL, frozen TEXT NOT NULL, PRIMARY KEY(owner_id,creation_key));
