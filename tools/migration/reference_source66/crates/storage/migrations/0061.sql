CREATE TABLE android_private_github_bindings (
 id TEXT PRIMARY KEY,
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 project_id TEXT NOT NULL REFERENCES android_personal_projects(id),
 revision BIGINT NOT NULL,
 binding_sha TEXT NOT NULL,
 payload TEXT NOT NULL,
 sealed TEXT,
 deleted BIGINT NOT NULL DEFAULT 0,
 updated_at BIGINT NOT NULL
);
CREATE INDEX android_private_github_by_actor ON android_private_github_bindings(owner_id,project_id,deleted);
CREATE TABLE android_private_github_operations (
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 project_id TEXT NOT NULL REFERENCES android_personal_projects(id),
 operation_key TEXT NOT NULL,
 request_key_id TEXT NOT NULL,
 request_mac TEXT NOT NULL,
 receipt TEXT NOT NULL,
 PRIMARY KEY(owner_id,project_id,operation_key)
);
CREATE TABLE android_private_github_sessions (
 id TEXT PRIMARY KEY,
 run_id TEXT NOT NULL REFERENCES android_private_server_runs(id),
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 project_id TEXT NOT NULL REFERENCES android_personal_projects(id),
 lease_id TEXT NOT NULL,
 binding_id TEXT NOT NULL REFERENCES android_private_github_bindings(id),
 binding_sha TEXT NOT NULL,
 directory TEXT NOT NULL,
 git_version TEXT REFERENCES android_private_loop_versions(id),
 baseline_version TEXT NOT NULL REFERENCES android_private_loop_versions(id),
 container_nonce TEXT REFERENCES android_private_containers(id),
 last_sequence BIGINT NOT NULL,
 state TEXT NOT NULL CHECK(state IN('admitted','ready','closed','uncertain')),
 revision BIGINT NOT NULL,
 created_at BIGINT NOT NULL
);
CREATE INDEX android_private_github_active ON android_private_github_sessions(run_id,state);
CREATE TABLE android_private_github_requests (
 session_id TEXT NOT NULL REFERENCES android_private_github_sessions(id),
 action_sequence BIGINT NOT NULL,
 request_id TEXT NOT NULL,
 request_sha TEXT NOT NULL,
 intent_version TEXT NOT NULL REFERENCES android_private_loop_versions(id),
 receipt_version TEXT REFERENCES android_private_loop_versions(id),
 dispatch_admitted BIGINT NOT NULL DEFAULT 0,
 effect BIGINT NOT NULL,
 state TEXT NOT NULL CHECK(state IN('admitted','received','uncertain')),
 created_at BIGINT NOT NULL,
 PRIMARY KEY(session_id,request_id)
);
