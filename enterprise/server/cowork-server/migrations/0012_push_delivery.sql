CREATE TABLE push_subscriptions (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_id UUID NOT NULL,
    provider TEXT NOT NULL CHECK (provider IN ('fcm', 'web_push')),
    endpoint_hash BYTEA NOT NULL,
    ciphertext BYTEA NOT NULL,
    encrypted_data_key BYTEA NOT NULL,
    nonce BYTEA NOT NULL,
    wrap_nonce BYTEA NOT NULL,
    failures INTEGER NOT NULL DEFAULT 0 CHECK (failures >= 0),
    last_error TEXT,
    last_success_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ,
    UNIQUE (user_id, provider, endpoint_hash)
);

CREATE INDEX push_subscriptions_user_active_idx
    ON push_subscriptions (user_id, device_id)
    WHERE revoked_at IS NULL;

CREATE TABLE push_deliveries (
    id UUID PRIMARY KEY,
    run_id UUID NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    event_sequence BIGINT NOT NULL,
    event_kind TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'queued'
        CHECK (state IN ('queued', 'processing', 'delivered', 'failed')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    delivered_at TIMESTAMPTZ,
    UNIQUE (user_id, run_id, event_sequence),
    FOREIGN KEY (run_id, event_sequence)
        REFERENCES run_events(run_id, sequence) ON DELETE CASCADE
);

CREATE INDEX push_deliveries_dispatch_idx
    ON push_deliveries (next_attempt_at, created_at)
    WHERE state IN ('queued', 'processing');
