CREATE TABLE runs (
    id UUID PRIMARY KEY,
    thread_id UUID NOT NULL,
    project_id UUID NOT NULL,
    creator_user_id UUID NOT NULL,
    idempotency_key TEXT NOT NULL,
    target_kind TEXT NOT NULL CHECK (target_kind IN ('server_linux', 'managed_windows_pool', 'personal_device')),
    target_pool_id UUID,
    target_device_id UUID,
    state TEXT NOT NULL CHECK (state IN (
        'queued', 'waiting_for_executor', 'waiting_for_snapshot', 'running',
        'waiting_approval', 'waiting_input', 'interrupted', 'completed',
        'failed', 'canceled', 'expired'
    )),
    revision BIGINT NOT NULL DEFAULT 1,
    spec JSONB NOT NULL,
    result JSONB,
    error JSONB,
    assigned_executor_id UUID,
    lease_owner UUID,
    lease_token UUID,
    lease_expires_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    CONSTRAINT runs_idempotency UNIQUE (creator_user_id, idempotency_key)
);

CREATE INDEX runs_dispatch_idx
    ON runs (target_kind, state, created_at)
    WHERE state IN ('queued', 'waiting_for_executor');
CREATE INDEX runs_thread_idx ON runs (thread_id, created_at);
CREATE INDEX runs_lease_idx ON runs (lease_expires_at) WHERE state = 'running';

CREATE TABLE run_events (
    run_id UUID NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    sequence BIGINT NOT NULL,
    event_id UUID NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (run_id, sequence)
);

CREATE TABLE executor_pools (
    id UUID PRIMARY KEY,
    revision BIGINT NOT NULL DEFAULT 1,
    etag TEXT NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('server_linux', 'managed_windows', 'personal_device')),
    team_id UUID,
    policy JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE TABLE executors (
    id UUID PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('server_linux', 'managed_windows', 'personal_device')),
    pool_id UUID REFERENCES executor_pools(id),
    owner_user_id UUID,
    registration JSONB NOT NULL,
    protocol_version INTEGER NOT NULL,
    max_concurrent_runs INTEGER NOT NULL CHECK (max_concurrent_runs > 0),
    active_runs INTEGER NOT NULL DEFAULT 0 CHECK (active_runs >= 0),
    draining BOOLEAN NOT NULL DEFAULT FALSE,
    last_seen_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX executors_routing_idx ON executors (kind, pool_id, draining, last_seen_at);

CREATE TABLE audit_events (
    id UUID PRIMARY KEY,
    actor_user_id UUID,
    actor_executor_id UUID,
    action TEXT NOT NULL,
    target_type TEXT NOT NULL,
    target_id UUID,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX audit_events_target_idx ON audit_events (target_type, target_id, created_at);
