CREATE TABLE android_private_browser_sessions (
 id TEXT PRIMARY KEY,
 run_id TEXT NOT NULL REFERENCES android_private_server_runs(id),
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 project_id TEXT NOT NULL REFERENCES android_personal_projects(id),
 lease_id TEXT NOT NULL,
 container_nonce TEXT REFERENCES android_private_containers(id),
 baseline_version TEXT NOT NULL REFERENCES android_private_loop_versions(id),
 last_sequence BIGINT NOT NULL,
 state TEXT NOT NULL CHECK(state IN('admitted','ready','closed','uncertain')),
 revision BIGINT NOT NULL,
 created_at BIGINT NOT NULL
);
CREATE INDEX android_private_browser_active ON android_private_browser_sessions(run_id,state);
CREATE TABLE android_private_browser_requests (
 session_id TEXT NOT NULL REFERENCES android_private_browser_sessions(id),
 action_sequence BIGINT NOT NULL,
 request_id TEXT NOT NULL,
 request_sha TEXT NOT NULL,
 intent_version TEXT NOT NULL REFERENCES android_private_loop_versions(id),
 receipt_version TEXT REFERENCES android_private_loop_versions(id),
 state TEXT NOT NULL CHECK(state IN('admitted','received','uncertain')),
 created_at BIGINT NOT NULL,
 PRIMARY KEY(session_id,request_id)
);
