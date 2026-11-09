-- Private execution identities precede every runner POST. A canceled HTTP
-- future is not evidence that its Docker execution stopped.
CREATE TABLE run_runner_attempts (
    id UUID PRIMARY KEY,
    run_id UUID NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    lease_owner UUID NOT NULL,
    lease_token UUID NOT NULL,
    workspace_id UUID,
    request_sha256 TEXT NOT NULL CHECK (request_sha256 ~ '^[0-9a-f]{64}$'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX run_runner_attempts_run ON run_runner_attempts(run_id, id);
ALTER TABLE runs ADD COLUMN runner_cancel_id UUID;
ALTER TABLE runs ADD COLUMN runner_cancel_lease UUID;
ALTER TABLE runs ADD COLUMN runner_stop_receipt JSONB;
CREATE FUNCTION immutable_runner_attempt() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'runner attempt bindings are immutable';
END;
$$;
CREATE TRIGGER immutable_runner_attempt BEFORE UPDATE ON run_runner_attempts
    FOR EACH ROW EXECUTE FUNCTION immutable_runner_attempt();
