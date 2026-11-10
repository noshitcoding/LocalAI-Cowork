-- Generic metadata materialization for the device Outbox/Inbox protocol. File
-- contents and device-local bindings remain outside this table by contract.
CREATE TABLE sync_entities (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    entity_type TEXT NOT NULL CHECK (char_length(entity_type) BETWEEN 1 AND 64),
    entity_id UUID NOT NULL,
    revision BIGINT NOT NULL CHECK (revision >= 1),
    etag TEXT NOT NULL,
    payload JSONB,
    tombstone BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, entity_type, entity_id),
    CHECK ((tombstone AND payload IS NULL) OR (NOT tombstone AND payload IS NOT NULL))
);

CREATE INDEX sync_entities_user_updated_idx
    ON sync_entities (user_id, updated_at DESC, entity_type, entity_id);
