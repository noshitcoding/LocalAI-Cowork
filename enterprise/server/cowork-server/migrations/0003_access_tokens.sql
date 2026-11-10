CREATE TABLE access_tokens (
    token_hash BYTEA PRIMARY KEY,
    session_id UUID NOT NULL REFERENCES auth_sessions(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ
);

CREATE INDEX access_tokens_user_expiry_idx ON access_tokens (user_id, expires_at)
    WHERE revoked_at IS NULL;
