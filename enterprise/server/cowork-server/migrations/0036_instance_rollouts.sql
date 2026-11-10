CREATE TABLE management_rollouts (
    id UUID PRIMARY KEY,
    organisation_id UUID NOT NULL REFERENCES management_organisations(id),
    actor_user_id UUID NOT NULL REFERENCES users(id),
    name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 120),
    release TEXT NOT NULL REFERENCES management_releases(id),
    batch_size INTEGER NOT NULL CHECK(batch_size BETWEEN 1 AND 10),
    state TEXT NOT NULL DEFAULT 'running' CHECK(state IN ('running','paused','canceling','canceled','succeeded')),
    error_code TEXT,
    revision BIGINT NOT NULL DEFAULT 1,
    idempotency_key UUID NOT NULL,
    request_digest TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(organisation_id,id),
    UNIQUE(actor_user_id,idempotency_key)
);
ALTER TABLE management_operations ADD COLUMN rollout_id UUID;
ALTER TABLE management_operations ADD CONSTRAINT management_operation_rollout_scope
    FOREIGN KEY(organisation_id,rollout_id) REFERENCES management_rollouts(organisation_id,id);
CREATE TABLE management_rollout_targets (
    organisation_id UUID NOT NULL,
    rollout_id UUID NOT NULL,
    instance_id UUID NOT NULL,
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    source_release TEXT NOT NULL CHECK(source_release ~ '^[a-f0-9]{64}$'),
    initial_state TEXT NOT NULL CHECK(initial_state IN ('ready','stopped')),
    expected_revision BIGINT NOT NULL,
    state TEXT NOT NULL DEFAULT 'waiting' CHECK(state IN ('waiting','active','succeeded','failed','canceled')),
    next_action TEXT CHECK(next_action IN ('stop','upgrade','start')),
    operation_id UUID REFERENCES management_operations(id),
    error_code TEXT,
    reserved BOOLEAN NOT NULL DEFAULT TRUE,
    PRIMARY KEY(rollout_id,instance_id),
    UNIQUE(rollout_id,ordinal),
    FOREIGN KEY(organisation_id,rollout_id) REFERENCES management_rollouts(organisation_id,id),
    FOREIGN KEY(organisation_id,instance_id) REFERENCES management_instances(organisation_id,id)
);
CREATE UNIQUE INDEX management_one_rollout_reservation ON management_rollout_targets(instance_id) WHERE reserved;
CREATE INDEX management_rollout_org ON management_rollouts(organisation_id,created_at);
