-- Records which canonical control-plane rows participate in the personal
-- metadata bridge, regardless of which side created them. This provenance
-- boundary prevents a globally unique sync ID from ever overwriting an
-- independent server or team object.
CREATE TABLE sync_materializations (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    entity_type TEXT NOT NULL CHECK (entity_type IN ('project', 'thread', 'message')),
    entity_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, entity_type, entity_id)
);

CREATE INDEX sync_materializations_entity_idx
    ON sync_materializations (entity_type, entity_id);
