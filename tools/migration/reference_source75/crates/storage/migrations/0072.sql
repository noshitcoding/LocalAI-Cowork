-- Current actor grants over original private content. No credentials or old execution grants.
CREATE TABLE android_private_project_grants (
 project_id TEXT NOT NULL REFERENCES android_personal_projects(id),
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 user_id TEXT NOT NULL REFERENCES identity_subjects(id),
 permissions BIGINT NOT NULL CHECK (permissions IN (1,5,15)),
 revision BIGINT NOT NULL CHECK (revision>0),
 active BIGINT NOT NULL CHECK (active IN (0,1)),
 updated_at BIGINT NOT NULL,
 PRIMARY KEY(project_id,user_id), CHECK(owner_id<>user_id)
);
CREATE INDEX android_private_project_grants_user ON android_private_project_grants(user_id,active,project_id);
CREATE TABLE android_private_sharing_operations (
 actor_id TEXT NOT NULL REFERENCES identity_subjects(id),
 operation_key TEXT NOT NULL,
 project_id TEXT NOT NULL REFERENCES android_personal_projects(id),
 request_sha256 TEXT NOT NULL,
 receipt TEXT NOT NULL,
 created_at BIGINT NOT NULL,
 PRIMARY KEY(actor_id,operation_key)
);

CREATE TABLE android_personal_storage_quotas (
 actor_id TEXT PRIMARY KEY REFERENCES identity_subjects(id), revision BIGINT NOT NULL CHECK(revision>0),
 limit_bytes BIGINT NOT NULL CHECK(limit_bytes>=0), consumed_bytes BIGINT NOT NULL CHECK(consumed_bytes>=0),
 reserved_bytes BIGINT NOT NULL CHECK(reserved_bytes>=0)
);
CREATE TABLE android_administration_operations (
 actor_id TEXT NOT NULL REFERENCES identity_subjects(id), operation_key TEXT NOT NULL,
 purpose TEXT NOT NULL, request_sha256 TEXT NOT NULL, receipt TEXT NOT NULL, created_at BIGINT NOT NULL,
 PRIMARY KEY(actor_id,operation_key)
);
CREATE TABLE android_mobile_cache_imports (
 id TEXT PRIMARY KEY, owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 request_id TEXT NOT NULL, archive_sha256 TEXT NOT NULL, archive_size BIGINT NOT NULL CHECK(archive_size>0),
 revision BIGINT NOT NULL CHECK(revision>0), state TEXT NOT NULL CHECK(state IN('receiving','indexed','retained')),
 reserved_bytes BIGINT NOT NULL CHECK(reserved_bytes>=0), entry_count BIGINT NOT NULL CHECK(entry_count>=0),
 payload_sha256 TEXT, source TEXT, error TEXT, created_at BIGINT NOT NULL,
 UNIQUE(owner_id,request_id)
);
CREATE TABLE android_mobile_cache_chunks (
 import_id TEXT NOT NULL REFERENCES android_mobile_cache_imports(id),
 stream TEXT NOT NULL, ordinal BIGINT NOT NULL CHECK(ordinal>=0),
 digest TEXT NOT NULL, size BIGINT NOT NULL CHECK(size>=0 AND size<=1048576), sealed TEXT NOT NULL,
 PRIMARY KEY(import_id,stream,ordinal)
);
CREATE TABLE android_mobile_cache_entries (
 import_id TEXT NOT NULL REFERENCES android_mobile_cache_imports(id), ordinal BIGINT NOT NULL CHECK(ordinal>0),
 family TEXT NOT NULL CHECK(family IN('run','event','message','outbox')),
 source_group TEXT NOT NULL, source_id TEXT, size BIGINT NOT NULL CHECK(size>=0), digest TEXT NOT NULL,
 activation_receipt_sha256 TEXT, target_message_id TEXT, PRIMARY KEY(import_id,ordinal)
);
CREATE INDEX android_mobile_cache_owner ON android_mobile_cache_imports(owner_id,id);
CREATE TABLE android_support_grants (
 id TEXT PRIMARY KEY, issuer_id TEXT NOT NULL REFERENCES identity_subjects(id), user_id TEXT NOT NULL REFERENCES identity_subjects(id),
 project_id TEXT NOT NULL, organization_id TEXT REFERENCES identity_organizations(id),
 membership_id TEXT REFERENCES identity_memberships(id), original_membership_revision BIGINT CHECK(original_membership_revision IS NULL OR original_membership_revision>0), source_owner_id TEXT REFERENCES identity_subjects(id),
 grant_revision BIGINT NOT NULL CHECK(grant_revision>0), expires_at BIGINT NOT NULL,
 active BIGINT NOT NULL CHECK(active IN(0,1)), revision BIGINT NOT NULL CHECK(revision>0),
 reason TEXT NOT NULL, created_at BIGINT NOT NULL,
 CHECK((organization_id IS NULL AND membership_id IS NULL AND source_owner_id IS NOT NULL) OR (organization_id IS NOT NULL AND membership_id IS NOT NULL AND source_owner_id IS NULL))
);
CREATE TABLE android_tenant_support_sources (
 resource_id TEXT NOT NULL REFERENCES identity_resources(id), membership_id TEXT NOT NULL REFERENCES identity_memberships(id),
 support_id TEXT NOT NULL UNIQUE REFERENCES android_support_grants(id), PRIMARY KEY(resource_id,membership_id)
);
CREATE TABLE android_private_support_sources (
 project_id TEXT NOT NULL, user_id TEXT NOT NULL, support_id TEXT NOT NULL UNIQUE REFERENCES android_support_grants(id),
 PRIMARY KEY(project_id,user_id), FOREIGN KEY(project_id,user_id) REFERENCES android_private_project_grants(project_id,user_id)
);
CREATE TABLE android_executor_pool_profiles (
 id TEXT PRIMARY KEY, organization_id TEXT REFERENCES identity_organizations(id), owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 revision BIGINT NOT NULL CHECK(revision>0), active BIGINT NOT NULL CHECK(active IN(0,1)), name TEXT NOT NULL,
 kind TEXT NOT NULL CHECK(kind='managed_windows'), policy TEXT NOT NULL, created_at BIGINT NOT NULL
);
CREATE TABLE android_executor_pool_projects (
 pool_id TEXT NOT NULL REFERENCES android_executor_pool_profiles(id), project_id TEXT NOT NULL REFERENCES tenant_projects(id),
 organization_id TEXT NOT NULL REFERENCES identity_organizations(id), revision BIGINT NOT NULL CHECK(revision>0),
 active BIGINT NOT NULL CHECK(active IN(0,1)), PRIMARY KEY(pool_id,project_id)
);

