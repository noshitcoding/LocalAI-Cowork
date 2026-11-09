CREATE TABLE owner_execution_plans_v2 (
 id TEXT PRIMARY KEY,
 order_id TEXT NOT NULL REFERENCES orders(id),
 project_id TEXT NOT NULL REFERENCES projects(id),
 idempotency_key TEXT NOT NULL,
 plan_sha256 TEXT NOT NULL,
 custody_sha256 TEXT NOT NULL,
 payload TEXT NOT NULL,
 retired BIGINT NOT NULL DEFAULT 0 CHECK(retired IN (0,1)),
 run_id TEXT REFERENCES runs(id),
 receipt TEXT,
 UNIQUE(order_id,idempotency_key)
);
CREATE TABLE owner_run_workspaces (
 workspace_id TEXT PRIMARY KEY,
 plan_id TEXT NOT NULL UNIQUE REFERENCES owner_execution_plans_v2(id),
 run_id TEXT NOT NULL UNIQUE REFERENCES runs(id),
 custody_sha256 TEXT NOT NULL,
 payload TEXT NOT NULL
);
