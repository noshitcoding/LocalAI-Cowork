CREATE TABLE user_totp (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    ciphertext BYTEA NOT NULL,
    encrypted_data_key BYTEA NOT NULL,
    nonce BYTEA NOT NULL CHECK (octet_length(nonce) = 12),
    wrap_nonce BYTEA NOT NULL CHECK (octet_length(wrap_nonce) = 12),
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    last_used_step BIGINT,
    pending_expires_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    enabled_at TIMESTAMPTZ
);

CREATE TABLE recovery_codes (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_hash BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    used_at TIMESTAMPTZ,
    UNIQUE (user_id, code_hash)
);

CREATE INDEX recovery_codes_unused_idx ON recovery_codes (user_id) WHERE used_at IS NULL;
