-- Environment execution rights refine existing instance rights. Existing and
-- newly created environments inherit until an administrator explicitly narrows.
CREATE TABLE management_environment_access_policies (
    environment_id UUID PRIMARY KEY,
    organisation_id UUID NOT NULL,
    instance_id UUID NOT NULL,
    revision BIGINT NOT NULL DEFAULT 1 CHECK(revision>0),
    mode TEXT NOT NULL DEFAULT 'inherit_instance' CHECK(mode IN ('inherit_instance','restricted')),
    updated_by UUID REFERENCES users(id),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE(organisation_id,instance_id,environment_id),
    FOREIGN KEY(organisation_id,instance_id,environment_id) REFERENCES management_environments(organisation_id,instance_id,id)
);
INSERT INTO management_environment_access_policies(environment_id,organisation_id,instance_id)
    SELECT id,organisation_id,instance_id FROM management_environments;
CREATE TABLE management_environment_access_members (
    organisation_id UUID NOT NULL,
    instance_id UUID NOT NULL,
    environment_id UUID NOT NULL,
    user_id UUID NOT NULL,
    role TEXT NOT NULL CHECK(role IN ('viewer','runner')),
    PRIMARY KEY(environment_id,user_id),
    FOREIGN KEY(organisation_id,instance_id,environment_id) REFERENCES management_environment_access_policies(organisation_id,instance_id,environment_id),
    FOREIGN KEY(organisation_id,user_id) REFERENCES management_members(organisation_id,user_id)
);
CREATE INDEX management_environment_access_user ON management_environment_access_members(organisation_id,instance_id,user_id);
CREATE TRIGGER management_environment_policy_active_org BEFORE INSERT OR UPDATE ON management_environment_access_policies
    FOR EACH ROW EXECUTE FUNCTION require_active_management_organisation('organisation_id');
CREATE TRIGGER management_environment_members_active_org BEFORE INSERT OR UPDATE ON management_environment_access_members
    FOR EACH ROW EXECUTE FUNCTION require_active_management_organisation('organisation_id');
CREATE FUNCTION create_management_environment_access_policy() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO management_environment_access_policies(environment_id,organisation_id,instance_id)
        VALUES(NEW.id,NEW.organisation_id,NEW.instance_id);
    RETURN NEW;
END;
$$;
CREATE TRIGGER management_environment_default_access AFTER INSERT ON management_environments
    FOR EACH ROW EXECUTE FUNCTION create_management_environment_access_policy();

CREATE FUNCTION management_environment_access_allowed(org UUID,instance UUID,environment UUID,member_user UUID,minimum_role TEXT)
RETURNS BOOLEAN LANGUAGE sql STABLE AS $$
    SELECT minimum_role IN ('viewer','runner') AND EXISTS(
        SELECT 1 FROM management_environment_access_policies p
        JOIN management_environments e ON e.id=p.environment_id AND e.organisation_id=p.organisation_id AND e.instance_id=p.instance_id
        JOIN management_instances i ON i.id=p.instance_id AND i.organisation_id=p.organisation_id
        JOIN management_organisations o ON o.id=p.organisation_id
        JOIN management_members m ON m.organisation_id=p.organisation_id AND m.user_id=member_user
        JOIN users u ON u.id=m.user_id
        WHERE p.organisation_id=org AND p.instance_id=instance AND p.environment_id=environment
        AND o.archived_at IS NULL AND u.deleted_at IS NULL AND m.role IN ('admin','member','auditor')
        AND i.state NOT IN ('deleted','deleting') AND e.state NOT IN ('deleted','deleting')
        AND (m.role='admin' OR i.owner_user_id=member_user OR EXISTS(
            SELECT 1 FROM management_instance_members im WHERE im.organisation_id=org AND im.instance_id=instance AND im.user_id=member_user
            AND (minimum_role='viewer' OR im.role IN ('manager','member'))))
        AND (minimum_role='viewer' OR m.role IN ('admin','member'))
        AND (p.mode='inherit_instance' OR EXISTS(
            SELECT 1 FROM management_environment_access_members a WHERE a.environment_id=environment AND a.user_id=member_user
            AND a.organisation_id=org AND a.instance_id=instance AND (minimum_role='viewer' OR a.role='runner')))
    )
$$;

CREATE OR REPLACE FUNCTION management_resource_scope_available(org UUID,member_user UUID,instance UUID,environment UUID,resource UUID)
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
          AND (environment IS NULL OR management_environment_access_allowed(org,instance,environment,member_user,'viewer'))))
$$;
