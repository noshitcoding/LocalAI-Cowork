-- Immutable locally prepared input, with native origin custody kept private.
-- This table grants neither transfer nor execution authority.
CREATE TABLE owner_execution_inputs (
    id TEXT PRIMARY KEY,
    order_id TEXT NOT NULL REFERENCES orders(id),
    project_id TEXT NOT NULL REFERENCES projects(id),
    idempotency_key TEXT NOT NULL,
    request_sha256 TEXT NOT NULL,
    request_payload TEXT NOT NULL,
    snapshot_sha256 TEXT NOT NULL,
    custody_sha256 TEXT NOT NULL,
    retired BIGINT NOT NULL DEFAULT 0 CHECK(retired IN (0,1)),
    payload TEXT NOT NULL,
    UNIQUE(order_id,idempotency_key)
);
