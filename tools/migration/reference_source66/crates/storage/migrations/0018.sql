CREATE TABLE workflow_write_keys (
 namespace TEXT NOT NULL,
 target_id TEXT NOT NULL,
 request_key TEXT NOT NULL,
 project_id TEXT NOT NULL REFERENCES projects(id),
 request_sha256 TEXT NOT NULL,
 request_payload TEXT NOT NULL,
 operation_id TEXT,
 state TEXT NOT NULL CHECK(state IN ('reserved','accepted','rejected','discarded')),
 receipt TEXT,
 PRIMARY KEY(namespace,target_id,request_key)
);
CREATE TABLE workflow_drafts (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES projects(id),
 revision BIGINT NOT NULL CHECK(revision >= 1),
 snapshot_id TEXT NOT NULL,
 payload TEXT NOT NULL,
 UNIQUE(id,project_id)
);
CREATE TABLE workflow_draft_snapshots (
 id TEXT PRIMARY KEY,
 draft_id TEXT NOT NULL,
 project_id TEXT NOT NULL,
 payload_bytes BIGINT NOT NULL CHECK(payload_bytes >= 0),
 payload TEXT NOT NULL,
 FOREIGN KEY(draft_id,project_id) REFERENCES workflow_drafts(id,project_id),
 UNIQUE(id,draft_id,project_id)
);
CREATE TABLE workflow_draft_saves (
 draft_id TEXT NOT NULL,
 request_key TEXT NOT NULL,
 request_sha256 TEXT NOT NULL,
 payload TEXT NOT NULL,
 PRIMARY KEY(draft_id,request_key),
 FOREIGN KEY(draft_id) REFERENCES workflow_drafts(id)
);
CREATE TABLE workflow_operations (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES projects(id),
 intent_key TEXT NOT NULL,
 request_sha256 TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('pending','accepted','rejected','discarded')),
 acknowledged_at BIGINT,
 payload TEXT NOT NULL,
 UNIQUE(project_id,intent_key)
);
CREATE INDEX workflow_drafts_project ON workflow_drafts(project_id,id);
CREATE INDEX workflow_operations_project ON workflow_operations(project_id,id);
CREATE TABLE workflow_journal_quotas (
 project_id TEXT PRIMARY KEY REFERENCES projects(id),
 drafts BIGINT NOT NULL DEFAULT 0 CHECK(drafts >= 0 AND drafts <= 32),
 snapshots BIGINT NOT NULL DEFAULT 0 CHECK(snapshots >= 0 AND snapshots <= 4096),
 snapshot_bytes BIGINT NOT NULL DEFAULT 0 CHECK(snapshot_bytes >= 0 AND snapshot_bytes <= 33554432),
 operations BIGINT NOT NULL DEFAULT 0 CHECK(operations >= 0 AND operations <= 512)
);
