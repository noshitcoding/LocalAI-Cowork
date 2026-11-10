-- Management metadata is hosted in the management service's own database.
CREATE TABLE management_organisations (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 120),
    policy JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE management_members (
    organisation_id UUID NOT NULL REFERENCES management_organisations(id),
    user_id UUID NOT NULL REFERENCES users(id),
    role TEXT NOT NULL CHECK (role IN ('admin','member','auditor')),
    PRIMARY KEY (organisation_id,user_id)
);
CREATE TABLE management_instances (
    id UUID PRIMARY KEY,
    organisation_id UUID NOT NULL REFERENCES management_organisations(id),
    owner_user_id UUID NOT NULL REFERENCES users(id),
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 120),
    backend TEXT NOT NULL CHECK (backend IN ('compose','kubernetes')),
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending','provisioning','ready','stopped','suspended','deleting','deleted','failed')),
    policy JSONB NOT NULL,
    revision BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (organisation_id,id),
    FOREIGN KEY (organisation_id,owner_user_id) REFERENCES management_members(organisation_id,user_id)
);
CREATE INDEX management_instances_org ON management_instances(organisation_id,owner_user_id);
CREATE TABLE management_instance_members (
    organisation_id UUID NOT NULL,
    instance_id UUID NOT NULL,
    user_id UUID NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('manager','member','viewer')),
    PRIMARY KEY(instance_id,user_id),
    FOREIGN KEY(organisation_id,instance_id) REFERENCES management_instances(organisation_id,id),
    FOREIGN KEY(organisation_id,user_id) REFERENCES management_members(organisation_id,user_id)
);
CREATE TABLE management_operations (
    id UUID PRIMARY KEY,
    organisation_id UUID NOT NULL,
    instance_id UUID NOT NULL,
    actor_user_id UUID NOT NULL REFERENCES users(id),
    action TEXT NOT NULL,
    arguments JSONB NOT NULL DEFAULT '{}',
    state TEXT NOT NULL DEFAULT 'queued' CHECK(state IN ('queued','running','succeeded','failed','reconciling')),
    idempotency_key TEXT NOT NULL,
    request_digest TEXT NOT NULL,
    lease_token UUID,
    lease_expires_at TIMESTAMPTZ,
    result JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(actor_user_id,idempotency_key),
    FOREIGN KEY(organisation_id,instance_id) REFERENCES management_instances(organisation_id,id)
);
CREATE UNIQUE INDEX management_one_active_operation ON management_operations(instance_id)
    WHERE state IN ('queued','running','reconciling');
CREATE TABLE management_templates (
    id UUID NOT NULL,
    organisation_id UUID NOT NULL REFERENCES management_organisations(id),
    revision BIGINT NOT NULL CHECK(revision > 0),
    name TEXT NOT NULL,
    backend TEXT NOT NULL,
    isolation TEXT NOT NULL CHECK(isolation IN ('container','virtual_machine')),
    image_reference TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY(id,revision),
    UNIQUE(organisation_id,id,revision)
);
CREATE TABLE management_environments (
    id UUID PRIMARY KEY,
    organisation_id UUID NOT NULL,
    instance_id UUID NOT NULL,
    template_id UUID NOT NULL,
    template_revision BIGINT NOT NULL,
    spec JSONB NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY(organisation_id,instance_id) REFERENCES management_instances(organisation_id,id),
    FOREIGN KEY(organisation_id,template_id,template_revision) REFERENCES management_templates(organisation_id,id,revision)
);
CREATE TABLE management_audit (
    sequence BIGSERIAL PRIMARY KEY,
    organisation_id UUID NOT NULL REFERENCES management_organisations(id),
    actor_user_id UUID NOT NULL,
    action TEXT NOT NULL,
    target_id UUID NOT NULL,
    payload JSONB NOT NULL,
    previous_digest TEXT NOT NULL,
    digest TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX management_audit_org ON management_audit(organisation_id,sequence);