CREATE TABLE android_executor_pool_runs (
 run_id TEXT PRIMARY KEY REFERENCES android_scoped_runs(id), pool_id TEXT NOT NULL REFERENCES android_executor_pool_profiles(id),
 project_id TEXT NOT NULL REFERENCES tenant_projects(id), stamp TEXT NOT NULL
);
CREATE INDEX android_executor_pool_run_scope ON android_executor_pool_runs(pool_id,run_id);

CREATE TABLE android_executor_sync_grants (
 executor_id TEXT NOT NULL REFERENCES android_devices(id), project_id TEXT NOT NULL,
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id), issuer_id TEXT NOT NULL REFERENCES identity_subjects(id),
 source_owner_id TEXT NOT NULL REFERENCES identity_subjects(id), organization_id TEXT REFERENCES identity_organizations(id),
 issuer_session_id TEXT NOT NULL REFERENCES identity_sessions(id), issuer_session_revision BIGINT NOT NULL CHECK(issuer_session_revision>0),
 source_authority TEXT NOT NULL, permissions BIGINT NOT NULL CHECK(permissions IN(1,3)),
 revision BIGINT NOT NULL CHECK(revision>0), active BIGINT NOT NULL CHECK(active IN(0,1)), expires_at BIGINT NOT NULL,
 PRIMARY KEY(executor_id,project_id)
);
CREATE INDEX android_executor_sync_owner ON android_executor_sync_grants(owner_id,executor_id,project_id);
