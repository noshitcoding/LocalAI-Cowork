-- Trusted agent cursors cannot be minted by a user or an external executor.
CREATE TABLE run_server_execution_attempts (
    id uuid PRIMARY KEY,
    run_id uuid NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    worker_id uuid NOT NULL,
    lease_token uuid NOT NULL,
    resume_checkpoint_id uuid REFERENCES run_checkpoints(id),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    stop_receipt jsonb,
    UNIQUE(run_id, lease_token),
    UNIQUE(id, run_id, worker_id, lease_token)
);
ALTER TABLE run_checkpoints ADD COLUMN origin text NOT NULL DEFAULT 'untrusted';
ALTER TABLE run_checkpoints ADD COLUMN worker_id uuid;
ALTER TABLE run_checkpoints ADD COLUMN worker_lease_token uuid;
ALTER TABLE run_checkpoints ADD COLUMN server_execution_attempt_id uuid;
ALTER TABLE run_checkpoints ADD CONSTRAINT checkpoint_origin CHECK (
    (origin='untrusted' AND worker_id IS NULL AND worker_lease_token IS NULL AND server_execution_attempt_id IS NULL)
    OR (origin='worker_other' AND worker_id IS NOT NULL AND worker_lease_token IS NOT NULL AND server_execution_attempt_id IS NULL)
    OR (origin='server_agent_v1' AND worker_id IS NOT NULL AND worker_lease_token IS NOT NULL AND server_execution_attempt_id IS NOT NULL)
);
ALTER TABLE run_checkpoints ADD CONSTRAINT checkpoint_worker_attempt FOREIGN KEY
    (server_execution_attempt_id,run_id,worker_id,worker_lease_token)
    REFERENCES run_server_execution_attempts(id,run_id,worker_id,lease_token);
ALTER TABLE runs ADD COLUMN server_resume_checkpoint_id uuid REFERENCES run_checkpoints(id);
CREATE TABLE run_resume_requests (
    request_id uuid PRIMARY KEY,
    run_id uuid NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES users(id),
    checkpoint_id uuid NOT NULL REFERENCES run_checkpoints(id),
    source_revision bigint NOT NULL,
    queued_revision bigint NOT NULL,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    CHECK(queued_revision=source_revision+1)
);
CREATE FUNCTION immutable_server_execution_attempt() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF ROW(NEW.id,NEW.run_id,NEW.worker_id,NEW.lease_token,NEW.resume_checkpoint_id,NEW.created_at)
       IS DISTINCT FROM ROW(OLD.id,OLD.run_id,OLD.worker_id,OLD.lease_token,OLD.resume_checkpoint_id,OLD.created_at)
       OR (OLD.stop_receipt IS NOT NULL AND NEW.stop_receipt IS DISTINCT FROM OLD.stop_receipt) THEN
        RAISE EXCEPTION 'server execution attempt identity and stop proof are immutable';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER immutable_server_execution_attempt BEFORE UPDATE ON run_server_execution_attempts
    FOR EACH ROW EXECUTE FUNCTION immutable_server_execution_attempt();
CREATE FUNCTION immutable_resume_record() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'resume and checkpoint records are immutable';
END;
$$;
CREATE TRIGGER immutable_resume_request BEFORE UPDATE ON run_resume_requests
    FOR EACH ROW EXECUTE FUNCTION immutable_resume_record();
CREATE TRIGGER immutable_run_checkpoint BEFORE UPDATE ON run_checkpoints
    FOR EACH ROW EXECUTE FUNCTION immutable_resume_record();
