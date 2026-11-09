ALTER TABLE management_instances ADD COLUMN execution_enrollment JSONB;

CREATE FUNCTION verify_execution_enrollment() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE instance management_instances; op management_operations;
BEGIN
    SELECT * INTO instance FROM management_instances WHERE id=NEW.id;
    IF OLD.execution_enrollment IS NOT NULL AND instance.execution_enrollment IS DISTINCT FROM OLD.execution_enrollment THEN
        RAISE EXCEPTION 'execution enrollment evidence is immutable';
    END IF;
    IF NOT OLD.execution_required AND NEW.execution_required THEN
        SELECT * INTO op FROM management_operations
            WHERE instance_id=instance.id AND action='enroll_execution' AND state='succeeded'
              AND result->'execution_enrollment'=instance.execution_enrollment;
        IF op.id IS NULL OR NOT op.execution_runtime_required OR op.execution_binding IS NULL
            OR instance.execution_enrollment->>'outcome' IS DISTINCT FROM 'enrolled'
            OR instance.execution_evidence->>'active_epoch' IS DISTINCT FROM instance.execution_epoch::text
            OR instance.execution_evidence->'runtime'->>'version' IS DISTINCT FROM '1' THEN
            RAISE EXCEPTION 'verified enrollment operation required for legacy activation';
        END IF;
    END IF;
    RETURN NULL;
END $$;
CREATE CONSTRAINT TRIGGER verify_execution_enrollment AFTER UPDATE ON management_instances
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION verify_execution_enrollment();

CREATE OR REPLACE FUNCTION verify_execution_dispatch() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE op management_operations; required BOOLEAN;
BEGIN
    SELECT * INTO op FROM management_operations WHERE id=NEW.id;
    SELECT execution_required INTO required FROM management_instances WHERE id=op.instance_id;
    IF (required OR op.action='enroll_execution') AND op.state IN ('running','reconciling')
        AND (NOT op.execution_runtime_required OR op.execution_binding IS NULL) THEN
        RAISE EXCEPTION 'execution admission contract required for dispatch';
    END IF;
    RETURN NULL;
END $$;
