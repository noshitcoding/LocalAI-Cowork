-- Additive. Existing NULL origins remain explicitly unclassified. No logout,
-- ownership reconstruction, local-default backfill or account deletion occurs.
ALTER TABLE auth_sessions ADD COLUMN auth_origin JSONB;
ALTER TABLE native_authorization_codes ADD COLUMN auth_origin JSONB;
ALTER TABLE auth_sessions ADD CONSTRAINT auth_session_origin_object CHECK(auth_origin IS NULL OR jsonb_typeof(auth_origin)='object');
ALTER TABLE native_authorization_codes ADD CONSTRAINT native_code_origin_object CHECK(auth_origin IS NULL OR jsonb_typeof(auth_origin)='object');
ALTER TABLE management_members ADD COLUMN directory_managed BOOLEAN NOT NULL DEFAULT FALSE;

CREATE TABLE directory_connectors (
    id UUID PRIMARY KEY,
    organisation_id UUID NOT NULL REFERENCES management_organisations(id),
    provider_id TEXT NOT NULL,
    issuer TEXT NOT NULL,
    client_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('scim','ldap')),
    fingerprint TEXT NOT NULL CHECK(fingerprint ~ '^[0-9a-f]{64}$'),
    revision BIGINT NOT NULL CHECK(revision>0),
    epoch UUID NOT NULL,
    active BOOLEAN NOT NULL,
    credential_ciphertext BYTEA NOT NULL,
    credential_key BYTEA NOT NULL,
    credential_nonce BYTEA NOT NULL CHECK(octet_length(credential_nonce)=12),
    credential_wrap_nonce BYTEA NOT NULL CHECK(octet_length(credential_wrap_nonce)=12),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(id,organisation_id)
);
CREATE TABLE directory_settings (
    connector_id UUID PRIMARY KEY REFERENCES directory_connectors(id),
    revision BIGINT NOT NULL DEFAULT 1 CHECK(revision>0),
    epoch UUID NOT NULL,
    state TEXT NOT NULL DEFAULT 'paused' CHECK(state IN ('paused','preview','active')),
    provider_fingerprint TEXT NOT NULL,
    sso_revision BIGINT NOT NULL,
    policy_revision BIGINT NOT NULL,
    adopt_provider_memberships BOOLEAN NOT NULL DEFAULT FALSE,
    provision_accounts BOOLEAN NOT NULL DEFAULT FALSE,
    retention_days INTEGER NOT NULL CHECK(retention_days BETWEEN 1 AND 3650),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE directory_operator_operations (
    id UUID PRIMARY KEY,
    connector_id UUID NOT NULL REFERENCES directory_connectors(id),
    expected_revision BIGINT NOT NULL,
    request_sha256 TEXT NOT NULL,
    receipt JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(connector_id,expected_revision)
);
CREATE TABLE directory_settings_operations (
    id UUID PRIMARY KEY,
    connector_id UUID NOT NULL REFERENCES directory_connectors(id),
    expected_revision BIGINT NOT NULL,
    actor_user_id UUID NOT NULL REFERENCES users(id),
    request_sha256 TEXT NOT NULL,
    receipt JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(connector_id,expected_revision)
);
CREATE TABLE directory_resources (
    id UUID PRIMARY KEY,
    connector_id UUID NOT NULL REFERENCES directory_connectors(id),
    kind TEXT NOT NULL CHECK(kind IN ('User','Group')),
    external_key TEXT NOT NULL CHECK(length(external_key) BETWEEN 1 AND 512),
    subject TEXT,
    user_id UUID REFERENCES users(id),
    identity_id UUID REFERENCES oidc_identities(id),
    owns_identity BOOLEAN NOT NULL DEFAULT FALSE,
    revision BIGINT NOT NULL DEFAULT 1 CHECK(revision>0),
    active BOOLEAN NOT NULL DEFAULT TRUE,
    deleted BOOLEAN NOT NULL DEFAULT FALSE,
    name_hash TEXT NOT NULL,
    dn_hash TEXT,
    payload_sha256 TEXT NOT NULL CHECK(payload_sha256 ~ '^[0-9a-f]{64}$'),
    payload_ciphertext BYTEA,
    payload_key BYTEA,
    payload_nonce BYTEA CHECK(payload_nonce IS NULL OR octet_length(payload_nonce)=12),
    payload_wrap_nonce BYTEA CHECK(payload_wrap_nonce IS NULL OR octet_length(payload_wrap_nonce)=12),
    observed_registry_epoch UUID NOT NULL,
    observed_settings_epoch UUID NOT NULL,
    observed_settings_revision BIGINT NOT NULL,
    observed_scan UUID,
    quarantined_legacy BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    UNIQUE(connector_id,kind,external_key),
    UNIQUE(connector_id,id),
    CHECK(kind<>'User' OR subject IS NOT NULL)
);
CREATE INDEX directory_resources_scan ON directory_resources(connector_id,kind,observed_scan,id);
CREATE INDEX directory_resources_user ON directory_resources(user_id,identity_id);
CREATE INDEX directory_resources_name ON directory_resources(connector_id,kind,name_hash);
CREATE TABLE directory_group_members (
    connector_id UUID NOT NULL,
    group_id UUID NOT NULL,
    user_resource_id UUID NOT NULL,
    PRIMARY KEY(group_id,user_resource_id),
    FOREIGN KEY(connector_id,group_id) REFERENCES directory_resources(connector_id,id),
    FOREIGN KEY(connector_id,user_resource_id) REFERENCES directory_resources(connector_id,id)
);
CREATE TABLE directory_membership_contributions (
    connector_id UUID NOT NULL,
    organisation_id UUID NOT NULL,
    user_id UUID NOT NULL REFERENCES users(id),
    user_resource_id UUID NOT NULL,
    identity_id UUID NOT NULL REFERENCES oidc_identities(id),
    subject_sha256 TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    provider_fingerprint TEXT NOT NULL,
    sso_revision BIGINT NOT NULL,
    policy_revision BIGINT NOT NULL,
    registry_epoch UUID NOT NULL,
    settings_epoch UUID NOT NULL,
    settings_revision BIGINT NOT NULL,
    role TEXT CHECK(role IN ('admin','member','auditor')),
    active BOOLEAN NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY(connector_id,user_id,user_resource_id),
    FOREIGN KEY(connector_id,organisation_id) REFERENCES directory_connectors(id,organisation_id),
    FOREIGN KEY(connector_id,user_resource_id) REFERENCES directory_resources(connector_id,id)
);
CREATE TABLE directory_operations (
    id UUID PRIMARY KEY,
    connector_id UUID NOT NULL REFERENCES directory_connectors(id),
    request_key TEXT NOT NULL CHECK(length(request_key) BETWEEN 1 AND 256),
    request_sha256 TEXT NOT NULL CHECK(request_sha256 ~ '^[0-9a-f]{64}$'),
    registry_epoch UUID NOT NULL,
    settings_epoch UUID NOT NULL,
    resource_id UUID,
    receipt JSONB NOT NULL,
    response_ciphertext BYTEA,
    response_key BYTEA,
    response_nonce BYTEA CHECK(response_nonce IS NULL OR octet_length(response_nonce)=12),
    response_wrap_nonce BYTEA CHECK(response_wrap_nonce IS NULL OR octet_length(response_wrap_nonce)=12),
    retained_until TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(connector_id,request_key)
);
CREATE TABLE directory_sync_jobs (
    connector_id UUID PRIMARY KEY REFERENCES directory_connectors(id),
    revision BIGINT NOT NULL DEFAULT 1,
    cycle_id UUID NOT NULL,
    registry_epoch UUID NOT NULL,
    settings_epoch UUID NOT NULL,
    phase TEXT NOT NULL CHECK(phase IN ('users','groups','reconcile_users','reconcile_groups','idle')),
    cursor_ciphertext BYTEA,
    cursor_key BYTEA,
    cursor_nonce BYTEA CHECK(cursor_nonce IS NULL OR octet_length(cursor_nonce)=12),
    cursor_wrap_nonce BYTEA CHECK(cursor_wrap_nonce IS NULL OR octet_length(cursor_wrap_nonce)=12),
    completed_entries BIGINT NOT NULL DEFAULT 0,
    completed_pages BIGINT NOT NULL DEFAULT 0,
    lease_id UUID,
    lease_until TIMESTAMPTZ,
    next_poll_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    failures INTEGER NOT NULL DEFAULT 0,
    last_success_at TIMESTAMPTZ,
    last_error_code TEXT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE directory_containment_plans (
    id UUID PRIMARY KEY,
    connector_id UUID NOT NULL REFERENCES directory_connectors(id),
    settings_revision BIGINT NOT NULL,
    registry_epoch UUID NOT NULL,
    settings_epoch UUID NOT NULL,
    provider_fingerprint TEXT NOT NULL,
    sso_revision BIGINT NOT NULL,
    policy_revision BIGINT NOT NULL,
    inventory_sha256 TEXT NOT NULL CHECK(inventory_sha256 ~ '^[0-9a-f]{64}$'),
    inventory JSONB NOT NULL,
    requested_by UUID NOT NULL REFERENCES users(id),
    expires_at TIMESTAMPTZ NOT NULL,
    receipt JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE directory_containment_users (
    connector_id UUID NOT NULL REFERENCES directory_connectors(id),
    user_id UUID NOT NULL REFERENCES users(id),
    identity_id UUID NOT NULL REFERENCES oidc_identities(id),
    plan_id UUID NOT NULL REFERENCES directory_containment_plans(id),
    confirmed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY(connector_id,user_id,identity_id)
);
ALTER TABLE oidc_identities DROP CONSTRAINT oidc_identities_binding_method_check;
ALTER TABLE oidc_identities ADD CONSTRAINT oidc_identities_binding_method_check
    CHECK(binding_method IN ('legacy_unverified','provisioned','authenticated_session','directory_provisioned'));
ALTER TABLE oidc_identities ADD COLUMN directory_owner_connector_id UUID REFERENCES directory_connectors(id);
ALTER TABLE oidc_identities ADD COLUMN directory_owner_resource_id UUID;
ALTER TABLE oidc_identities ADD CONSTRAINT oidc_directory_original_owner
    FOREIGN KEY(directory_owner_connector_id,directory_owner_resource_id) REFERENCES directory_resources(connector_id,id);
CREATE INDEX auth_session_directory_origin ON auth_sessions((auth_origin->'directory'->>'connector_id'),user_id) WHERE revoked_at IS NULL;

-- Current contributors alone determine a directory-managed projection. Old API
-- replicas cannot promote it using stale signed-group login upserts.
CREATE FUNCTION directory_effective_role(p_org UUID,p_user UUID) RETURNS TEXT LANGUAGE SQL STABLE AS $$
    SELECT mapping->>'role' FROM directory_membership_contributions x
    JOIN directory_connectors c ON c.id=x.connector_id AND c.organisation_id=x.organisation_id
    JOIN directory_settings d ON d.connector_id=c.id
    JOIN directory_resources r ON r.id=x.user_resource_id AND r.connector_id=c.id
    JOIN oidc_identities i ON i.id=x.identity_id AND i.user_id=x.user_id AND i.issuer=c.issuer AND i.subject=r.subject
    JOIN directory_group_members gm ON gm.connector_id=c.id AND gm.user_resource_id=r.id
    JOIN directory_resources g ON g.connector_id=c.id AND g.id=gm.group_id AND g.kind='Group'
    JOIN management_sso s ON s.organisation_id=c.organisation_id
    JOIN management_organisations o ON o.id=c.organisation_id
    CROSS JOIN LATERAL jsonb_array_elements(s.group_mappings) mapping
    WHERE x.organisation_id=p_org AND x.user_id=p_user AND x.active AND c.active
      AND d.state='active' AND r.active AND NOT r.deleted AND NOT r.quarantined_legacy
      AND s.enabled AND s.provider_id=x.provider_id AND s.provider_fingerprint=x.provider_fingerprint
      AND c.provider_id=x.provider_id AND r.user_id=x.user_id AND r.identity_id=x.identity_id
      AND s.revision=x.sso_revision AND (o.policy->>'revision')::BIGINT=x.policy_revision
      AND d.provider_fingerprint=s.provider_fingerprint AND d.sso_revision=s.revision
      AND d.policy_revision=(o.policy->>'revision')::BIGINT
      AND i.binding_method<>'legacy_unverified'
      AND x.subject_sha256=encode(sha256(convert_to(r.subject,'UTF8')),'hex')
      AND d.revision=x.settings_revision AND d.epoch=x.settings_epoch AND c.epoch=x.registry_epoch
      AND r.observed_registry_epoch=c.epoch AND r.observed_settings_epoch=d.epoch
      AND r.observed_settings_revision=d.revision
      AND g.active AND NOT g.deleted AND g.observed_registry_epoch=c.epoch
      AND g.observed_settings_epoch=d.epoch AND g.observed_settings_revision=d.revision
      AND g.name_hash=encode(sha256(convert_to(mapping->>'group','UTF8')),'hex')
      AND mapping->>'role' IN ('admin','member','auditor')
    ORDER BY CASE mapping->>'role' WHEN 'admin' THEN 3 WHEN 'member' THEN 2 WHEN 'auditor' THEN 1 ELSE 0 END DESC
    LIMIT 1
$$;
CREATE FUNCTION directory_guard_membership() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.directory_managed THEN
        NEW.role=coalesce(directory_effective_role(NEW.organisation_id,NEW.user_id),'suspended');
        NEW.content_admin=FALSE;
        IF NEW.sso_provider_id IS NULL THEN
            RAISE EXCEPTION 'directory-managed membership needs its original provider';
        END IF;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER directory_guard_membership BEFORE INSERT OR UPDATE ON management_members
    FOR EACH ROW EXECUTE FUNCTION directory_guard_membership();

CREATE FUNCTION directory_refresh_projection() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE p_org UUID; p_connector UUID;
BEGIN
    IF TG_TABLE_NAME='management_organisations' THEN p_org=NEW.id;
    ELSIF TG_TABLE_NAME IN ('management_sso','directory_connectors','directory_membership_contributions') THEN
        IF TG_OP='DELETE' THEN p_org=OLD.organisation_id; ELSE p_org=NEW.organisation_id; END IF;
    ELSE
        IF TG_OP='DELETE' THEN p_connector=OLD.connector_id; ELSE p_connector=NEW.connector_id; END IF;
        SELECT organisation_id INTO p_org FROM directory_connectors WHERE id=p_connector;
    END IF;
    UPDATE management_members SET role=coalesce(directory_effective_role(organisation_id,user_id),'suspended'),content_admin=FALSE
      WHERE organisation_id=p_org AND directory_managed;
    RETURN NULL;
END $$;
CREATE TRIGGER directory_refresh_org AFTER UPDATE OF policy ON management_organisations FOR EACH ROW EXECUTE FUNCTION directory_refresh_projection();
CREATE TRIGGER directory_refresh_sso AFTER UPDATE ON management_sso FOR EACH ROW EXECUTE FUNCTION directory_refresh_projection();
CREATE TRIGGER directory_refresh_registry AFTER UPDATE ON directory_connectors FOR EACH ROW EXECUTE FUNCTION directory_refresh_projection();
CREATE TRIGGER directory_refresh_settings AFTER UPDATE ON directory_settings FOR EACH ROW EXECUTE FUNCTION directory_refresh_projection();
CREATE TRIGGER directory_refresh_resources AFTER UPDATE ON directory_resources FOR EACH ROW EXECUTE FUNCTION directory_refresh_projection();
CREATE TRIGGER directory_refresh_contributors AFTER INSERT OR UPDATE OR DELETE ON directory_membership_contributions FOR EACH ROW EXECUTE FUNCTION directory_refresh_projection();
CREATE TRIGGER directory_refresh_groups AFTER INSERT OR UPDATE OR DELETE ON directory_group_members FOR EACH ROW EXECUTE FUNCTION directory_refresh_projection();

-- Only users explicitly contained (or newly directory-owned) are protected
-- from old replicas admitting new unclassified sign-ins. Existing rows are
-- untouched. Current local/other-provider sign-ins carry their own origin.
CREATE FUNCTION directory_guard_unknown_session() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    -- Serialize with explicit containment before reading its committed marker.
    -- A previously started old-replica INSERT must wait for the exact user;
    -- the volatile trigger then rereads the marker after that lock wait.
    PERFORM id FROM users WHERE id=NEW.user_id FOR SHARE;
    IF NEW.auth_origin IS NULL AND (
        EXISTS(SELECT 1 FROM directory_containment_users WHERE user_id=NEW.user_id)
        OR EXISTS(SELECT 1 FROM oidc_identities WHERE user_id=NEW.user_id AND directory_owner_connector_id IS NOT NULL)
    ) THEN
        IF TG_TABLE_NAME='auth_sessions' THEN
            NEW.revoked_at=now(); NEW.revoke_reason='directory_unclassified_origin';
        ELSE
            NEW.consumed_at=now();
        END IF;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER directory_guard_unknown_session BEFORE INSERT ON auth_sessions
    FOR EACH ROW EXECUTE FUNCTION directory_guard_unknown_session();
CREATE TRIGGER directory_guard_unknown_code BEFORE INSERT ON native_authorization_codes
    FOR EACH ROW EXECUTE FUNCTION directory_guard_unknown_session();
