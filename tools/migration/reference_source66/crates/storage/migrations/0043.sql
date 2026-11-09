CREATE TABLE android_hub_connections (
 executor_id TEXT PRIMARY KEY, node_id TEXT NOT NULL, boot_nonce TEXT NOT NULL,
 connection_nonce TEXT NOT NULL, epoch BIGINT NOT NULL, origin TEXT,
 credential_id TEXT NOT NULL, credential_revision BIGINT NOT NULL,
 device_revision BIGINT NOT NULL, owner_id TEXT NOT NULL, expires_at BIGINT NOT NULL
);
CREATE TABLE android_hub_streams (
 id TEXT PRIMARY KEY, executor_id TEXT NOT NULL, connection_nonce TEXT NOT NULL,
 epoch BIGINT NOT NULL, access TEXT NOT NULL, expires_at BIGINT NOT NULL
);
CREATE INDEX android_hub_stream_expiry ON android_hub_streams(expires_at);
CREATE TABLE android_hub_forwards (
 token_sha256 TEXT PRIMARY KEY, executor_id TEXT NOT NULL,
 connection_nonce TEXT NOT NULL, epoch BIGINT NOT NULL,
 kind TEXT NOT NULL, access TEXT NOT NULL, agent TEXT, stream_id TEXT,
 expires_at BIGINT NOT NULL, consumed BIGINT NOT NULL DEFAULT 0
);
CREATE INDEX android_hub_forward_expiry ON android_hub_forwards(expires_at);
