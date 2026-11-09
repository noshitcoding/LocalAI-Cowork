ALTER TABLE management_instances ADD COLUMN current_release TEXT
    CHECK (current_release ~ '^[a-f0-9]{64}$');

CREATE TABLE management_releases (
    id TEXT PRIMARY KEY CHECK (id ~ '^[a-f0-9]{64}$'),
    manifest JSONB NOT NULL,
    active BOOLEAN NOT NULL DEFAULT TRUE,
    refreshed_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE management_release_qualifications (
    release_id TEXT NOT NULL REFERENCES management_releases(id),
    source_release TEXT NOT NULL CHECK (source_release ~ '^[a-f0-9]{64}$'),
    operation_id UUID NOT NULL REFERENCES management_operations(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (release_id, source_release)
);
