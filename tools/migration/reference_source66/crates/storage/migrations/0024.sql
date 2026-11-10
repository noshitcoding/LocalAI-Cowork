CREATE TABLE android_compat_guard (id BIGINT PRIMARY KEY CHECK(id=1), observed_at BIGINT NOT NULL);
INSERT INTO android_compat_guard(id,observed_at) VALUES(1,0);
CREATE TABLE android_auth_grants (id TEXT PRIMARY KEY, kind TEXT NOT NULL CHECK(kind IN('code','refresh')), token_sha256 TEXT NOT NULL UNIQUE, device_id TEXT NOT NULL, expires_at BIGINT NOT NULL, consumed BIGINT NOT NULL CHECK(consumed IN(0,1)), payload TEXT NOT NULL);
CREATE INDEX android_auth_expiry ON android_auth_grants(expires_at);
CREATE TABLE android_run_events (run_id TEXT NOT NULL REFERENCES team_runs(id), sequence BIGINT NOT NULL CHECK(sequence>0), snapshot_sha256 TEXT NOT NULL, payload TEXT NOT NULL, PRIMARY KEY(run_id,sequence), UNIQUE(run_id,snapshot_sha256));
CREATE TABLE android_wire_operations (identity_id TEXT NOT NULL REFERENCES identity_subjects(id), team_id TEXT NOT NULL REFERENCES identity_organizations(id), project_id TEXT NOT NULL REFERENCES tenant_projects(id), operation_key TEXT NOT NULL, request_sha256 TEXT NOT NULL, receipt TEXT, PRIMARY KEY(identity_id,operation_key));
CREATE TABLE android_run_files (run_id TEXT NOT NULL REFERENCES team_runs(id), blob_id TEXT NOT NULL REFERENCES team_blobs(id), PRIMARY KEY(run_id,blob_id));
