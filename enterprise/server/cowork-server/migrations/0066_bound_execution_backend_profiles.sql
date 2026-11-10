-- Backend activation is an explicit original plan and a scoped operator receipt.
CREATE TABLE management_backend_profile_plans (
    id UUID PRIMARY KEY,
    organisation_id UUID NOT NULL,
    instance_id UUID NOT NULL,
    host_id UUID NOT NULL REFERENCES management_executor_hosts(id),
    actor_user_id UUID NOT NULL REFERENCES users(id),
    request_id UUID NOT NULL,
    request_sha256 TEXT NOT NULL,
    expected_instance_revision BIGINT NOT NULL,
    expected_profile_revision BIGINT NOT NULL CHECK(expected_profile_revision>=0),
    specification JSONB NOT NULL,
    specification_sha256 TEXT NOT NULL,
    executor_host_binding JSONB NOT NULL,
    challenge UUID NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()+interval '10 minutes',
    receipt JSONB,
    UNIQUE(organisation_id,actor_user_id,request_id),
    FOREIGN KEY(organisation_id,instance_id) REFERENCES management_instances(organisation_id,id)
);
CREATE TABLE management_backend_profiles (
    instance_id UUID PRIMARY KEY,
    organisation_id UUID NOT NULL,
    host_id UUID NOT NULL REFERENCES management_executor_hosts(id),
    revision BIGINT NOT NULL CHECK(revision>0),
    plan_id UUID NOT NULL UNIQUE REFERENCES management_backend_profile_plans(id),
    specification JSONB NOT NULL,
    specification_sha256 TEXT NOT NULL,
    executor_host_binding JSONB NOT NULL,
    receipt JSONB NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    FOREIGN KEY(organisation_id,instance_id) REFERENCES management_instances(organisation_id,id)
);
ALTER TABLE management_operations ADD COLUMN backend_profile JSONB;
