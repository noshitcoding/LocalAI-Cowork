CREATE TABLE executor_credentials (
    id UUID PRIMARY KEY,
    executor_id UUID NOT NULL REFERENCES executors(id) ON DELETE CASCADE,
    token_hash BYTEA NOT NULL UNIQUE,
    label TEXT NOT NULL,
    created_by_user_id UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    revoke_reason TEXT
);

CREATE INDEX executor_credentials_active_idx
    ON executor_credentials (executor_id, expires_at)
    WHERE revoked_at IS NULL;
