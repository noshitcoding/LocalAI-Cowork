-- Environment reservations survive uncertain outcomes and stopped storage.
ALTER TABLE management_environments ADD COLUMN revision BIGINT NOT NULL DEFAULT 1 CHECK(revision>0);
ALTER TABLE management_environments ADD COLUMN runtime JSONB;
ALTER TABLE management_environments ADD COLUMN registered_by UUID REFERENCES users(id);
ALTER TABLE management_environments ADD COLUMN request_id UUID;
ALTER TABLE management_environments ADD COLUMN request_sha256 TEXT;
CREATE UNIQUE INDEX management_environment_original_create ON management_environments(organisation_id,registered_by,request_id);
ALTER TABLE management_templates ADD COLUMN registered_by UUID REFERENCES users(id);
ALTER TABLE management_templates ADD COLUMN request_id UUID;
ALTER TABLE management_templates ADD COLUMN request_sha256 TEXT;
CREATE UNIQUE INDEX management_template_original_create ON management_templates(organisation_id,registered_by,request_id);
CREATE TABLE management_environment_jobs (
    id UUID PRIMARY KEY,
    organisation_id UUID NOT NULL,
    instance_id UUID NOT NULL,
    environment_id UUID REFERENCES management_environments(id),
    template_id UUID NOT NULL,
    template_revision BIGINT NOT NULL,
    actor_user_id UUID NOT NULL REFERENCES users(id),
    request_id UUID NOT NULL,
    request_sha256 TEXT NOT NULL CHECK(length(request_sha256)=64),
    action TEXT NOT NULL CHECK(action IN ('qualify','start','stop','delete','inspect')),
    plan JSONB NOT NULL,
    plan_sha256 TEXT NOT NULL CHECK(length(plan_sha256)=64),
    state TEXT NOT NULL DEFAULT 'queued' CHECK(state IN ('queued','running','held','completed')),
    lease_token UUID,
    receipt JSONB,
    recovery_actor_user_id UUID REFERENCES users(id),
    recovery_authorized BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ,
    UNIQUE(organisation_id,actor_user_id,request_id),
    FOREIGN KEY(organisation_id,instance_id) REFERENCES management_instances(organisation_id,id),
    FOREIGN KEY(organisation_id,template_id,template_revision) REFERENCES management_templates(organisation_id,id,revision)
);
CREATE UNIQUE INDEX management_one_environment_job ON management_environment_jobs(instance_id)
    WHERE state IN ('queued','running','held');
CREATE TABLE management_template_qualifications (
    organisation_id UUID NOT NULL,
    template_id UUID NOT NULL,
    template_revision BIGINT NOT NULL,
    instance_id UUID NOT NULL,
    profile_sha256 TEXT NOT NULL CHECK(length(profile_sha256)=64),
    job_id UUID NOT NULL REFERENCES management_environment_jobs(id),
    receipt JSONB NOT NULL,
    PRIMARY KEY(template_id,template_revision,instance_id,profile_sha256),
    FOREIGN KEY(organisation_id,template_id,template_revision) REFERENCES management_templates(organisation_id,id,revision)
);
