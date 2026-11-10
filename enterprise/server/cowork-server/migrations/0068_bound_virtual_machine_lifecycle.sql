-- Original manager plans, independent VM controllers and disk snapshot custody.
ALTER TABLE management_environment_jobs ADD COLUMN recovery_authority JSONB;
ALTER TABLE management_environment_jobs DROP CONSTRAINT management_environment_jobs_action_check;
ALTER TABLE management_environment_jobs ADD CONSTRAINT management_environment_jobs_action_check CHECK(action IN ('qualify','start','stop','delete','inspect','power_start','power_stop'));
ALTER TABLE management_environment_jobs ALTER COLUMN template_id DROP NOT NULL;
ALTER TABLE management_environment_jobs ALTER COLUMN template_revision DROP NOT NULL;
CREATE TABLE management_environment_executions (
    id UUID PRIMARY KEY,
    organisation_id UUID NOT NULL,
    instance_id UUID NOT NULL,
    environment_id UUID NOT NULL REFERENCES management_environments(id),
    user_id UUID NOT NULL REFERENCES users(id),
    request_id UUID NOT NULL,
    workflow_id UUID NOT NULL,
    step_id UUID NOT NULL,
    request_sha256 TEXT NOT NULL CHECK(length(request_sha256)=64),
    plan JSONB NOT NULL,
    plan_sha256 TEXT NOT NULL CHECK(length(plan_sha256)=64),
    state TEXT NOT NULL CHECK(state IN ('queued','running','held','completed','failed','halted')),
    lease_token UUID,
    result JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ,
    stop_requested_at TIMESTAMPTZ,
    stop_reader_authority JSONB,
    UNIQUE(instance_id,request_id),
    FOREIGN KEY(organisation_id,instance_id) REFERENCES management_instances(organisation_id,id)
);
CREATE UNIQUE INDEX management_one_environment_execution ON management_environment_executions(environment_id) WHERE state IN ('queued','running','held');
CREATE TABLE management_vm_controller_plans (
    id UUID PRIMARY KEY,
    organisation_id UUID NOT NULL REFERENCES management_organisations(id),
    host_id UUID NOT NULL,
    actor_user_id UUID NOT NULL REFERENCES users(id),
    request_id UUID NOT NULL,
    request_sha256 TEXT NOT NULL CHECK(length(request_sha256)=64),
    actor_authority JSONB NOT NULL,
    expected_revision BIGINT NOT NULL CHECK(expected_revision>=0),
    specification JSONB NOT NULL,
    specification_sha256 TEXT NOT NULL CHECK(length(specification_sha256)=64),
    executor_host_binding JSONB NOT NULL,
    challenge UUID NOT NULL,
    receipt JSONB,
    expires_at TIMESTAMPTZ NOT NULL DEFAULT now()+interval '10 minutes',
    UNIQUE(organisation_id,actor_user_id,request_id)
);
CREATE TABLE management_vm_controllers (
    id UUID PRIMARY KEY,
    organisation_id UUID NOT NULL REFERENCES management_organisations(id),
    host_id UUID NOT NULL,
    backend TEXT NOT NULL CHECK(backend IN ('qemu','libvirt','kubevirt','hyper_v','attached','open_shell')),
    revision BIGINT NOT NULL CHECK(revision>0),
    plan_id UUID NOT NULL REFERENCES management_vm_controller_plans(id),
    specification JSONB NOT NULL,
    specification_sha256 TEXT NOT NULL CHECK(length(specification_sha256)=64),
    executor_host_binding JSONB NOT NULL,
    receipt JSONB NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    UNIQUE(host_id,backend)
);
CREATE TABLE management_vm_jobs (
    id UUID PRIMARY KEY,
    organisation_id UUID NOT NULL,
    instance_id UUID NOT NULL,
    environment_id UUID REFERENCES management_environments(id),
    actor_user_id UUID NOT NULL REFERENCES users(id),
    request_id UUID NOT NULL,
    request_sha256 TEXT NOT NULL CHECK(length(request_sha256)=64),
    action TEXT NOT NULL CHECK(action IN ('snapshot','restore_snapshot','delete_snapshot')),
    plan JSONB NOT NULL,
    plan_sha256 TEXT NOT NULL CHECK(length(plan_sha256)=64),
    state TEXT NOT NULL DEFAULT 'queued' CHECK(state IN ('queued','running','held','completed')),
    lease_token UUID,
    receipt JSONB,
    recovery_actor_user_id UUID REFERENCES users(id),
    recovery_authorized BOOLEAN NOT NULL DEFAULT FALSE,
    recovery_authority JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ,
    UNIQUE(organisation_id,actor_user_id,request_id),
    FOREIGN KEY(organisation_id,instance_id) REFERENCES management_instances(organisation_id,id)
);
CREATE UNIQUE INDEX management_one_vm_job ON management_vm_jobs(instance_id) WHERE state IN ('queued','running','held');
CREATE TABLE management_vm_snapshots (
    id UUID PRIMARY KEY,
    organisation_id UUID NOT NULL,
    instance_id UUID NOT NULL,
    environment_id UUID REFERENCES management_environments(id),
    controller_id UUID REFERENCES management_vm_controllers(id),
    source_job_id UUID NOT NULL REFERENCES management_vm_jobs(id),
    receipt JSONB NOT NULL,
    state TEXT NOT NULL DEFAULT 'retained' CHECK(state IN ('retained','deleted')),
    revision BIGINT NOT NULL DEFAULT 1 CHECK(revision>0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY(organisation_id,instance_id) REFERENCES management_instances(organisation_id,id)
);
