-- Process liveness is separate from queue/runner health. A fresh pulse does not
-- certify that a scheduler, executor or a new release has been qualified.
CREATE TABLE server_process_health (
    process_id uuid PRIMARY KEY,
    process_mode text NOT NULL CHECK (process_mode IN ('api','worker','all','management')),
    build_version text NOT NULL,
    migration_version bigint NOT NULL,
    lifecycle text NOT NULL CHECK (lifecycle IN ('running','draining','stopped')),
    started_at timestamptz NOT NULL,
    last_seen_at timestamptz NOT NULL,
    observations jsonb NOT NULL CHECK (jsonb_typeof(observations) = 'object')
);
CREATE INDEX server_process_health_last_seen ON server_process_health(last_seen_at);
