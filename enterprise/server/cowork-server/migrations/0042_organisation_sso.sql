CREATE TABLE management_sso (
    organisation_id UUID PRIMARY KEY REFERENCES management_organisations(id),
    revision BIGINT NOT NULL CHECK(revision>0),
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    provider_id TEXT,
    provider_fingerprint TEXT,
    auto_provision BOOLEAN NOT NULL DEFAULT FALSE,
    group_mappings JSONB NOT NULL DEFAULT '[]',
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK(NOT enabled OR (provider_id IS NOT NULL AND provider_fingerprint IS NOT NULL))
);
ALTER TABLE management_members DROP CONSTRAINT management_members_role_check;
ALTER TABLE management_members ADD CONSTRAINT management_members_role_check
    CHECK(role IN ('admin','member','auditor','suspended'));
ALTER TABLE management_members ADD COLUMN sso_provider_id TEXT;
ALTER TABLE management_members ADD CONSTRAINT management_suspended_requires_sso
    CHECK(role<>'suspended' OR sso_provider_id IS NOT NULL);
ALTER TABLE oidc_authorizations ADD COLUMN organisation_id UUID REFERENCES management_organisations(id);
ALTER TABLE oidc_authorizations ADD COLUMN organisation_sso_revision BIGINT;
ALTER TABLE oidc_authorizations ADD COLUMN organisation_provider_fingerprint TEXT;
