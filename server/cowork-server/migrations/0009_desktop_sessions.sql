-- desktop_sessions was introduced by migration 0002 as a domain placeholder.
-- Turn it into the executable Linux-GUI session model without recreating the
-- table, so upgrades from every released schema remain non-destructive.
ALTER TABLE desktop_sessions
    DROP CONSTRAINT IF EXISTS desktop_sessions_executor_id_fkey,
    ALTER COLUMN stream_protocol SET DEFAULT 'rfb.binary.v1',
    ADD COLUMN IF NOT EXISTS runner_metadata JSONB NOT NULL DEFAULT '{}'::jsonb;

CREATE UNIQUE INDEX desktop_sessions_active_run
    ON desktop_sessions(run_id)
    WHERE state IN ('starting', 'agent_controlled', 'user_controlled', 'paused');

CREATE TABLE reauthentication_grants (
    token_hash BYTEA PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    session_id UUID NOT NULL REFERENCES auth_sessions(id) ON DELETE CASCADE,
    purpose TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    used_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX reauthentication_grants_expiry
    ON reauthentication_grants(expires_at)
    WHERE used_at IS NULL;

CREATE TABLE desktop_stream_tickets (
    token_hash BYTEA PRIMARY KEY,
    desktop_session_id UUID NOT NULL REFERENCES desktop_sessions(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    control BOOLEAN NOT NULL DEFAULT FALSE,
    expires_at TIMESTAMPTZ NOT NULL,
    used_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX desktop_stream_tickets_expiry
    ON desktop_stream_tickets(expires_at)
    WHERE used_at IS NULL;
