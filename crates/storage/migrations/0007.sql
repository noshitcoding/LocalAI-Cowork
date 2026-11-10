CREATE TABLE file_uploads (
    id TEXT PRIMARY KEY,
    order_id TEXT NOT NULL REFERENCES orders(id),
    idempotency_key TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    revision BIGINT NOT NULL,
    created_at BIGINT NOT NULL,
    payload TEXT NOT NULL,
    UNIQUE(order_id, idempotency_key)
);
CREATE INDEX file_uploads_order ON file_uploads(order_id, created_at, id);
