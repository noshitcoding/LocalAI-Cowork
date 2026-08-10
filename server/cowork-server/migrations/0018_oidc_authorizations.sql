ALTER TABLE oidc_identities
    ADD COLUMN IF NOT EXISTS email_at_link TEXT,
    ADD COLUMN IF NOT EXISTS last_login_at TIMESTAMPTZ;

CREATE TABLE oidc_authorizations (
    id UUID PRIMARY KEY,
    state_hash BYTEA NOT NULL UNIQUE,
    nonce TEXT NOT NULL,
    provider_pkce_verifier TEXT NOT NULL,
    device_id UUID NOT NULL,
    client_code_challenge TEXT NOT NULL,
    client_state TEXT NOT NULL,
    client_redirect_uri TEXT NOT NULL,
    link_user_id UUID REFERENCES users(id) ON DELETE CASCADE,
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ
);

CREATE INDEX oidc_authorizations_expiry_idx
    ON oidc_authorizations (expires_at) WHERE consumed_at IS NULL;
