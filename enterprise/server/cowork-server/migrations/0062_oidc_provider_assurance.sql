-- Additive: legacy sessions and pending codes have no provider assurance.
ALTER TABLE auth_sessions ADD COLUMN oidc_assurance JSONB;
ALTER TABLE native_authorization_codes ADD COLUMN oidc_assurance JSONB;
ALTER TABLE auth_sessions ADD CONSTRAINT auth_sessions_oidc_assurance_object
    CHECK (oidc_assurance IS NULL OR jsonb_typeof(oidc_assurance)='object');
ALTER TABLE native_authorization_codes ADD CONSTRAINT native_codes_oidc_assurance_object
    CHECK (oidc_assurance IS NULL OR jsonb_typeof(oidc_assurance)='object');

-- The operator's stable scope is a single global provider or one explicitly
-- named organisation provider. Epoch/revision fence old replicas and sessions.
CREATE TABLE oidc_assurance_policies (
    scope TEXT PRIMARY KEY,
    issuer TEXT NOT NULL,
    client_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL CHECK (fingerprint ~ '^[0-9a-f]{64}$'),
    revision BIGINT NOT NULL CHECK (revision > 0),
    epoch UUID NOT NULL,
    active BOOLEAN NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE oidc_assurance_policy_operations (
    id UUID PRIMARY KEY,
    scope TEXT NOT NULL REFERENCES oidc_assurance_policies(scope),
    expected_revision BIGINT NOT NULL CHECK (expected_revision > 0),
    request_sha256 TEXT NOT NULL CHECK (request_sha256 ~ '^[0-9a-f]{64}$'),
    receipt JSONB NOT NULL CHECK (jsonb_typeof(receipt)='object'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (scope, expected_revision)
);
