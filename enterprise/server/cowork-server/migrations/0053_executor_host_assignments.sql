-- Existing workloads remain explicitly unassigned local infrastructure.
CREATE TABLE management_executor_hosts (
    id UUID PRIMARY KEY,
    trust_domain UUID NOT NULL REFERENCES management_organisations(id),
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 120),
    daemon_id TEXT NOT NULL UNIQUE CHECK (daemon_id ~ '^[a-zA-Z0-9:_.-]{12,128}$'),
    operator_state_id UUID NOT NULL UNIQUE,
    capacity JSONB,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_by UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(id, trust_domain)
);
CREATE TABLE management_executor_host_credentials (
    id UUID PRIMARY KEY,
    host_id UUID NOT NULL REFERENCES management_executor_hosts(id),
    token_sha256 BYTEA NOT NULL UNIQUE CHECK (octet_length(token_sha256)=32),
    created_by UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ
);
CREATE FUNCTION preserve_executor_host_credential() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF (NEW.id,NEW.host_id,NEW.token_sha256,NEW.created_by,NEW.created_at)
       IS DISTINCT FROM (OLD.id,OLD.host_id,OLD.token_sha256,OLD.created_by,OLD.created_at)
       OR (OLD.revoked_at IS NOT NULL AND NEW.revoked_at IS DISTINCT FROM OLD.revoked_at) THEN
        RAISE EXCEPTION 'executor host credential identity and revocation are immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER preserve_executor_host_credential BEFORE UPDATE ON management_executor_host_credentials
    FOR EACH ROW EXECUTE FUNCTION preserve_executor_host_credential();
CREATE FUNCTION preserve_executor_host_identity() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF (NEW.id,NEW.trust_domain,NEW.daemon_id,NEW.operator_state_id,NEW.capacity)
       IS DISTINCT FROM (OLD.id,OLD.trust_domain,OLD.daemon_id,OLD.operator_state_id,OLD.capacity) THEN
        RAISE EXCEPTION 'executor host identity is immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER preserve_executor_host_identity BEFORE UPDATE ON management_executor_hosts
    FOR EACH ROW EXECUTE FUNCTION preserve_executor_host_identity();
ALTER TABLE management_instances ADD COLUMN executor_host_id UUID;
ALTER TABLE management_instances ADD CONSTRAINT instance_executor_host_scope
    FOREIGN KEY(executor_host_id,organisation_id) REFERENCES management_executor_hosts(id,trust_domain);
CREATE FUNCTION preserve_instance_executor_host() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.executor_host_id IS DISTINCT FROM OLD.executor_host_id THEN
        RAISE EXCEPTION 'instance executor host assignment is immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER preserve_instance_executor_host BEFORE UPDATE ON management_instances
    FOR EACH ROW EXECUTE FUNCTION preserve_instance_executor_host();
ALTER TABLE management_operations ADD COLUMN executor_host_binding JSONB;
CREATE FUNCTION freeze_operation_executor_host() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE expected JSONB;
BEGIN
    SELECT jsonb_build_object('version',1,'host_id',h.id,'trust_domain',h.trust_domain,
           'daemon_id',h.daemon_id,'operator_state_id',h.operator_state_id)
      INTO expected FROM management_instances i JOIN management_executor_hosts h ON h.id=i.executor_host_id
      WHERE i.id=NEW.instance_id AND i.organisation_id=NEW.organisation_id;
    IF TG_OP='INSERT' THEN
        IF NEW.executor_host_binding IS NOT NULL AND NEW.executor_host_binding IS DISTINCT FROM expected THEN
            RAISE EXCEPTION 'operation executor host mismatch';
        END IF;
        NEW.executor_host_binding := expected;
    ELSIF NEW.executor_host_binding IS DISTINCT FROM OLD.executor_host_binding
       OR NEW.executor_host_binding IS DISTINCT FROM expected THEN
        RAISE EXCEPTION 'operation executor host binding is immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER freeze_operation_executor_host BEFORE INSERT OR UPDATE ON management_operations
    FOR EACH ROW EXECUTE FUNCTION freeze_operation_executor_host();
CREATE INDEX management_instances_executor_host ON management_instances(executor_host_id);
CREATE TABLE management_executor_host_releases (
    host_id UUID NOT NULL REFERENCES management_executor_hosts(id),
    release_id TEXT NOT NULL REFERENCES management_releases(id),
    active BOOLEAN NOT NULL DEFAULT TRUE,
    refreshed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY(host_id,release_id)
);
ALTER TABLE management_release_qualifications ADD COLUMN executor_host_id UUID REFERENCES management_executor_hosts(id);
ALTER TABLE management_release_qualifications DROP CONSTRAINT management_release_qualifications_pkey;
CREATE UNIQUE INDEX management_release_qualifications_scope ON management_release_qualifications
    (release_id,source_release,executor_host_id) NULLS NOT DISTINCT;
CREATE FUNCTION validate_executor_host_qualification() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS(SELECT 1 FROM management_operations o JOIN management_instances i ON i.id=o.instance_id
       WHERE o.id=NEW.operation_id AND i.executor_host_id IS NOT DISTINCT FROM NEW.executor_host_id) THEN
        RAISE EXCEPTION 'canary qualification executor host mismatch';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER validate_executor_host_qualification BEFORE INSERT OR UPDATE ON management_release_qualifications
    FOR EACH ROW EXECUTE FUNCTION validate_executor_host_qualification();
