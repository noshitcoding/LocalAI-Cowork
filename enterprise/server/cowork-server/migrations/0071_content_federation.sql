-- Management stores current authority and original receipts, never customer bundle bytes.
CREATE TABLE management_content_grants (
 id UUID PRIMARY KEY, source_organisation_id UUID NOT NULL, source_instance_id UUID NOT NULL,
 target_organisation_id UUID NOT NULL, target_instance_id UUID NOT NULL,
 publisher_user_id UUID NOT NULL REFERENCES users(id), publisher_session_id UUID NOT NULL REFERENCES auth_sessions(id),
 recipient_user_id UUID NOT NULL REFERENCES users(id), recipient_session_id UUID REFERENCES auth_sessions(id),
 revision BIGINT NOT NULL CHECK(revision>0), epoch UUID NOT NULL, state TEXT NOT NULL CHECK(state IN ('pending','active','revoked')),
 source_instance_revision BIGINT NOT NULL, target_instance_revision BIGINT NOT NULL, source_epoch UUID NOT NULL, target_epoch UUID NOT NULL,
 source_policy_sha256 TEXT NOT NULL, target_policy_sha256 TEXT NOT NULL,
 project_id UUID NOT NULL, version_id UUID NOT NULL, paths JSONB NOT NULL, expires_at TIMESTAMPTZ NOT NULL,
 created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 FOREIGN KEY(source_organisation_id,source_instance_id) REFERENCES management_instances(organisation_id,id),
 FOREIGN KEY(target_organisation_id,target_instance_id) REFERENCES management_instances(organisation_id,id),
 FOREIGN KEY(source_organisation_id,publisher_user_id) REFERENCES management_members(organisation_id,user_id),
 FOREIGN KEY(target_organisation_id,recipient_user_id) REFERENCES management_members(organisation_id,user_id),
 CHECK(source_instance_id<>target_instance_id)
);
CREATE TABLE management_content_operations (
 actor_user_id UUID NOT NULL REFERENCES users(id),request_id UUID NOT NULL,kind TEXT NOT NULL,grant_id UUID NOT NULL REFERENCES management_content_grants(id),request_sha256 TEXT NOT NULL,receipt JSONB NOT NULL,created_at TIMESTAMPTZ NOT NULL DEFAULT now(),PRIMARY KEY(actor_user_id,request_id)
);
CREATE TABLE management_content_exports (
 transfer_id UUID PRIMARY KEY,grant_id UUID NOT NULL REFERENCES management_content_grants(id),grant_revision BIGINT NOT NULL,grant_epoch UUID NOT NULL,bundle_sha256 TEXT NOT NULL,size BIGINT NOT NULL CHECK(size BETWEEN 0 AND 25165824),source_receipt JSONB NOT NULL,target_receipt JSONB,created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- Each instance owns its real local object and session FKs. Remote IDs are provenance only.
CREATE TABLE instance_content_exports (
 transfer_id UUID PRIMARY KEY,grant_id UUID NOT NULL,grant_revision BIGINT NOT NULL,grant_epoch UUID NOT NULL,
 source_instance_id UUID NOT NULL,source_epoch UUID NOT NULL,publisher_user_id UUID NOT NULL REFERENCES users(id),publisher_session_id UUID NOT NULL REFERENCES auth_sessions(id),
 project_id UUID NOT NULL REFERENCES projects(id),version_id UUID NOT NULL REFERENCES project_versions(id),original_owner_user_id UUID NOT NULL REFERENCES users(id),bundle_sha256 TEXT NOT NULL,
 ciphertext BYTEA NOT NULL,encrypted_data_key BYTEA NOT NULL,nonce BYTEA NOT NULL,wrap_nonce BYTEA NOT NULL,source_receipt JSONB NOT NULL,management_receipt JSONB,created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE instance_content_imports (
 transfer_id UUID PRIMARY KEY,grant_id UUID NOT NULL,grant_revision BIGINT NOT NULL,grant_epoch UUID NOT NULL,
 source_instance_id UUID NOT NULL,target_instance_id UUID NOT NULL,target_epoch UUID NOT NULL,recipient_user_id UUID NOT NULL REFERENCES users(id),recipient_session_id UUID NOT NULL REFERENCES auth_sessions(id),
 bundle_sha256 TEXT NOT NULL,phase TEXT NOT NULL CHECK(phase IN ('prepared','staged','committed')),
 project_id UUID NOT NULL REFERENCES projects(id),version_id UUID REFERENCES project_versions(id),snapshot_manifest_id UUID REFERENCES snapshot_manifests(id),
 ciphertext BYTEA NOT NULL,encrypted_data_key BYTEA NOT NULL,nonce BYTEA NOT NULL,wrap_nonce BYTEA NOT NULL,local_receipt JSONB,management_receipt JSONB,created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX management_content_recipient ON management_content_grants(target_organisation_id,recipient_user_id,state);
CREATE FUNCTION content_federation_original() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_TABLE_NAME='management_content_grants' THEN
  IF (to_jsonb(NEW)-'revision'-'state'-'recipient_session_id') IS DISTINCT FROM (to_jsonb(OLD)-'revision'-'state'-'recipient_session_id')
   OR (OLD.recipient_session_id IS NOT NULL AND NEW.recipient_session_id IS DISTINCT FROM OLD.recipient_session_id)
   OR NOT ((NEW.state=OLD.state AND NEW.revision=OLD.revision)
     OR (OLD.state='pending' AND NEW.state IN ('active','revoked') AND NEW.revision=OLD.revision+1)
     OR (OLD.state='active' AND NEW.state='revoked' AND NEW.revision=OLD.revision+1)) THEN RAISE EXCEPTION 'immutable original content grant'; END IF;
 ELSIF TG_TABLE_NAME='instance_content_exports' THEN
  IF (to_jsonb(NEW)-'management_receipt') IS DISTINCT FROM (to_jsonb(OLD)-'management_receipt')
   OR (OLD.management_receipt IS NOT NULL AND NEW.management_receipt IS DISTINCT FROM OLD.management_receipt) THEN RAISE EXCEPTION 'immutable original source export'; END IF;
 ELSIF TG_TABLE_NAME='instance_content_imports' THEN
  IF (to_jsonb(NEW)-'phase'-'version_id'-'snapshot_manifest_id'-'local_receipt'-'management_receipt') IS DISTINCT FROM (to_jsonb(OLD)-'phase'-'version_id'-'snapshot_manifest_id'-'local_receipt'-'management_receipt')
   OR (OLD.version_id IS NOT NULL AND NEW.version_id IS DISTINCT FROM OLD.version_id)
   OR (OLD.snapshot_manifest_id IS NOT NULL AND NEW.snapshot_manifest_id IS DISTINCT FROM OLD.snapshot_manifest_id)
   OR (OLD.local_receipt IS NOT NULL AND NEW.local_receipt IS DISTINCT FROM OLD.local_receipt)
   OR (OLD.management_receipt IS NOT NULL AND NEW.management_receipt IS DISTINCT FROM OLD.management_receipt)
   OR NOT (NEW.phase=OLD.phase OR (OLD.phase='prepared' AND NEW.phase='staged') OR (OLD.phase='staged' AND NEW.phase='committed')) THEN RAISE EXCEPTION 'immutable original target import'; END IF;
  IF NEW.phase IN ('staged','committed') AND NOT EXISTS(SELECT 1 FROM project_versions v JOIN snapshot_manifests s ON s.id=v.snapshot_manifest_id JOIN projects p ON p.id=v.project_id WHERE v.id=NEW.version_id AND v.project_id=NEW.project_id AND s.id=NEW.snapshot_manifest_id AND s.project_id=NEW.project_id AND p.owner_user_id=NEW.recipient_user_id) THEN RAISE EXCEPTION 'target import needs actual matching owned project/version/snapshot'; END IF;
  IF NEW.phase='committed' AND (NEW.management_receipt IS NULL OR NEW.management_receipt->'original_receipt' IS DISTINCT FROM NEW.local_receipt) THEN RAISE EXCEPTION 'target import needs original management receipt'; END IF;
 ELSIF TG_TABLE_NAME='management_content_exports' THEN
  IF (to_jsonb(NEW)-'target_receipt') IS DISTINCT FROM (to_jsonb(OLD)-'target_receipt')
   OR (OLD.target_receipt IS NOT NULL AND NEW.target_receipt IS DISTINCT FROM OLD.target_receipt) THEN RAISE EXCEPTION 'immutable original content transfer receipt'; END IF;
 ELSE RAISE EXCEPTION 'immutable original content operation';
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER management_content_grant_original BEFORE UPDATE ON management_content_grants FOR EACH ROW EXECUTE FUNCTION content_federation_original();
CREATE TRIGGER management_content_export_original BEFORE UPDATE ON management_content_exports FOR EACH ROW EXECUTE FUNCTION content_federation_original();
CREATE TRIGGER management_content_operation_original BEFORE UPDATE ON management_content_operations FOR EACH ROW EXECUTE FUNCTION content_federation_original();
CREATE TRIGGER instance_content_export_original BEFORE UPDATE ON instance_content_exports FOR EACH ROW EXECUTE FUNCTION content_federation_original();
CREATE TRIGGER instance_content_import_original BEFORE UPDATE ON instance_content_imports FOR EACH ROW EXECUTE FUNCTION content_federation_original();
