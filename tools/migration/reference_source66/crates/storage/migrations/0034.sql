-- Archival markers retain immutable receipts and idempotency identities while
-- removing reviewed history from the active journal quota.
CREATE TABLE workflow_journal_archives (
 project_id TEXT NOT NULL REFERENCES projects(id),
 kind TEXT NOT NULL CHECK(kind IN ('draft','snapshot','operation')),
 object_id TEXT NOT NULL,
 archived_at BIGINT NOT NULL,
 PRIMARY KEY(project_id,kind,object_id)
);
