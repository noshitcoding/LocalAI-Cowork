CREATE TABLE team_container_recovery_runs (
    run_id TEXT PRIMARY KEY REFERENCES team_runs(id),
    revision BIGINT NOT NULL CHECK(revision >= 0)
);
CREATE TABLE team_container_recovery_operations (
    run_id TEXT NOT NULL REFERENCES team_runs(id),
    operation_id TEXT NOT NULL,
    actor_id TEXT NOT NULL REFERENCES identity_subjects(id),
    request_sha256 TEXT NOT NULL,
    plan_sha256 TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('admitted','completed','rejected')),
    payload TEXT NOT NULL,
    receipt TEXT,
    created_at BIGINT NOT NULL,
    PRIMARY KEY(run_id, operation_id)
);
CREATE UNIQUE INDEX team_container_recovery_one_open
    ON team_container_recovery_operations(run_id) WHERE state='admitted';
