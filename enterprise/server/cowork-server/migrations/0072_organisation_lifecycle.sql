-- Archival retains organisation identity, memberships and the audit chain.
ALTER TABLE management_organisations
    ADD COLUMN lifecycle_revision BIGINT NOT NULL DEFAULT 1 CHECK (lifecycle_revision > 0),
    ADD COLUMN archived_at TIMESTAMPTZ,
    ADD COLUMN archived_by UUID REFERENCES users(id),
    ADD CONSTRAINT organisation_archive_actor CHECK ((archived_at IS NULL) = (archived_by IS NULL));

CREATE INDEX management_active_organisations ON management_organisations(name) WHERE archived_at IS NULL;

-- Foreign-key KEY SHARE alone does not conflict with a non-key archive update.
-- These authority-bearing writes take SHARE on the original organisation so
-- archive and a previously admitted writer cannot both commit successfully.
CREATE FUNCTION require_active_management_organisation() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE
    organisation UUID;
BEGIN
    organisation := (to_jsonb(NEW)->>TG_ARGV[0])::UUID;
    PERFORM id FROM management_organisations
        WHERE id=organisation AND archived_at IS NULL FOR SHARE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'organisation is archived or unavailable' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER management_members_active_org BEFORE INSERT OR UPDATE ON management_members
    FOR EACH ROW EXECUTE FUNCTION require_active_management_organisation('organisation_id');
CREATE TRIGGER management_instances_active_org BEFORE INSERT OR UPDATE ON management_instances
    FOR EACH ROW EXECUTE FUNCTION require_active_management_organisation('organisation_id');
CREATE TRIGGER management_instance_members_active_org BEFORE INSERT OR UPDATE ON management_instance_members
    FOR EACH ROW EXECUTE FUNCTION require_active_management_organisation('organisation_id');
CREATE TRIGGER management_invitations_active_org BEFORE INSERT OR UPDATE ON management_invitations
    FOR EACH ROW EXECUTE FUNCTION require_active_management_organisation('organisation_id');
CREATE TRIGGER management_sso_active_org BEFORE INSERT OR UPDATE ON management_sso
    FOR EACH ROW EXECUTE FUNCTION require_active_management_organisation('organisation_id');
CREATE TRIGGER management_templates_active_org BEFORE INSERT OR UPDATE ON management_templates
    FOR EACH ROW EXECUTE FUNCTION require_active_management_organisation('organisation_id');
CREATE TRIGGER management_executor_hosts_active_org BEFORE INSERT OR UPDATE ON management_executor_hosts
    FOR EACH ROW EXECUTE FUNCTION require_active_management_organisation('trust_domain');
CREATE TRIGGER directory_connectors_active_org BEFORE INSERT OR UPDATE ON directory_connectors
    FOR EACH ROW EXECUTE FUNCTION require_active_management_organisation('organisation_id');
CREATE TRIGGER organisation_mfa_policies_active_org BEFORE INSERT OR UPDATE ON organisation_mfa_policies
    FOR EACH ROW EXECUTE FUNCTION require_active_management_organisation('organisation_id');
