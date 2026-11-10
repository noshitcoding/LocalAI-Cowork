CREATE TABLE model_registry_guard (id BIGINT PRIMARY KEY, revision BIGINT NOT NULL);
INSERT INTO model_registry_guard(id,revision) VALUES(1,0);
CREATE TABLE model_registry (id TEXT NOT NULL, version BIGINT NOT NULL, provider_id TEXT NOT NULL, provider_sha256 TEXT NOT NULL, bundle_sha256 TEXT NOT NULL, payload TEXT NOT NULL, registered_at BIGINT NOT NULL, revoked BIGINT NOT NULL DEFAULT 0, PRIMARY KEY(id,version));
CREATE INDEX model_registry_provider ON model_registry(provider_id,version);
CREATE TABLE model_registry_operations (id TEXT PRIMARY KEY, request_sha256 TEXT NOT NULL, receipt TEXT NOT NULL);
CREATE TABLE context_selections (order_id TEXT PRIMARY KEY, revision BIGINT NOT NULL, payload TEXT NOT NULL);
CREATE TABLE context_archives (id TEXT PRIMARY KEY, order_id TEXT NOT NULL, revision BIGINT NOT NULL, request_sha256 TEXT NOT NULL, payload_sha256 TEXT NOT NULL, payload TEXT NOT NULL, created_at BIGINT NOT NULL);
CREATE INDEX context_archives_order ON context_archives(order_id,revision);
CREATE TABLE context_operations (order_id TEXT NOT NULL, idempotency_key TEXT NOT NULL, request_sha256 TEXT NOT NULL, archive_id TEXT NOT NULL, PRIMARY KEY(order_id,idempotency_key));
