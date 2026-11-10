-- Proposed additive schema70 leaf, central migration registration belongs to the integrator.
CREATE TABLE android_project_versions (
 id TEXT PRIMARY KEY, project_id TEXT NOT NULL,
 team_id TEXT REFERENCES identity_organizations(id), creator_id TEXT NOT NULL REFERENCES identity_subjects(id),
 revision BIGINT NOT NULL CHECK(revision>0), payload TEXT NOT NULL,
 payload_sha256 TEXT NOT NULL CHECK(LENGTH(payload_sha256)=64),
 content_sha256 TEXT NOT NULL CHECK(LENGTH(content_sha256)=64), created_at BIGINT NOT NULL,
 ready BIGINT NOT NULL DEFAULT 1 CHECK(ready IN(0,1)),
 UNIQUE(project_id,revision)
);
CREATE TABLE android_project_version_parts (
 version_id TEXT NOT NULL REFERENCES android_project_versions(id), piece_key TEXT NOT NULL,
 ordinal BIGINT NOT NULL CHECK(ordinal>=0 AND ordinal<256), digest TEXT NOT NULL CHECK(LENGTH(digest)=64),
 size BIGINT NOT NULL CHECK(size>0 AND size<=1048576), sealed TEXT NOT NULL,
 PRIMARY KEY(version_id,piece_key,ordinal)
);
CREATE TABLE android_project_version_objects (
 version_id TEXT NOT NULL REFERENCES android_project_versions(id), kind TEXT NOT NULL, source_id TEXT NOT NULL,
 payload_sha256 TEXT NOT NULL CHECK(LENGTH(payload_sha256)=64), size BIGINT NOT NULL CHECK(size>=0 AND size<=16777216),
 part_count BIGINT NOT NULL CHECK(part_count>=0 AND part_count<=16), PRIMARY KEY(version_id,kind,source_id)
);
CREATE TABLE android_project_version_files (
 version_id TEXT NOT NULL REFERENCES android_project_versions(id), path TEXT NOT NULL, source_id TEXT NOT NULL,
 size BIGINT NOT NULL CHECK(size>=0 AND size<=33554432), sha256 TEXT NOT NULL CHECK(LENGTH(sha256)=64),
 media_type TEXT NOT NULL, payload TEXT NOT NULL, PRIMARY KEY(version_id,path), UNIQUE(version_id,source_id)
);
CREATE TABLE android_project_version_heads (
 project_id TEXT PRIMARY KEY, team_id TEXT REFERENCES identity_organizations(id),
 current_version_id TEXT NOT NULL REFERENCES android_project_versions(id), active_snapshot_id TEXT,
 revision BIGINT NOT NULL CHECK(revision>0)
);
CREATE TABLE android_project_version_operations (
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id), operation_key TEXT NOT NULL, project_id TEXT NOT NULL,
 kind TEXT NOT NULL, request_sha256 TEXT NOT NULL CHECK(LENGTH(request_sha256)=64), receipt TEXT NOT NULL,
 receipt_sha256 TEXT NOT NULL CHECK(LENGTH(receipt_sha256)=64), PRIMARY KEY(owner_id,operation_key)
);
CREATE TABLE android_project_thread_history (
 id TEXT PRIMARY KEY, project_id TEXT NOT NULL, thread_id TEXT NOT NULL,
 team_id TEXT REFERENCES identity_organizations(id), creator_id TEXT NOT NULL REFERENCES identity_subjects(id),
 payload TEXT NOT NULL, source_id TEXT NOT NULL, original_sha256 TEXT NOT NULL CHECK(LENGTH(original_sha256)=64)
);
CREATE INDEX android_project_thread_history_scope ON android_project_thread_history(project_id,thread_id,id);
CREATE TABLE android_project_thread_lineage (
 thread_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, creator_id TEXT NOT NULL REFERENCES identity_subjects(id),
 source_thread_id TEXT NOT NULL, source_message_id TEXT, receipt TEXT NOT NULL,
 receipt_sha256 TEXT NOT NULL CHECK(LENGTH(receipt_sha256)=64)
);
CREATE TABLE android_project_snapshots (
 id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES tenant_projects(id),
 team_id TEXT NOT NULL REFERENCES identity_organizations(id), creator_id TEXT NOT NULL REFERENCES identity_subjects(id),
 creation_key TEXT NOT NULL, request_sha256 TEXT NOT NULL CHECK(LENGTH(request_sha256)=64),
 payload TEXT NOT NULL, committed BIGINT NOT NULL CHECK(committed IN(0,1)), UNIQUE(creator_id,creation_key)
);
CREATE INDEX android_project_snapshots_scope ON android_project_snapshots(project_id,id);
CREATE TABLE android_project_snapshot_chunks (
 snapshot_id TEXT NOT NULL REFERENCES android_project_snapshots(id), digest TEXT NOT NULL CHECK(LENGTH(digest)=64),
 ordinal BIGINT NOT NULL CHECK(ordinal>=0 AND ordinal<4), page_digest TEXT NOT NULL CHECK(LENGTH(page_digest)=64),
 size BIGINT NOT NULL CHECK(size>0 AND size<=1048576), sealed TEXT NOT NULL,
 PRIMARY KEY(snapshot_id,digest,ordinal)
);
CREATE TABLE android_project_copy_plans (
 id TEXT PRIMARY KEY, owner_id TEXT NOT NULL REFERENCES identity_subjects(id), operation_key TEXT NOT NULL,
 source_project_id TEXT NOT NULL, source_version_id TEXT NOT NULL REFERENCES android_project_versions(id),
 target_project_id TEXT NOT NULL, request_sha256 TEXT NOT NULL CHECK(LENGTH(request_sha256)=64),
 payload TEXT NOT NULL, payload_sha256 TEXT NOT NULL CHECK(LENGTH(payload_sha256)=64),
 state TEXT NOT NULL CHECK(state IN('prepared','receiving','uncertain','committed','retired')),
 receipt TEXT, receipt_sha256 TEXT CHECK(receipt_sha256 IS NULL OR LENGTH(receipt_sha256)=64),
 created_at BIGINT NOT NULL, UNIQUE(owner_id,operation_key)
);
CREATE TABLE android_project_copy_effects (
 plan_id TEXT NOT NULL REFERENCES android_project_copy_plans(id), path TEXT NOT NULL,
 original_sha256 TEXT NOT NULL CHECK(LENGTH(original_sha256)=64), target TEXT NOT NULL,
 request_sha256 TEXT NOT NULL CHECK(LENGTH(request_sha256)=64),
 state TEXT NOT NULL CHECK(state IN('prepared','dispatched','received','uncertain','retired')),
 nonce TEXT, dispatch_sha256 TEXT CHECK(dispatch_sha256 IS NULL OR LENGTH(dispatch_sha256)=64),
 receipt TEXT, receipt_sha256 TEXT CHECK(receipt_sha256 IS NULL OR LENGTH(receipt_sha256)=64), PRIMARY KEY(plan_id,path)
);
CREATE TABLE android_project_copy_receipts (
 plan_id TEXT NOT NULL, path TEXT NOT NULL, kind TEXT NOT NULL CHECK(kind IN('original_put','original_read')),
 receipt TEXT NOT NULL, receipt_sha256 TEXT NOT NULL CHECK(LENGTH(receipt_sha256)=64), PRIMARY KEY(plan_id,path,kind),
 FOREIGN KEY(plan_id,path) REFERENCES android_project_copy_effects(plan_id,path)
);
