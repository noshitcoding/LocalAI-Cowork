CREATE TABLE android_private_loop_checkpoints (
 run_id TEXT PRIMARY KEY REFERENCES android_private_server_runs(id),
 digest TEXT NOT NULL,
 sealed TEXT NOT NULL,
 revision BIGINT NOT NULL,
 updated_at BIGINT NOT NULL
);
CREATE TABLE android_private_loop_steps (
 run_id TEXT NOT NULL REFERENCES android_private_server_runs(id),
 sequence BIGINT NOT NULL,
 invocation_id TEXT NOT NULL,
 kind TEXT NOT NULL CHECK(kind IN('model','tool','mcp','process')),
 lease_id TEXT NOT NULL,
 checkpoint_sha TEXT NOT NULL,
 request_sha TEXT NOT NULL,
 intent TEXT NOT NULL,
 receipt_sha TEXT,
 receipt TEXT,
 state TEXT NOT NULL CHECK(state IN('admitted','received','uncertain')),
 created_at BIGINT NOT NULL,
 PRIMARY KEY(run_id,sequence),
 UNIQUE(run_id,invocation_id)
);
CREATE TABLE android_private_loop_versions (
 id TEXT PRIMARY KEY,
 run_id TEXT NOT NULL REFERENCES android_private_server_runs(id),
 path TEXT NOT NULL,
 kind TEXT NOT NULL CHECK(kind IN('file','directory','artifact')),
 digest TEXT NOT NULL,
 size BIGINT NOT NULL,
 sealed TEXT NOT NULL,
 created_at BIGINT NOT NULL
);
CREATE TABLE android_private_loop_entries (
 run_id TEXT NOT NULL REFERENCES android_private_server_runs(id),
 path TEXT NOT NULL,
 version_id TEXT NOT NULL REFERENCES android_private_loop_versions(id),
 PRIMARY KEY(run_id,path)
);
CREATE TABLE android_private_loop_requests (
 id TEXT PRIMARY KEY,
 run_id TEXT NOT NULL REFERENCES android_private_server_runs(id),
 invocation_id TEXT NOT NULL,
 kind TEXT NOT NULL CHECK(kind IN('approval','input')),
 fingerprint TEXT NOT NULL,
 payload TEXT NOT NULL,
 state TEXT NOT NULL,
 revision BIGINT NOT NULL,
 expires_at BIGINT NOT NULL,
 UNIQUE(run_id,invocation_id,kind)
);
CREATE TABLE android_private_loop_artifacts (
 id TEXT PRIMARY KEY,
 run_id TEXT NOT NULL REFERENCES android_private_server_runs(id),
 version_id TEXT NOT NULL REFERENCES android_private_loop_versions(id),
 payload TEXT NOT NULL
);
CREATE TABLE android_private_loop_results (
 run_id TEXT PRIMARY KEY REFERENCES android_private_server_runs(id),
 snapshot_id TEXT NOT NULL REFERENCES android_private_snapshots(id),
 checkpoint_sha TEXT NOT NULL,
 receipt_sha TEXT NOT NULL,
 payload TEXT NOT NULL
);
