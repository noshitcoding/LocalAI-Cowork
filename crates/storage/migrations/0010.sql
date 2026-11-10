CREATE TABLE environments (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id),
    idempotency_key TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    revision BIGINT NOT NULL,
    payload TEXT NOT NULL,
    UNIQUE(project_id,idempotency_key)
);
