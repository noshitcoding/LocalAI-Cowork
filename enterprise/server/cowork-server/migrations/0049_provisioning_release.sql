-- The initial image selection survives queued work and changes to operator defaults.
ALTER TABLE management_instances ADD COLUMN provisioning_release TEXT
    REFERENCES management_releases(id);

CREATE FUNCTION preserve_provisioning_release() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.provisioning_release IS DISTINCT FROM OLD.provisioning_release THEN
        RAISE EXCEPTION 'initial provisioning release is immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER preserve_provisioning_release BEFORE UPDATE ON management_instances
    FOR EACH ROW EXECUTE FUNCTION preserve_provisioning_release();

ALTER TABLE management_operations ADD COLUMN provisioning_release_version INTEGER;

CREATE FUNCTION verify_provisioning_release() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE selected TEXT;
BEGIN
    IF NEW.action='provision' THEN
        SELECT provisioning_release INTO selected FROM management_instances WHERE id=NEW.instance_id;
        IF NEW.arguments->>'release' IS DISTINCT FROM selected
            OR (selected IS NOT NULL AND NEW.arguments->'provisioning_release_version' IS DISTINCT FROM '1'::jsonb)
            OR (selected IS NULL AND NEW.arguments ? 'provisioning_release_version')
            OR (selected IS NOT NULL AND NEW.state IN ('running','reconciling','succeeded')
                AND NEW.provisioning_release_version IS DISTINCT FROM 1) THEN
            RAISE EXCEPTION 'provisioning operation must preserve the initial release';
        END IF;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER verify_provisioning_release BEFORE INSERT OR UPDATE ON management_operations
    FOR EACH ROW EXECUTE FUNCTION verify_provisioning_release();
