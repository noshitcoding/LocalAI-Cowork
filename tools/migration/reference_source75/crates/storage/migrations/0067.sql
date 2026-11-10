CREATE TABLE android_run_checkpoint_index (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
    origin TEXT NOT NULL CHECK(origin IN('private_server','device_agent')),
    source_table TEXT NOT NULL CHECK(source_table IN('android_private_loop_checkpoints','android_scoped_checkpoints','android_device_checkpoints')),
    source_row_id TEXT NOT NULL,
    sequence BIGINT NOT NULL CHECK(sequence>0),
    source_revision BIGINT NOT NULL CHECK(source_revision>0),
    checkpoint_sha256 TEXT NOT NULL,
    original_payload_sha256 TEXT NOT NULL,
    spec_sha256 TEXT NOT NULL,
    original_lease_id TEXT,
    header_version_id TEXT REFERENCES android_private_loop_versions(id),
    state_version_id TEXT REFERENCES android_private_loop_versions(id),
    state_sha256 TEXT NOT NULL,
    state_size BIGINT NOT NULL CHECK(state_size>=0 AND state_size<=16777216),
    safe_to_resume BIGINT NOT NULL CHECK(safe_to_resume IN(0,1)),
    created_at BIGINT NOT NULL,
    UNIQUE(run_id,owner_id,origin,sequence)
);
CREATE INDEX android_run_checkpoint_pages ON android_run_checkpoint_index(owner_id,origin,run_id,sequence);
CREATE TABLE android_private_run_continuations (
    run_id TEXT PRIMARY KEY REFERENCES android_private_server_runs(id),
    owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
    request_id TEXT NOT NULL,
    request_sha256 TEXT NOT NULL,
    resume_version_id TEXT NOT NULL REFERENCES android_private_loop_versions(id),
    resume_sha256 TEXT NOT NULL,
    source_revision BIGINT NOT NULL CHECK(source_revision>0),
    queued_revision BIGINT NOT NULL CHECK(queued_revision>source_revision),
    checkpoint_id TEXT NOT NULL REFERENCES android_run_checkpoint_index(id),
    checkpoint_sha256 TEXT NOT NULL,
    spec_sha256 TEXT NOT NULL,
    original_session_id TEXT NOT NULL,
    original_session_revision BIGINT NOT NULL CHECK(original_session_revision>0),
    session_id TEXT NOT NULL REFERENCES identity_sessions(id),
    session_revision BIGINT NOT NULL CHECK(session_revision>0),
    project_revision BIGINT NOT NULL CHECK(project_revision>0),
    created_at BIGINT NOT NULL,
    UNIQUE(owner_id,request_id)
);
CREATE TABLE android_run_waiting_guards (
    run_id TEXT NOT NULL,
    owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
    family TEXT NOT NULL CHECK(family IN('private_server','device_agent')),
    run_revision BIGINT NOT NULL CHECK(run_revision>0),
    state TEXT NOT NULL,
    lease_id TEXT,
    host_slot TEXT,
    reason TEXT NOT NULL CHECK(reason IN('platform_capacity','organisation_capacity','user_capacity','instance_capacity','chat_workspace_busy','admission_unavailable','runner_capacity')),
    checked_at BIGINT NOT NULL,
    PRIMARY KEY(run_id,owner_id,family)
);
