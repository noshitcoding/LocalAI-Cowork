-- A retained prefix checkpoint is the original last removed digest, never a
-- replacement chain. Frozen exports are recoverable after a lost HTTP response.
CREATE TABLE management_audit_checkpoints (
    organisation_id UUID PRIMARY KEY REFERENCES management_organisations(id),
    revision BIGINT NOT NULL DEFAULT 0 CHECK (revision >= 0),
    sequence BIGINT NOT NULL DEFAULT 0 CHECK (sequence >= 0),
    digest TEXT NOT NULL DEFAULT repeat('0',64) CHECK (length(digest)=64),
    witness_receipt JSONB
);
CREATE TABLE management_audit_exports (
    id UUID PRIMARY KEY,
    organisation_id UUID NOT NULL REFERENCES management_organisations(id),
    actor_user_id UUID NOT NULL REFERENCES users(id),
    request_id UUID NOT NULL,
    request_sha256 TEXT NOT NULL,
    base_revision BIGINT NOT NULL,
    export_sha256 TEXT NOT NULL,
    document JSONB NOT NULL,
    retain_until TIMESTAMPTZ NOT NULL,
    witness_receipt JSONB,
    final_receipt JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE (organisation_id,actor_user_id,request_id)
);
CREATE INDEX management_audit_exports_org ON management_audit_exports(organisation_id,created_at);
