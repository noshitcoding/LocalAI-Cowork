CREATE TABLE workflows (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES projects(id),
 name TEXT NOT NULL,
 revision BIGINT NOT NULL CHECK(revision >= 1),
 revoked BIGINT NOT NULL CHECK(revoked IN (0,1)),
 request_key TEXT NOT NULL,
 request_sha256 TEXT NOT NULL,
 UNIQUE(project_id,request_key),
 UNIQUE(id,project_id)
);
CREATE TABLE workflow_versions (
 id TEXT PRIMARY KEY,
 workflow_id TEXT NOT NULL,
 project_id TEXT NOT NULL,
 revision BIGINT NOT NULL,
 name TEXT NOT NULL,
 sha256 TEXT NOT NULL,
 created_at BIGINT NOT NULL,
 payload TEXT NOT NULL,
 UNIQUE(workflow_id,revision),
 UNIQUE(id,workflow_id,project_id),
 FOREIGN KEY(workflow_id,project_id) REFERENCES workflows(id,project_id)
);
CREATE TABLE workflow_changes (
 workflow_id TEXT NOT NULL REFERENCES workflows(id),
 request_key TEXT NOT NULL,
 request_sha256 TEXT NOT NULL,
 revision BIGINT NOT NULL,
 payload TEXT NOT NULL,
 PRIMARY KEY(workflow_id,request_key),
 UNIQUE(workflow_id,revision)
);
CREATE TABLE workflow_runs (
 run_id TEXT PRIMARY KEY REFERENCES runs(id),
 workflow_id TEXT NOT NULL,
 version_id TEXT NOT NULL,
 project_id TEXT NOT NULL,
 revision BIGINT NOT NULL CHECK(revision >= 0),
 plan_sha256 TEXT NOT NULL,
 progress_sha256 TEXT NOT NULL,
 payload TEXT NOT NULL,
 FOREIGN KEY(version_id,workflow_id,project_id) REFERENCES workflow_versions(id,workflow_id,project_id)
);
