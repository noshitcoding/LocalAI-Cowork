CREATE TABLE owner_execution_plans_v2 (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id),
    order_id TEXT NOT NULL REFERENCES orders(id),
    idempotency_key TEXT NOT NULL,
    request_sha256 TEXT NOT NULL,
    plan_sha256 TEXT NOT NULL,
    payload TEXT NOT NULL,
    run_id TEXT UNIQUE REFERENCES runs(id),
    receipt TEXT,
    retired BIGINT NOT NULL DEFAULT 0 CHECK(retired IN (0,1)),
    CHECK ((run_id IS NULL AND receipt IS NULL) OR (run_id IS NOT NULL AND receipt IS NOT NULL)),
    CHECK (retired=0 OR run_id IS NULL),
    UNIQUE(order_id, idempotency_key)
);
