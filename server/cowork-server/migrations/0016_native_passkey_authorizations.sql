CREATE TABLE native_passkey_authorizations (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_id UUID NOT NULL,
    code_challenge TEXT NOT NULL,
    client_state TEXT NOT NULL,
    redirect_uri TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ
);

ALTER TABLE webauthn_challenges
    ADD COLUMN authorization_id UUID REFERENCES native_passkey_authorizations(id) ON DELETE CASCADE;

ALTER TABLE webauthn_challenges
    DROP CONSTRAINT webauthn_challenges_kind_check,
    ADD CONSTRAINT webauthn_challenges_kind_check
        CHECK (kind IN ('registration', 'authentication', 'native_authentication'));

CREATE INDEX native_passkey_authorizations_expiry_idx
    ON native_passkey_authorizations (expires_at) WHERE consumed_at IS NULL;

CREATE INDEX webauthn_challenges_authorization_idx
    ON webauthn_challenges (authorization_id) WHERE authorization_id IS NOT NULL;
