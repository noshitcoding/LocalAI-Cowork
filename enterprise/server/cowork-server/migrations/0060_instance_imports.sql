-- A source-to-new-target import never changes an instance's immutable host assignment.
ALTER TABLE management_backups ADD CONSTRAINT management_backup_import_scope UNIQUE(organisation_id,instance_id,id);
CREATE TABLE management_instance_imports (
    id UUID PRIMARY KEY,
    organisation_id UUID NOT NULL REFERENCES management_organisations(id),
    source_instance_id UUID NOT NULL,
    target_instance_id UUID NOT NULL,
    actor_user_id UUID NOT NULL REFERENCES users(id),
    backup_id UUID NOT NULL,
    idempotency_key UUID NOT NULL,
    request_digest TEXT NOT NULL CHECK(request_digest ~ '^[a-f0-9]{64}$'),
    binding JSONB NOT NULL,
    binding_sha256 TEXT NOT NULL CHECK(binding_sha256 ~ '^[a-f0-9]{64}$'),
    phase TEXT NOT NULL DEFAULT 'prepared' CHECK(phase IN ('prepared','source_quiescent','export_verified','target_restored_paused','qualified','source_retained','aborted')),
    revision BIGINT NOT NULL DEFAULT 1 CHECK(revision>0),
    export_proof JSONB,
    cutover_receipt JSONB,
    abort_receipt JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(organisation_id,id),
    UNIQUE(actor_user_id,idempotency_key),
    CHECK(source_instance_id<>target_instance_id),
    FOREIGN KEY(organisation_id,source_instance_id) REFERENCES management_instances(organisation_id,id),
    FOREIGN KEY(organisation_id,target_instance_id) REFERENCES management_instances(organisation_id,id),
    FOREIGN KEY(organisation_id,source_instance_id,backup_id) REFERENCES management_backups(organisation_id,instance_id,id)
);
CREATE TABLE management_instance_import_locks (
    instance_id UUID PRIMARY KEY,
    organisation_id UUID NOT NULL,
    import_id UUID NOT NULL,
    FOREIGN KEY(organisation_id,instance_id) REFERENCES management_instances(organisation_id,id),
    FOREIGN KEY(organisation_id,import_id) REFERENCES management_instance_imports(organisation_id,id)
);
CREATE TABLE management_instance_import_receipts (
    import_id UUID NOT NULL REFERENCES management_instance_imports(id),
    operation_id UUID NOT NULL UNIQUE REFERENCES management_operations(id),
    step TEXT NOT NULL CHECK(step IN ('quiesce','export','restore','qualify')),
    receipt JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY(import_id,step)
);
CREATE TABLE management_instance_routes (
    organisation_id UUID NOT NULL,
    source_instance_id UUID PRIMARY KEY,
    target_instance_id UUID NOT NULL,
    revision BIGINT NOT NULL DEFAULT 1 CHECK(revision>0),
    import_id UUID REFERENCES management_instance_imports(id),
    FOREIGN KEY(organisation_id,source_instance_id) REFERENCES management_instances(organisation_id,id),
    FOREIGN KEY(organisation_id,target_instance_id) REFERENCES management_instances(organisation_id,id)
);
ALTER TABLE management_operations ADD COLUMN instance_import_id UUID REFERENCES management_instance_imports(id);

CREATE FUNCTION guard_instance_import_binding() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
    IF NEW.organisation_id IS DISTINCT FROM OLD.organisation_id OR NEW.source_instance_id IS DISTINCT FROM OLD.source_instance_id
      OR NEW.target_instance_id IS DISTINCT FROM OLD.target_instance_id OR NEW.actor_user_id IS DISTINCT FROM OLD.actor_user_id
      OR NEW.backup_id IS DISTINCT FROM OLD.backup_id OR NEW.request_digest IS DISTINCT FROM OLD.request_digest
      OR NEW.idempotency_key IS DISTINCT FROM OLD.idempotency_key OR NEW.binding IS DISTINCT FROM OLD.binding
      OR NEW.binding_sha256 IS DISTINCT FROM OLD.binding_sha256 THEN RAISE EXCEPTION 'instance import binding is immutable'; END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER immutable_instance_import BEFORE UPDATE ON management_instance_imports FOR EACH ROW EXECUTE FUNCTION guard_instance_import_binding();
CREATE FUNCTION guard_instance_import_operation() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
    IF TG_OP='UPDATE' AND NEW.instance_import_id IS DISTINCT FROM OLD.instance_import_id THEN
      RAISE EXCEPTION 'import operation binding is immutable'; END IF;
    IF EXISTS(SELECT 1 FROM management_instance_import_locks l WHERE l.instance_id=NEW.instance_id
      AND l.import_id IS DISTINCT FROM NEW.instance_import_id) THEN RAISE EXCEPTION 'instance is locked by a durable import'; END IF;
    IF NEW.instance_import_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM management_instance_imports m
      WHERE m.id=NEW.instance_import_id AND m.organisation_id=NEW.organisation_id
      AND ((NEW.instance_id=m.source_instance_id AND NEW.action IN ('import_quiesce','import_export'))
        OR (NEW.instance_id=m.target_instance_id AND NEW.action IN ('import_restore','import_qualify')))) THEN
      RAISE EXCEPTION 'invalid import operation scope'; END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER scoped_instance_import_operation BEFORE INSERT OR UPDATE ON management_operations FOR EACH ROW EXECUTE FUNCTION guard_instance_import_operation();
CREATE FUNCTION guard_import_phase_receipt() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
    RAISE EXCEPTION 'import phase receipt is append-only';
END $$;
CREATE TRIGGER immutable_import_phase_receipt BEFORE UPDATE OR DELETE ON management_instance_import_receipts FOR EACH ROW EXECUTE FUNCTION guard_import_phase_receipt();

-- Copied execution history is retained, but never becomes local Resume authority.
CREATE TABLE IF NOT EXISTS run_instance_import_origins (
    run_id UUID PRIMARY KEY REFERENCES runs(id),
    import_id UUID NOT NULL,
    organisation_id UUID NOT NULL,
    source_instance_id UUID NOT NULL,
    target_instance_id UUID NOT NULL,
    source_backup_id UUID NOT NULL,
    source_manifest_sha256 TEXT NOT NULL CHECK(source_manifest_sha256 ~ '^[a-f0-9]{64}$'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK(source_instance_id<>target_instance_id)
);
CREATE FUNCTION guard_import_history() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
    RAISE EXCEPTION 'imported run provenance and Resume fence are immutable';
END $$;
CREATE TRIGGER immutable_import_run_origin BEFORE UPDATE OR DELETE ON run_instance_import_origins FOR EACH ROW EXECUTE FUNCTION guard_import_history();

-- Every worker/executor path shares the imported-history activation fence.
CREATE OR REPLACE FUNCTION guard_imported_run_activation() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
    IF EXISTS(SELECT 1 FROM run_instance_import_origins o WHERE o.run_id=NEW.id)
      AND (NEW.state NOT IN ('completed','failed','canceled','expired','interrupted')
        OR NEW.lease_owner IS NOT NULL OR NEW.lease_token IS NOT NULL OR NEW.lease_expires_at IS NOT NULL) THEN
      RAISE EXCEPTION 'imported historical run cannot acquire execution authority'; END IF;
    RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS imported_run_activation_fence ON runs;
CREATE TRIGGER imported_run_activation_fence BEFORE UPDATE ON runs FOR EACH ROW EXECUTE FUNCTION guard_imported_run_activation();
