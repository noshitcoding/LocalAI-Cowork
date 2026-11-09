ALTER TABLE messages ADD COLUMN position BIGINT NOT NULL DEFAULT 0;
ALTER TABLE runs ADD COLUMN updated_at BIGINT NOT NULL DEFAULT 0;
CREATE TABLE order_history (order_id TEXT PRIMARY KEY REFERENCES orders(id), source_order_id TEXT NOT NULL REFERENCES orders(id), idempotency_key TEXT NOT NULL, request_hash TEXT NOT NULL, payload TEXT NOT NULL, UNIQUE(source_order_id,idempotency_key));
CREATE TABLE search_documents (id TEXT PRIMARY KEY, order_id TEXT NOT NULL REFERENCES orders(id), kind TEXT NOT NULL, original TEXT NOT NULL, normalized TEXT NOT NULL, created_at BIGINT NOT NULL);
CREATE INDEX search_documents_order ON search_documents(order_id);
CREATE INDEX search_documents_created ON search_documents(created_at,id);
CREATE INDEX runs_activity ON runs(state,updated_at,id);
