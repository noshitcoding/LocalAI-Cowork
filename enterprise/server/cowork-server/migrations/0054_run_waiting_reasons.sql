-- Admission diagnostics contain only a reason code and a database check time.
-- Keep them separate from the shared run/event contracts.
CREATE TABLE run_waiting_reasons (
    run_id uuid PRIMARY KEY REFERENCES runs(id) ON DELETE CASCADE,
    reason text NOT NULL CHECK (reason IN (
        'platform_capacity', 'organisation_capacity', 'user_capacity',
        'instance_capacity', 'chat_workspace_busy', 'admission_unavailable', 'runner_capacity'
    )),
    checked_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    lease_owner uuid,
    lease_token uuid,
    CHECK (
        (reason = 'runner_capacity' AND lease_owner IS NOT NULL AND lease_token IS NOT NULL)
        OR (reason <> 'runner_capacity' AND lease_owner IS NULL AND lease_token IS NULL)
    )
);

CREATE FUNCTION clear_run_waiting_reason() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.state NOT IN ('queued', 'waiting_for_executor', 'waiting_for_snapshot') THEN
        DELETE FROM run_waiting_reasons waiting WHERE waiting.run_id = NEW.id
          AND (waiting.reason <> 'runner_capacity' OR NEW.state <> 'running'
               OR waiting.lease_owner IS DISTINCT FROM NEW.lease_owner
               OR waiting.lease_token IS DISTINCT FROM NEW.lease_token
               OR NEW.lease_expires_at IS NULL OR NEW.lease_expires_at <= clock_timestamp());
    END IF;
    -- A queue transition must never retain a prior running lease's diagnostic.
    IF NEW.state <> 'running' THEN
        DELETE FROM run_waiting_reasons WHERE run_id = NEW.id AND reason = 'runner_capacity';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER runs_clear_waiting_reason
AFTER UPDATE OF state, lease_owner, lease_token, lease_expires_at ON runs
FOR EACH ROW EXECUTE FUNCTION clear_run_waiting_reason();
