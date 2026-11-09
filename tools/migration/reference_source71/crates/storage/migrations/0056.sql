CREATE TABLE actor_model_registry_guard (
 id BIGINT PRIMARY KEY CHECK(id=1),
 revision BIGINT NOT NULL
);
INSERT INTO actor_model_registry_guard(id,revision) VALUES(1,1);
CREATE TABLE actor_model_registry (
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 project_id TEXT NOT NULL REFERENCES android_personal_projects(id),
 id TEXT NOT NULL,
 version BIGINT NOT NULL CHECK(version>0),
 provider_id TEXT NOT NULL REFERENCES android_personal_provider_profiles(id),
 provider_revision BIGINT NOT NULL CHECK(provider_revision>0),
 provider_binding_sha256 TEXT NOT NULL,
 provider_sha256 TEXT NOT NULL,
 bundle_sha256 TEXT NOT NULL,
 payload TEXT NOT NULL,
 receipt TEXT NOT NULL,
 registered_at BIGINT NOT NULL,
 revoked BIGINT NOT NULL DEFAULT 0 CHECK(revoked IN(0,1)),
 PRIMARY KEY(owner_id,project_id,id,version)
);
CREATE INDEX actor_model_registry_profile ON actor_model_registry(owner_id,project_id,provider_id,revoked);
CREATE TABLE actor_model_registry_operations (
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 project_id TEXT NOT NULL REFERENCES android_personal_projects(id),
 idempotency_key TEXT NOT NULL,
 request_sha256 TEXT NOT NULL,
 receipt TEXT NOT NULL,
 PRIMARY KEY(owner_id,idempotency_key)
);
