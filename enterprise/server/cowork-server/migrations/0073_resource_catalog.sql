-- Catalog records are configuration metadata. They contain secret references,
-- never provider credentials, and confer no platform/organisation role.
CREATE TABLE management_resource_catalog (
    id UUID PRIMARY KEY,
    kind TEXT NOT NULL CHECK(kind IN ('model','endpoint','mcp','skill','agent_role')),
    name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 120),
    description TEXT NOT NULL DEFAULT '' CHECK(octet_length(description)<=4000),
    configuration JSONB NOT NULL CHECK(jsonb_typeof(configuration)='object'),
    secret_reference TEXT CHECK(secret_reference ~ '^(secret|vault|env):[A-Za-z0-9_./-]{1,180}$'),
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    revision BIGINT NOT NULL DEFAULT 1 CHECK(revision>0),
    created_by UUID NOT NULL REFERENCES users(id),
    updated_by UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);
CREATE TABLE management_resource_dependencies (
    resource_id UUID NOT NULL REFERENCES management_resource_catalog(id),
    depends_on UUID NOT NULL REFERENCES management_resource_catalog(id),
    PRIMARY KEY(resource_id,depends_on),
    CHECK(resource_id<>depends_on)
);
CREATE INDEX management_resource_dependency_target ON management_resource_dependencies(depends_on);

CREATE FUNCTION management_resource_available(resource UUID) RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    WITH RECURSIVE dependency_tree(id) AS (
        SELECT resource UNION
        SELECT d.depends_on FROM management_resource_dependencies d
        JOIN dependency_tree t ON t.id=d.resource_id
    )
    SELECT EXISTS(SELECT 1 FROM management_resource_catalog WHERE id=resource)
       AND NOT EXISTS(SELECT 1 FROM dependency_tree t
           LEFT JOIN management_resource_catalog r ON r.id=t.id
           WHERE r.id IS NULL OR NOT r.enabled OR r.deleted_at IS NOT NULL)
$$;

CREATE TABLE management_resource_assignment_sets (
    organisation_id UUID NOT NULL,
    user_id UUID NOT NULL,
    revision BIGINT NOT NULL DEFAULT 0 CHECK(revision>=0),
    updated_by UUID NOT NULL REFERENCES users(id),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY(organisation_id,user_id),
    FOREIGN KEY(organisation_id,user_id) REFERENCES management_members(organisation_id,user_id) ON DELETE CASCADE
);
ALTER TABLE management_environments ADD CONSTRAINT management_environment_resource_scope UNIQUE(organisation_id,instance_id,id);
CREATE TABLE management_resource_assignments (
    id UUID PRIMARY KEY,
    organisation_id UUID NOT NULL,
    user_id UUID NOT NULL,
    resource_id UUID NOT NULL REFERENCES management_resource_catalog(id),
    instance_id UUID,
    environment_id UUID,
    assigned_by UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK(environment_id IS NULL OR instance_id IS NOT NULL),
    FOREIGN KEY(organisation_id,user_id) REFERENCES management_resource_assignment_sets(organisation_id,user_id) ON DELETE CASCADE,
    FOREIGN KEY(organisation_id,instance_id) REFERENCES management_instances(organisation_id,id),
    FOREIGN KEY(organisation_id,instance_id,environment_id) REFERENCES management_environments(organisation_id,instance_id,id),
    UNIQUE NULLS NOT DISTINCT(organisation_id,user_id,resource_id,instance_id,environment_id)
);
CREATE INDEX management_resource_assignment_resource ON management_resource_assignments(resource_id);
CREATE INDEX management_resource_assignment_instance ON management_resource_assignments(organisation_id,instance_id,user_id);
CREATE TRIGGER management_resource_sets_active_org BEFORE INSERT OR UPDATE ON management_resource_assignment_sets
    FOR EACH ROW EXECUTE FUNCTION require_active_management_organisation('organisation_id');
CREATE TRIGGER management_resource_assignments_active_org BEFORE INSERT OR UPDATE ON management_resource_assignments
    FOR EACH ROW EXECUTE FUNCTION require_active_management_organisation('organisation_id');

-- Grants remain metadata after suspension/deletion, but are unavailable as soon
-- as any current membership, catalog dependency or execution scope disappears.
CREATE FUNCTION management_resource_scope_available(org UUID,member_user UUID,instance UUID,environment UUID,resource UUID)
RETURNS BOOLEAN LANGUAGE sql STABLE AS $$
    SELECT management_resource_available(resource)
      AND EXISTS(SELECT 1 FROM management_organisations o JOIN management_members m ON m.organisation_id=o.id
          JOIN users u ON u.id=m.user_id WHERE o.id=org AND o.archived_at IS NULL AND u.id=member_user
          AND u.deleted_at IS NULL AND m.role IN ('admin','member','auditor'))
      AND ((instance IS NULL AND environment IS NULL) OR EXISTS(
          SELECT 1 FROM management_instances i JOIN management_members m ON m.organisation_id=i.organisation_id
          WHERE i.organisation_id=org AND i.id=instance AND m.user_id=member_user
          AND i.state NOT IN ('deleted','deleting') AND (m.role='admin' OR i.owner_user_id=member_user
              OR EXISTS(SELECT 1 FROM management_instance_members im WHERE im.organisation_id=org AND im.instance_id=i.id AND im.user_id=member_user))
          AND (environment IS NULL OR EXISTS(SELECT 1 FROM management_environments e WHERE e.id=environment
              AND e.organisation_id=org AND e.instance_id=instance AND e.state NOT IN ('deleted','deleting')))))
$$;
