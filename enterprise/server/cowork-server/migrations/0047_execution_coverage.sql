-- A persisted activation cannot be undone by starting a replica with a flag off.
CREATE TABLE management_execution_activation (
    singleton BOOLEAN PRIMARY KEY DEFAULT true CHECK (singleton),
    contract_version INTEGER NOT NULL CHECK (contract_version=1),
    activated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
ALTER TABLE management_instances ADD COLUMN execution_required BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE management_instances ADD COLUMN execution_evidence JSONB;
ALTER TABLE management_operations ADD COLUMN execution_runtime_required BOOLEAN NOT NULL DEFAULT false;

CREATE FUNCTION protect_execution_activation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'execution admission activation is irreversible through ordinary configuration';
END $$;
CREATE TRIGGER protect_execution_activation BEFORE UPDATE OR DELETE ON management_execution_activation
    FOR EACH ROW EXECUTE FUNCTION protect_execution_activation();

CREATE FUNCTION require_instance_execution_admission() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='INSERT' THEN
        NEW.execution_required := NEW.execution_required OR EXISTS(SELECT 1 FROM management_execution_activation);
    ELSIF OLD.execution_required AND NOT NEW.execution_required THEN
        RAISE EXCEPTION 'instance execution admission cannot be downgraded';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER require_instance_execution_admission BEFORE INSERT OR UPDATE ON management_instances
    FOR EACH ROW EXECUTE FUNCTION require_instance_execution_admission();

-- Deferred: current management freezes its binding after acquiring the lease in
-- the same transaction. An older replica cannot commit an unbound dispatch.
CREATE FUNCTION verify_execution_dispatch() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE op management_operations; required BOOLEAN;
BEGIN
    SELECT * INTO op FROM management_operations WHERE id=NEW.id;
    SELECT execution_required INTO required FROM management_instances WHERE id=op.instance_id;
    IF required AND op.state IN ('running','reconciling')
        AND (NOT op.execution_runtime_required OR op.execution_binding IS NULL) THEN
        RAISE EXCEPTION 'execution admission contract required for dispatch';
    END IF;
    RETURN NULL;
END $$;
CREATE CONSTRAINT TRIGGER verify_execution_dispatch AFTER INSERT OR UPDATE ON management_operations
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION verify_execution_dispatch();

CREATE FUNCTION verify_execution_readiness() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE instance management_instances;
BEGIN
    SELECT * INTO instance FROM management_instances WHERE id=NEW.id;
    IF instance.execution_required AND instance.state='ready' AND
        (instance.execution_evidence IS NULL
         OR (instance.execution_evidence->>'active_epoch') IS DISTINCT FROM instance.execution_epoch::text
         OR (instance.execution_evidence->'runtime'->>'version') IS DISTINCT FROM '1') THEN
        RAISE EXCEPTION 'verified execution runtime required before readiness';
    END IF;
    RETURN NULL;
END $$;
CREATE CONSTRAINT TRIGGER verify_execution_readiness AFTER INSERT OR UPDATE ON management_instances
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION verify_execution_readiness();
