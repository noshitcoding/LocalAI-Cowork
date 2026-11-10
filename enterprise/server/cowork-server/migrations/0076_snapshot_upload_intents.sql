-- An original upload can be recovered after a lost begin response without
-- reserving quota or creating a second manifest. Existing clients remain valid.
CREATE TABLE snapshot_upload_intents (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    idempotency_key UUID NOT NULL,
    manifest_id UUID NOT NULL REFERENCES snapshot_manifests(id) ON DELETE CASCADE,
    request_sha256 BYTEA NOT NULL CHECK (octet_length(request_sha256) = 32),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, idempotency_key)
);
