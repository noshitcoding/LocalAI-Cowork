CREATE TABLE android_private_mcp_bindings (
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 project_id TEXT NOT NULL REFERENCES android_personal_projects(id),
 entity_id TEXT NOT NULL,
 revision BIGINT NOT NULL,
 metadata_revision BIGINT NOT NULL,
 metadata_sha TEXT NOT NULL,
 payload TEXT NOT NULL,
 binding_sha TEXT NOT NULL,
 sealed TEXT NOT NULL,
 deleted BIGINT NOT NULL DEFAULT 0 CHECK(deleted IN(0,1)),
 PRIMARY KEY(owner_id,project_id,entity_id)
);
CREATE TABLE android_private_mcp_operations (
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 project_id TEXT NOT NULL REFERENCES android_personal_projects(id),
 operation_key TEXT NOT NULL,
 request_mac TEXT NOT NULL,
 request_key_id TEXT NOT NULL,
 receipt TEXT NOT NULL,
 PRIMARY KEY(owner_id,project_id,operation_key)
);
CREATE TABLE android_private_runner_bindings (
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 project_id TEXT NOT NULL REFERENCES android_personal_projects(id),
 revision BIGINT NOT NULL,
 payload TEXT NOT NULL,
 revoked BIGINT NOT NULL DEFAULT 0 CHECK(revoked IN(0,1)),
 PRIMARY KEY(owner_id,project_id)
);
CREATE TABLE android_private_mcp_sessions (
 run_id TEXT NOT NULL REFERENCES android_private_server_runs(id),
 entity_id TEXT NOT NULL,
 binding_sha TEXT NOT NULL,
 catalog_sha TEXT NOT NULL,
 definitions TEXT NOT NULL,
 protocol TEXT NOT NULL,
 checkpoint TEXT,
 revision BIGINT NOT NULL,
 state TEXT NOT NULL CHECK(state IN('ready','admitted','uncertain','closed')),
 PRIMARY KEY(run_id,entity_id)
);
