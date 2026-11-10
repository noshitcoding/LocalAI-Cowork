-- Assigned hosts remain unqualified until a scoped operator reports the exact
-- inventory. No guessed service defaults or allocations are adopted for old hosts.
ALTER TABLE management_executor_hosts ADD COLUMN service_plan JSONB;
ALTER TABLE management_executor_hosts ADD COLUMN allocation_revision BIGINT NOT NULL DEFAULT 0 CHECK(allocation_revision>=0);
ALTER TABLE management_operations ADD COLUMN host_allocation_binding JSONB;

CREATE FUNCTION preserve_host_service_plan() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.service_plan IS DISTINCT FROM OLD.service_plan THEN
        RAISE EXCEPTION 'executor host service plan is immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER preserve_host_service_plan BEFORE UPDATE ON management_executor_hosts
    FOR EACH ROW EXECUTE FUNCTION preserve_host_service_plan();

CREATE TABLE management_host_allocations (
    instance_id UUID PRIMARY KEY REFERENCES management_instances(id),
    host_id UUID NOT NULL,
    organisation_id UUID NOT NULL,
    plan JSONB NOT NULL,
    plan_sha256 TEXT NOT NULL CHECK(plan_sha256 ~ '^[0-9a-f]{64}$'),
    cpu_millis BIGINT NOT NULL CHECK(cpu_millis>0),
    memory_mib BIGINT NOT NULL CHECK(memory_mib>0),
    revision BIGINT NOT NULL DEFAULT 1 CHECK(revision>0),
    state TEXT NOT NULL DEFAULT 'reserved' CHECK(state IN ('reserved','verified','unknown','released')),
    verified_epoch UUID,
    verified_manifest_sha256 TEXT CHECK(verified_manifest_sha256 ~ '^[0-9a-f]{64}$'),
    verified_revision BIGINT,
    verified_budget JSONB,
    release_proof JSONB,
    released_at TIMESTAMPTZ,
    FOREIGN KEY(host_id,organisation_id) REFERENCES management_executor_hosts(id,trust_domain),
    CHECK((state='released')=(release_proof IS NOT NULL AND released_at IS NOT NULL))
);
CREATE INDEX management_host_allocations_host ON management_host_allocations(host_id);
CREATE FUNCTION preserve_host_allocation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        RAISE EXCEPTION 'host allocations are retained until verified permanent deletion';
    END IF;
    IF NOT EXISTS(SELECT 1 FROM management_instances i WHERE i.id=NEW.instance_id
          AND i.organisation_id=NEW.organisation_id AND i.executor_host_id=NEW.host_id) THEN
        RAISE EXCEPTION 'host allocation assignment mismatch';
    END IF;
    IF TG_OP='UPDATE' AND ((NEW.instance_id,NEW.host_id,NEW.organisation_id,NEW.plan,NEW.plan_sha256,NEW.cpu_millis,NEW.memory_mib)
          IS DISTINCT FROM (OLD.instance_id,OLD.host_id,OLD.organisation_id,OLD.plan,OLD.plan_sha256,OLD.cpu_millis,OLD.memory_mib)
          OR (OLD.state='released' AND NEW IS DISTINCT FROM OLD)
          OR NEW.revision<OLD.revision) THEN
        RAISE EXCEPTION 'host allocation identity and maximum charge are immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER preserve_host_allocation BEFORE INSERT OR UPDATE OR DELETE ON management_host_allocations
    FOR EACH ROW EXECUTE FUNCTION preserve_host_allocation();
CREATE FUNCTION advance_host_allocation_revision() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='INSERT' OR NEW.revision IS DISTINCT FROM OLD.revision
       OR (NEW.state='released' AND OLD.state<>'released') THEN
        UPDATE management_executor_hosts SET allocation_revision=allocation_revision+1 WHERE id=NEW.host_id;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER advance_host_allocation_revision AFTER INSERT OR UPDATE ON management_host_allocations
    FOR EACH ROW EXECUTE FUNCTION advance_host_allocation_revision();

CREATE TABLE management_host_capacity_proofs (
    host_id UUID PRIMARY KEY REFERENCES management_executor_hosts(id),
    host_revision BIGINT NOT NULL,
    allocation_revision BIGINT NOT NULL,
    qualified BOOLEAN NOT NULL,
    reason TEXT,
    observed_capacity JSONB NOT NULL,
    inventory_sha256 TEXT NOT NULL CHECK(inventory_sha256 ~ '^[0-9a-f]{64}$'),
    checked_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    expires_at TIMESTAMPTZ NOT NULL,
    CHECK(qualified=(reason IS NULL))
);
CREATE TABLE management_host_capacity_challenges (
    id UUID PRIMARY KEY,
    host_id UUID NOT NULL REFERENCES management_executor_hosts(id),
    host_revision BIGINT NOT NULL,
    allocation_revision BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()+interval '30 seconds'
);
CREATE INDEX management_host_capacity_challenges_expiry ON management_host_capacity_challenges(expires_at);

CREATE FUNCTION freeze_host_allocation_binding() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.host_allocation_binding IS NOT NULL AND NEW.host_allocation_binding IS DISTINCT FROM OLD.host_allocation_binding THEN
        RAISE EXCEPTION 'operation host allocation binding is immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER freeze_host_allocation_binding BEFORE UPDATE ON management_operations
    FOR EACH ROW EXECUTE FUNCTION freeze_host_allocation_binding();
