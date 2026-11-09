CREATE TABLE document_preparations (
 id TEXT PRIMARY KEY,
 order_id TEXT NOT NULL REFERENCES orders(id),
 idempotency_key TEXT NOT NULL,
 request_hash TEXT NOT NULL,
 payload TEXT NOT NULL,
 UNIQUE(order_id,idempotency_key)
);
CREATE TABLE document_drafts (
 id TEXT NOT NULL,
 order_id TEXT NOT NULL REFERENCES orders(id),
 revision BIGINT NOT NULL,
 payload TEXT NOT NULL,
 PRIMARY KEY(order_id,id)
);
CREATE TABLE document_draft_saves (
 order_id TEXT NOT NULL REFERENCES orders(id),
 idempotency_key TEXT NOT NULL,
 request_hash TEXT NOT NULL,
 payload TEXT,
 PRIMARY KEY(order_id,idempotency_key)
);
CREATE TABLE document_draft_snapshots (
 id TEXT PRIMARY KEY,
 order_id TEXT NOT NULL REFERENCES orders(id),
 draft_id TEXT NOT NULL,
 created_at BIGINT NOT NULL,
 payload TEXT NOT NULL
);
CREATE INDEX document_draft_history ON document_draft_snapshots(order_id,draft_id,created_at,id);
