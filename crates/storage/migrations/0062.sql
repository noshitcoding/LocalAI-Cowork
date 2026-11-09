CREATE TABLE android_private_terminal_sessions (
 id TEXT PRIMARY KEY,
 run_id TEXT NOT NULL REFERENCES android_private_server_runs(id),
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 project_id TEXT NOT NULL REFERENCES android_personal_projects(id),
 lease_id TEXT NOT NULL,
 creation_key TEXT NOT NULL,
 request_sha TEXT NOT NULL,
 ticket_sha TEXT NOT NULL,
 ticket_version TEXT NOT NULL REFERENCES android_private_loop_versions(id),
 baseline_version TEXT NOT NULL REFERENCES android_private_loop_versions(id),
 container_nonce TEXT REFERENCES android_private_containers(id),
 state TEXT NOT NULL CHECK(state IN('created','connected','closing','closed','uncertain')),
 columns BIGINT NOT NULL,
 rows BIGINT NOT NULL,
 ticket_expires_at BIGINT NOT NULL,
 deadline BIGINT NOT NULL,
 input_sequence BIGINT NOT NULL,
 output_sequence BIGINT NOT NULL,
 input_bytes BIGINT NOT NULL,
 output_bytes BIGINT NOT NULL,
 receipt_version TEXT REFERENCES android_private_loop_versions(id),
 publication_version TEXT REFERENCES android_private_loop_versions(id),
 revision BIGINT NOT NULL,
 created_at BIGINT NOT NULL,
 UNIQUE(run_id,creation_key)
);
CREATE INDEX android_private_terminal_active ON android_private_terminal_sessions(run_id,state);
CREATE TABLE android_private_terminal_inputs (
 session_id TEXT NOT NULL REFERENCES android_private_terminal_sessions(id),
 sequence BIGINT NOT NULL,
 request_sha TEXT NOT NULL,
 intent_version TEXT NOT NULL REFERENCES android_private_loop_versions(id),
 receipt_version TEXT REFERENCES android_private_loop_versions(id),
 state TEXT NOT NULL CHECK(state IN('admitted','received','uncertain')),
 PRIMARY KEY(session_id,sequence)
);
CREATE TABLE android_private_terminal_outputs (
 session_id TEXT NOT NULL REFERENCES android_private_terminal_sessions(id),
 sequence BIGINT NOT NULL,
 original_sha TEXT NOT NULL,
 version_id TEXT NOT NULL REFERENCES android_private_loop_versions(id),
 PRIMARY KEY(session_id,sequence)
);
