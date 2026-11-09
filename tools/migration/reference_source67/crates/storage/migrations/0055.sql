CREATE TABLE android_private_containers (
 id TEXT PRIMARY KEY,
 run_id TEXT NOT NULL REFERENCES android_private_server_runs(id),
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 project_id TEXT NOT NULL REFERENCES android_personal_projects(id),
 invocation_id TEXT NOT NULL,
 lease_id TEXT NOT NULL,
 original_binding TEXT NOT NULL,
 creation_sha TEXT NOT NULL,
 creation_receipt TEXT,
 container_id TEXT,
 state TEXT NOT NULL CHECK(state IN('prepared','created','start_admitted','running','terminated','removed','uncertain')),
 termination_witness TEXT,
 revision BIGINT NOT NULL,
 created_at BIGINT NOT NULL,
 UNIQUE(run_id,invocation_id)
);
CREATE INDEX android_private_container_pending ON android_private_containers(state,created_at);
