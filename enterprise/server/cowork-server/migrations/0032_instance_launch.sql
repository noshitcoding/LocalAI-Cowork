-- Replay protection lives in the instance database; no user content is centralised.
CREATE TABLE instance_launch_nonces (
    nonce UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id),
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
