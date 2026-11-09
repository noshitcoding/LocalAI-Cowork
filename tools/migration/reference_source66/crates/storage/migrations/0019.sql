CREATE TABLE workflow_input_drafts (
 run_id TEXT NOT NULL REFERENCES runs(id), request_id TEXT NOT NULL, owner TEXT NOT NULL,
 project_id TEXT NOT NULL REFERENCES projects(id), revision BIGINT NOT NULL, snapshot_id TEXT NOT NULL, payload TEXT NOT NULL,
 PRIMARY KEY(run_id,request_id,owner)
);
CREATE TABLE workflow_input_draft_snapshots (
 id TEXT PRIMARY KEY, run_id TEXT NOT NULL, request_id TEXT NOT NULL, owner TEXT NOT NULL,
 project_id TEXT NOT NULL REFERENCES projects(id), payload_bytes BIGINT NOT NULL, payload TEXT NOT NULL,
 FOREIGN KEY(run_id,request_id,owner) REFERENCES workflow_input_drafts(run_id,request_id,owner)
);
CREATE TABLE workflow_input_draft_saves (
 run_id TEXT NOT NULL, request_id TEXT NOT NULL, owner TEXT NOT NULL, request_key TEXT NOT NULL,
 request_sha256 TEXT NOT NULL, payload TEXT NOT NULL,
 PRIMARY KEY(run_id,request_id,owner,request_key),
 FOREIGN KEY(run_id,request_id,owner) REFERENCES workflow_input_drafts(run_id,request_id,owner)
);
CREATE TABLE workflow_input_answer_intents (
 id TEXT PRIMARY KEY, run_id TEXT NOT NULL, request_id TEXT NOT NULL, owner TEXT NOT NULL,
 project_id TEXT NOT NULL REFERENCES projects(id), request_key TEXT NOT NULL,
 prepare_sha256 TEXT NOT NULL, request_sha256 TEXT NOT NULL, state TEXT NOT NULL, payload TEXT NOT NULL,
 UNIQUE(run_id,owner,request_key),
 FOREIGN KEY(run_id,request_id,owner) REFERENCES workflow_input_drafts(run_id,request_id,owner)
);
CREATE INDEX workflow_input_intent_scope ON workflow_input_answer_intents(run_id,request_id,owner);
CREATE TABLE workflow_input_quotas (
 project_id TEXT PRIMARY KEY REFERENCES projects(id), heads BIGINT NOT NULL DEFAULT 0,
 snapshots BIGINT NOT NULL DEFAULT 0, snapshot_bytes BIGINT NOT NULL DEFAULT 0, intents BIGINT NOT NULL DEFAULT 0
);
