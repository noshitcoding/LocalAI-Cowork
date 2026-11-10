CREATE TABLE passkeys (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    credential_id BYTEA NOT NULL UNIQUE,
    label TEXT NOT NULL,
    credential JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ
);

CREATE INDEX passkeys_user_idx ON passkeys (user_id, created_at);

CREATE TABLE webauthn_challenges (
    id UUID PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('registration', 'authentication')),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_id UUID,
    state JSONB NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    used_at TIMESTAMPTZ
);

CREATE INDEX webauthn_challenges_expiry_idx
    ON webauthn_challenges (expires_at)
    WHERE used_at IS NULL;
