ALTER TABLE historical_import_file_operations ADD COLUMN dispatch_admitted BIGINT NOT NULL DEFAULT 1 CHECK(dispatch_admitted IN(0,1));
ALTER TABLE historical_import_file_operations ADD COLUMN original_unsent_receipt TEXT;
ALTER TABLE historical_import_file_operations ADD COLUMN reserved_bytes BIGINT CHECK(reserved_bytes>=0);
CREATE TABLE IF NOT EXISTS identity_storage_releases (charge_id TEXT PRIMARY KEY, release_key TEXT NOT NULL UNIQUE, bytes BIGINT NOT NULL CHECK(bytes>0), provenance_sha256 TEXT NOT NULL, original_receipt TEXT NOT NULL, created_at BIGINT NOT NULL);
CREATE TABLE historical_import_key_references (import_id TEXT NOT NULL, owner TEXT NOT NULL, stream TEXT NOT NULL, chunk_index BIGINT NOT NULL CHECK(chunk_index>=0), key_instance TEXT NOT NULL, key_id TEXT NOT NULL, sealed_sha256 TEXT NOT NULL, PRIMARY KEY(import_id,stream,chunk_index), FOREIGN KEY(import_id,stream,chunk_index) REFERENCES historical_import_chunks(import_id,stream,chunk_index));
CREATE INDEX historical_import_key_lookup ON historical_import_key_references(owner,import_id,stream,chunk_index,key_id);
ALTER TABLE android_private_containers ADD COLUMN create_dispatch_admitted BIGINT NOT NULL DEFAULT 1 CHECK(create_dispatch_admitted IN(0,1));
ALTER TABLE android_private_containers ADD COLUMN original_unsent_receipt TEXT;
ALTER TABLE android_private_containers ADD COLUMN original_dispatch_receipt TEXT;
CREATE TABLE android_private_schedule_resolutions (
    id TEXT PRIMARY KEY,
    owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
    schedule_id TEXT NOT NULL REFERENCES android_private_schedules(id),
    occurrence_id TEXT NOT NULL UNIQUE REFERENCES android_private_schedule_occurrences(id),
    operation_key TEXT NOT NULL,
    request_sha256 TEXT NOT NULL,
    action TEXT NOT NULL CHECK(action IN('resolve','retire')),
    outcome TEXT NOT NULL,
    evidence_sha256 TEXT NOT NULL,
    evidence TEXT NOT NULL,
    receipt TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    UNIQUE(owner_id,operation_key)
);
CREATE INDEX android_private_schedule_resolution_lookup ON android_private_schedule_resolutions(schedule_id,action);
CREATE TABLE IF NOT EXISTS android_private_runner_authorities (
    slot TEXT PRIMARY KEY,
    host_id TEXT NOT NULL,
    engine_id TEXT NOT NULL,
    revision BIGINT NOT NULL CHECK(revision>0),
    binding_sha256 TEXT NOT NULL,
    payload TEXT NOT NULL,
    active BIGINT NOT NULL CHECK(active IN(0,1)),
    updated_at BIGINT NOT NULL,
    UNIQUE(host_id,engine_id)
);
CREATE TABLE IF NOT EXISTS android_private_runner_revocations (
    slot TEXT NOT NULL REFERENCES android_private_runner_authorities(slot),
    original_revision BIGINT NOT NULL CHECK(original_revision>0),
    binding_sha256 TEXT NOT NULL,
    receipt TEXT NOT NULL,
    PRIMARY KEY(slot,original_revision)
);
CREATE TABLE IF NOT EXISTS android_private_runner_activations (
    slot TEXT NOT NULL REFERENCES android_private_runner_authorities(slot),
    original_revision BIGINT NOT NULL CHECK(original_revision>0),
    requested_binding_sha256 TEXT NOT NULL,
    receipt TEXT NOT NULL,
    PRIMARY KEY(slot,original_revision)
);
CREATE TABLE historical_import_activated_parts (
    import_id TEXT NOT NULL REFERENCES historical_imports(id),
    owner TEXT NOT NULL,
    source_id TEXT NOT NULL,
    preview_id TEXT NOT NULL REFERENCES historical_import_previews(id),
    part_receipt_sha256 TEXT NOT NULL,
    part_receipt TEXT NOT NULL,
    activated_at BIGINT NOT NULL,
    PRIMARY KEY(import_id,source_id)
);
CREATE INDEX historical_import_activated_parts_owner ON historical_import_activated_parts(owner,import_id,source_id);
