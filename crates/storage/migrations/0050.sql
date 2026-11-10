CREATE TABLE android_private_server_runs (
 id TEXT PRIMARY KEY,
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 session_id TEXT NOT NULL REFERENCES identity_sessions(id),
 session_revision BIGINT NOT NULL,
 project_id TEXT NOT NULL REFERENCES android_personal_projects(id),
 project_revision BIGINT NOT NULL,
 thread_id TEXT NOT NULL REFERENCES android_personal_threads(id),
 provider_id TEXT NOT NULL REFERENCES android_personal_provider_profiles(id),
 snapshot_id TEXT,
 creation_key TEXT NOT NULL,
 request_sha TEXT NOT NULL,
 initial_receipt TEXT NOT NULL,
 payload TEXT NOT NULL,
 state TEXT NOT NULL,
 lease_id TEXT,
 lease_until BIGINT NOT NULL DEFAULT 0,
 effect_intent TEXT,
 effect_received TEXT,
 created_at BIGINT NOT NULL,
 UNIQUE(owner_id,creation_key)
);
CREATE INDEX android_private_server_queue ON android_private_server_runs(state,created_at);
CREATE TABLE android_private_server_events (
 run_id TEXT NOT NULL REFERENCES android_private_server_runs(id),
 sequence BIGINT NOT NULL,
 payload TEXT NOT NULL,
 PRIMARY KEY(run_id,sequence)
);
CREATE TABLE android_private_snapshots (
 id TEXT PRIMARY KEY,
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 project_id TEXT NOT NULL REFERENCES android_personal_projects(id),
 creation_key TEXT NOT NULL,
 request_sha TEXT NOT NULL,
 payload TEXT NOT NULL,
 committed BIGINT NOT NULL DEFAULT 0 CHECK(committed IN(0,1)),
 deleted BIGINT NOT NULL DEFAULT 0 CHECK(deleted IN(0,1)),
 UNIQUE(owner_id,creation_key)
);
CREATE TABLE android_private_snapshot_chunks (
 snapshot_id TEXT NOT NULL REFERENCES android_private_snapshots(id),
 digest TEXT NOT NULL,
 size BIGINT NOT NULL,
 sealed TEXT NOT NULL,
 PRIMARY KEY(snapshot_id,digest)
);
CREATE TABLE android_private_files (
 id TEXT PRIMARY KEY,
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 project_id TEXT NOT NULL REFERENCES android_personal_projects(id),
 snapshot_id TEXT NOT NULL REFERENCES android_private_snapshots(id),
 path TEXT NOT NULL,
 payload TEXT NOT NULL,
 deleted BIGINT NOT NULL DEFAULT 0 CHECK(deleted IN(0,1)),
 UNIQUE(snapshot_id,path)
);
