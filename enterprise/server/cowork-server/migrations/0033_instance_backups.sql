CREATE TABLE management_backups (
    id UUID PRIMARY KEY,
    organisation_id UUID NOT NULL,
    instance_id UUID NOT NULL,
    operation_id UUID NOT NULL UNIQUE REFERENCES management_operations(id),
    sha256 TEXT NOT NULL CHECK (sha256 ~ '^[a-f0-9]{64}$'),
    size_bytes BIGINT NOT NULL CHECK (size_bytes > 0),
    manifest_version INTEGER NOT NULL CHECK (manifest_version = 1),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (organisation_id,instance_id) REFERENCES management_instances(organisation_id,id)
);
CREATE INDEX management_backups_instance ON management_backups(organisation_id,instance_id,created_at);
