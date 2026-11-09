CREATE TABLE android_private_workflows (
 id TEXT PRIMARY KEY, owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 project_id TEXT NOT NULL REFERENCES android_personal_projects(id),
 name TEXT NOT NULL, engine TEXT NOT NULL CHECK(engine IN('workflow','crew')),
 revision BIGINT NOT NULL CHECK(revision>0), revoked BIGINT NOT NULL CHECK(revoked IN(0,1)),
 published_version_id TEXT, source_entity_id TEXT, source_revision BIGINT, source_legacy_id TEXT,
 UNIQUE(id,owner_id,project_id), CHECK((source_entity_id IS NULL)=(source_revision IS NULL)),
 UNIQUE(owner_id,project_id,engine,source_legacy_id),
 CHECK(source_revision IS NULL OR source_revision>0)
);
CREATE INDEX android_private_workflow_owner ON android_private_workflows(owner_id,project_id,id);
CREATE TABLE android_private_workflow_versions (
 id TEXT PRIMARY KEY, workflow_id TEXT NOT NULL, owner_id TEXT NOT NULL, project_id TEXT NOT NULL,
 revision BIGINT NOT NULL CHECK(revision>0), definition_sha256 TEXT NOT NULL,
 definition_size BIGINT NOT NULL CHECK(definition_size>0 AND definition_size<=2097152),
 created_at BIGINT NOT NULL,
 UNIQUE(workflow_id,revision), UNIQUE(id,workflow_id,owner_id,project_id),
 FOREIGN KEY(workflow_id,owner_id,project_id) REFERENCES android_private_workflows(id,owner_id,project_id)
);
CREATE TABLE android_private_workflow_chunks (
 id TEXT PRIMARY KEY, owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 project_id TEXT NOT NULL REFERENCES android_personal_projects(id), object_id TEXT NOT NULL,
 purpose TEXT NOT NULL CHECK(purpose IN('version','request','receipt')),
 ordinal BIGINT NOT NULL CHECK(ordinal>=0), digest TEXT NOT NULL,
 size BIGINT NOT NULL CHECK(size>0 AND size<=1048576), sealed TEXT NOT NULL,
 UNIQUE(owner_id,project_id,object_id,purpose,ordinal)
);
CREATE INDEX android_private_workflow_chunk_object ON android_private_workflow_chunks(owner_id,project_id,object_id,purpose,ordinal);
CREATE TABLE android_private_workflow_operations (
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id), request_id TEXT NOT NULL,
 project_id TEXT NOT NULL REFERENCES android_personal_projects(id), workflow_id TEXT NOT NULL,
 purpose TEXT NOT NULL CHECK(purpose IN('create','version','change')),
 request_sha256 TEXT NOT NULL, request_size BIGINT NOT NULL CHECK(request_size>0 AND request_size<=3145728),
 receipt_sha256 TEXT NOT NULL, receipt_size BIGINT NOT NULL CHECK(receipt_size>0 AND receipt_size<=1048576),
 created_at BIGINT NOT NULL,
 PRIMARY KEY(owner_id,request_id),
 FOREIGN KEY(workflow_id,owner_id,project_id) REFERENCES android_private_workflows(id,owner_id,project_id)
);
CREATE TABLE android_private_workflow_runs (
 run_id TEXT PRIMARY KEY REFERENCES android_private_server_runs(id),
 owner_id TEXT NOT NULL, project_id TEXT NOT NULL, workflow_id TEXT NOT NULL, version_id TEXT NOT NULL,
 engine TEXT NOT NULL CHECK(engine IN('workflow','crew')), registry_revision BIGINT NOT NULL CHECK(registry_revision>0),
 definition_sha256 TEXT NOT NULL, plan_sha256 TEXT NOT NULL, revision BIGINT NOT NULL CHECK(revision>0),
 checkpoint_version_id TEXT NOT NULL REFERENCES android_private_loop_versions(id),
 checkpoint_sha256 TEXT NOT NULL, deadline BIGINT NOT NULL CHECK(deadline>0),
 observed_at BIGINT NOT NULL CHECK(observed_at>0),
 FOREIGN KEY(version_id,workflow_id,owner_id,project_id) REFERENCES android_private_workflow_versions(id,workflow_id,owner_id,project_id)
);
CREATE TABLE android_private_workflow_nodes (
 run_id TEXT NOT NULL REFERENCES android_private_workflow_runs(run_id), node_id TEXT NOT NULL,
 role_id TEXT NOT NULL, claim_token TEXT, parent_lease_id TEXT,
 state TEXT NOT NULL CHECK(state IN('pending','running','waiting','completed','failed','interrupted','blocked')),
 revision BIGINT NOT NULL CHECK(revision>0), context_version_id TEXT REFERENCES android_private_loop_versions(id),
 context_sha256 TEXT, output_version_id TEXT REFERENCES android_private_loop_versions(id),
 output_sha256 TEXT, PRIMARY KEY(run_id,node_id),
 CHECK((context_version_id IS NULL)=(context_sha256 IS NULL)),
 CHECK((output_version_id IS NULL)=(output_sha256 IS NULL))
);
CREATE TABLE android_private_workflow_engines (
 slot TEXT PRIMARY KEY, engine TEXT NOT NULL CHECK(engine IN('workflow','crew')),
 instance_id TEXT NOT NULL, runner_binding_sha256 TEXT,
 revision BIGINT NOT NULL CHECK(revision>0), config_sha256 TEXT NOT NULL,
 active BIGINT NOT NULL CHECK(active IN(0,1)), payload TEXT NOT NULL,
 UNIQUE(instance_id,engine,slot)
);
CREATE TABLE android_private_workflow_engine_operations (
 slot TEXT NOT NULL REFERENCES android_private_workflow_engines(slot),
 request_id TEXT NOT NULL, expected_revision BIGINT NOT NULL CHECK(expected_revision>0),
 request_sha256 TEXT NOT NULL, original_receipt TEXT NOT NULL,
 PRIMARY KEY(slot,request_id), UNIQUE(slot,expected_revision)
);
CREATE TABLE android_private_workflow_rpcs (
 run_id TEXT NOT NULL, node_id TEXT NOT NULL, source_id TEXT NOT NULL,
 original_lease_id TEXT NOT NULL, container_nonce TEXT NOT NULL REFERENCES android_private_containers(id),
 kind TEXT NOT NULL CHECK(kind IN('model','tool','result')), request_sha256 TEXT NOT NULL,
 request_version_id TEXT NOT NULL REFERENCES android_private_loop_versions(id),
 context_sha256 TEXT NOT NULL, reply_version_id TEXT REFERENCES android_private_loop_versions(id),
 reply_sha256 TEXT, state TEXT NOT NULL CHECK(state IN('prepared','admitted','received','uncertain')),
 step_sequence BIGINT, created_at BIGINT NOT NULL,
 PRIMARY KEY(run_id,source_id),
 FOREIGN KEY(run_id,node_id) REFERENCES android_private_workflow_nodes(run_id,node_id),
 CHECK((reply_version_id IS NULL)=(reply_sha256 IS NULL))
);
