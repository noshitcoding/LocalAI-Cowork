ALTER TABLE schedules
    ADD COLUMN input JSONB NOT NULL DEFAULT '{}'::jsonb,
    ADD COLUMN model_profile_id UUID REFERENCES provider_profiles(id);

CREATE TABLE run_input_requests (
    id UUID PRIMARY KEY,
    run_id UUID NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    revision BIGINT NOT NULL DEFAULT 1,
    etag TEXT NOT NULL,
    prompt JSONB NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending', 'submitted', 'expired')),
    response JSONB,
    requested_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    responded_by UUID REFERENCES users(id),
    responded_at TIMESTAMPTZ
);
CREATE INDEX run_input_requests_pending_idx ON run_input_requests (expires_at)
    WHERE state = 'pending';
CREATE UNIQUE INDEX run_input_requests_one_pending_per_run
    ON run_input_requests (run_id) WHERE state = 'pending';

CREATE TABLE run_checkpoints (
    id UUID PRIMARY KEY,
    run_id UUID NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    sequence BIGINT NOT NULL,
    safe_to_resume BOOLEAN NOT NULL,
    executor_state JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (run_id, sequence)
);
CREATE INDEX run_checkpoints_run_idx ON run_checkpoints (run_id, sequence DESC);

ALTER TABLE approval_requests
    ADD COLUMN requested_by_executor_id UUID REFERENCES executors(id);
CREATE UNIQUE INDEX approval_requests_one_pending_per_run
    ON approval_requests (run_id) WHERE state = 'pending';
