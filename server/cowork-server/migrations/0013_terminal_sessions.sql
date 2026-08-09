CREATE TABLE terminal_sessions (
    id UUID PRIMARY KEY,
    run_id UUID NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    state TEXT NOT NULL CHECK (state IN ('created', 'connected', 'ended', 'failed')),
    columns INTEGER NOT NULL CHECK (columns BETWEEN 20 AND 400),
    rows INTEGER NOT NULL CHECK (rows BETWEEN 5 AND 200),
    input_bytes BIGINT NOT NULL DEFAULT 0 CHECK (input_bytes >= 0),
    output_bytes BIGINT NOT NULL DEFAULT 0 CHECK (output_bytes >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    connected_at TIMESTAMPTZ,
    ended_at TIMESTAMPTZ,
    failure TEXT
);

CREATE INDEX terminal_sessions_run_idx ON terminal_sessions (run_id, created_at);

CREATE TABLE terminal_stream_tickets (
    token_hash BYTEA PRIMARY KEY,
    terminal_session_id UUID NOT NULL REFERENCES terminal_sessions(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at TIMESTAMPTZ NOT NULL,
    used_at TIMESTAMPTZ
);

CREATE INDEX terminal_stream_tickets_expiry_idx
    ON terminal_stream_tickets (expires_at)
    WHERE used_at IS NULL;
