CREATE TABLE auth_refresh_token_history (
    token_hash BYTEA PRIMARY KEY,
    session_id UUID NOT NULL REFERENCES auth_sessions(id) ON DELETE CASCADE,
    refresh_family_id UUID NOT NULL,
    rotated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX auth_refresh_token_history_family_idx
    ON auth_refresh_token_history (refresh_family_id, rotated_at);

-- Preserve replay detection for the immediately previous token held by an
-- installation upgraded in place. Future rotations retain every ancestor.
INSERT INTO auth_refresh_token_history (token_hash, session_id, refresh_family_id, rotated_at)
SELECT previous_token_hash, id, refresh_family_id, last_used_at
FROM auth_sessions
WHERE previous_token_hash IS NOT NULL
ON CONFLICT (token_hash) DO NOTHING;
