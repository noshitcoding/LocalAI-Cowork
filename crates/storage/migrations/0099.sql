CREATE TABLE sandbox_workspace_guards(resource_id TEXT PRIMARY KEY, revision BIGINT NOT NULL);
CREATE TABLE sandbox_operations(
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES projects(id),
 order_id TEXT NOT NULL REFERENCES orders(id),
 resource_id TEXT NOT NULL,
 idempotency_key TEXT NOT NULL,
 request_hash TEXT NOT NULL,
 operation TEXT NOT NULL,
 state TEXT NOT NULL,
 request TEXT NOT NULL,
 result TEXT,
 sequence BIGINT NOT NULL,
 created_at BIGINT NOT NULL,
 UNIQUE(project_id,order_id,idempotency_key)
);
CREATE INDEX sandbox_operations_workspace ON sandbox_operations(resource_id,state);
