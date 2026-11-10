ALTER TABLE snapshot_manifests
    ADD COLUMN upload_expires_at TIMESTAMPTZ NOT NULL DEFAULT (now() + interval '24 hours');

CREATE TABLE snapshot_chunk_reservations (
    manifest_id UUID NOT NULL REFERENCES snapshot_manifests(id) ON DELETE CASCADE,
    key_scope_type TEXT NOT NULL,
    key_scope_id UUID NOT NULL,
    plaintext_digest BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (manifest_id, key_scope_type, key_scope_id, plaintext_digest),
    FOREIGN KEY (key_scope_type, key_scope_id, plaintext_digest)
        REFERENCES snapshot_chunks(key_scope_type, key_scope_id, plaintext_digest)
        ON DELETE CASCADE
);
CREATE INDEX snapshot_chunk_reservations_chunk_idx
    ON snapshot_chunk_reservations (key_scope_type, key_scope_id, plaintext_digest);
