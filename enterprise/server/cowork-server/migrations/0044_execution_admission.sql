-- Execution generations fence old restored runtimes without forgiving their
-- outstanding reservations. No timeout or cascading delete releases capacity.
ALTER TABLE management_instances ADD COLUMN execution_epoch UUID NOT NULL DEFAULT gen_random_uuid();

CREATE TABLE management_execution_ceiling_revisions (
    revision BIGINT PRIMARY KEY CHECK (revision > 0),
    settings JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE management_execution_limits (
    organisation_id UUID PRIMARY KEY REFERENCES management_organisations(id),
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    organisation_concurrent_runs INTEGER NOT NULL CHECK (organisation_concurrent_runs > 0),
    user_concurrent_runs INTEGER NOT NULL CHECK (user_concurrent_runs > 0)
);

CREATE TABLE management_execution_reservations (
    id UUID PRIMARY KEY,
    organisation_id UUID NOT NULL,
    instance_id UUID NOT NULL,
    execution_epoch UUID NOT NULL,
    run_id UUID NOT NULL,
    user_id UUID NOT NULL REFERENCES users(id),
    target JSONB NOT NULL,
    state TEXT NOT NULL DEFAULT 'held' CHECK (state IN ('held','released')),
    completion JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    released_at TIMESTAMPTZ,
    UNIQUE(instance_id,execution_epoch,run_id),
    FOREIGN KEY (organisation_id,instance_id) REFERENCES management_instances(organisation_id,id),
    CHECK ((state='held' AND completion IS NULL AND released_at IS NULL)
        OR (state='released' AND completion IS NOT NULL AND released_at IS NOT NULL))
);
CREATE INDEX execution_reservations_held ON management_execution_reservations(organisation_id,user_id,instance_id) WHERE state='held';

-- Instance-local durable receipt relay. These rows are written in the same
-- transaction as the local lease/completion, and never inferred by a reaper.
CREATE TABLE instance_execution_reservations (
    execution_epoch UUID NOT NULL,
    run_id UUID NOT NULL REFERENCES runs(id),
    reservation_id UUID NOT NULL UNIQUE,
    lease_token UUID NOT NULL,
    completion JSONB,
    acknowledged_at TIMESTAMPTZ,
    PRIMARY KEY(execution_epoch,run_id),
    CHECK (acknowledged_at IS NULL OR completion IS NOT NULL)
);
CREATE INDEX instance_execution_outbox ON instance_execution_reservations(run_id) WHERE completion IS NOT NULL AND acknowledged_at IS NULL;
