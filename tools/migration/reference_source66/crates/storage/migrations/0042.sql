-- Personal Android content is owned by an authenticated identity, never by an invented team
CREATE TABLE android_personal_projects (id TEXT PRIMARY KEY, owner_id TEXT NOT NULL REFERENCES identity_subjects(id), revision BIGINT NOT NULL, payload TEXT NOT NULL, deleted BIGINT NOT NULL CHECK(deleted IN(0,1)));
CREATE TABLE android_personal_threads (id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES android_personal_projects(id), owner_id TEXT NOT NULL REFERENCES identity_subjects(id), revision BIGINT NOT NULL, payload TEXT NOT NULL, deleted BIGINT NOT NULL CHECK(deleted IN(0,1)), creation_key TEXT, request_sha256 TEXT, UNIQUE(owner_id,creation_key));
CREATE TABLE android_personal_operations (owner_id TEXT NOT NULL REFERENCES identity_subjects(id), operation_key TEXT NOT NULL, project_id TEXT NOT NULL REFERENCES android_personal_projects(id), request_sha256 TEXT NOT NULL, receipt TEXT NOT NULL, PRIMARY KEY(owner_id,operation_key));
CREATE TABLE android_task_versions (id TEXT NOT NULL, revision BIGINT NOT NULL, project_id TEXT NOT NULL, payload TEXT NOT NULL, released BIGINT NOT NULL CHECK(released IN(0,1)), deleted BIGINT NOT NULL CHECK(deleted IN(0,1)), PRIMARY KEY(id,revision));
CREATE TABLE android_team_run_tasks (run_id TEXT PRIMARY KEY REFERENCES team_runs(id), task_id TEXT NOT NULL, task_revision BIGINT NOT NULL, payload TEXT NOT NULL);
-- Separate versioned execution tables retain every old row and receipt without rewriting migration checksums
CREATE TABLE android_scoped_runs (id TEXT PRIMARY KEY, team_id TEXT REFERENCES identity_organizations(id), project_id TEXT NOT NULL, order_id TEXT NOT NULL, creator_id TEXT NOT NULL REFERENCES identity_subjects(id), creator_session_id TEXT NOT NULL REFERENCES identity_sessions(id), creator_session_revision BIGINT NOT NULL, authorization_epoch BIGINT NOT NULL, project_revision BIGINT NOT NULL, executor_id TEXT NOT NULL REFERENCES android_devices(id), executor_revision BIGINT NOT NULL, creation_key TEXT NOT NULL, request_sha256 TEXT NOT NULL, payload TEXT NOT NULL, state TEXT NOT NULL, revision BIGINT NOT NULL, lease_token TEXT, credential_id TEXT, credential_revision BIGINT, lease_expires_at BIGINT, completion_sha256 TEXT, created_at BIGINT NOT NULL, updated_at BIGINT NOT NULL, UNIQUE(creator_id,creation_key));
INSERT INTO android_scoped_runs SELECT * FROM android_device_runs;
CREATE TABLE android_scoped_events (run_id TEXT NOT NULL REFERENCES android_scoped_runs(id), sequence BIGINT NOT NULL, event_id TEXT NOT NULL UNIQUE, source_event_id TEXT, request_sha256 TEXT NOT NULL, payload TEXT NOT NULL, PRIMARY KEY(run_id,sequence), UNIQUE(run_id,source_event_id));
INSERT INTO android_scoped_events SELECT * FROM android_device_events;
CREATE TABLE android_scoped_artifacts (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES android_scoped_runs(id), payload TEXT NOT NULL, bytes_base64 TEXT NOT NULL);
INSERT INTO android_scoped_artifacts SELECT * FROM android_device_artifacts;
CREATE TABLE android_scoped_desktop_sessions (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES android_scoped_runs(id), executor_id TEXT NOT NULL REFERENCES android_devices(id), credential_id TEXT NOT NULL REFERENCES android_device_credentials(id), credential_revision BIGINT NOT NULL, lease_token TEXT NOT NULL, revision BIGINT NOT NULL, state TEXT NOT NULL, payload TEXT NOT NULL, controller_generation TEXT, created_at BIGINT NOT NULL, ended_at BIGINT);
INSERT INTO android_scoped_desktop_sessions SELECT * FROM android_desktop_sessions;
CREATE TABLE android_scoped_snapshots (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES android_scoped_runs(id), payload TEXT NOT NULL, committed BIGINT NOT NULL CHECK(committed IN(0,1)));
INSERT INTO android_scoped_snapshots SELECT * FROM android_device_snapshots;
CREATE TABLE android_scoped_snapshot_chunks (manifest_id TEXT NOT NULL REFERENCES android_scoped_snapshots(id), digest TEXT NOT NULL, bytes_base64 TEXT NOT NULL, PRIMARY KEY(manifest_id,digest));
INSERT INTO android_scoped_snapshot_chunks SELECT * FROM android_device_snapshot_chunks;
CREATE TABLE android_scoped_requests (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES android_scoped_runs(id), kind TEXT NOT NULL, source_id TEXT, request_sha256 TEXT NOT NULL, payload TEXT NOT NULL, state TEXT NOT NULL, revision BIGINT NOT NULL, expires_at BIGINT NOT NULL, UNIQUE(run_id,kind,source_id));
INSERT INTO android_scoped_requests SELECT * FROM android_device_requests;
CREATE TABLE android_scoped_checkpoints (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES android_scoped_runs(id), source_id TEXT NOT NULL, request_sha256 TEXT NOT NULL, payload TEXT NOT NULL, UNIQUE(run_id,source_id));
INSERT INTO android_scoped_checkpoints SELECT * FROM android_device_checkpoints;
CREATE INDEX android_scoped_runs_dispatch ON android_scoped_runs(executor_id,state,created_at);
CREATE INDEX android_scoped_runs_project ON android_scoped_runs(project_id,created_at);
