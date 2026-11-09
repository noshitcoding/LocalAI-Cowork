CREATE TABLE android_personal_provider_profiles (
 id TEXT PRIMARY KEY,
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 revision BIGINT NOT NULL CHECK(revision>0),
 payload TEXT NOT NULL,
 deleted BIGINT NOT NULL DEFAULT 0 CHECK(deleted IN(0,1))
);
CREATE INDEX android_personal_provider_owner ON android_personal_provider_profiles(owner_id,deleted);
CREATE TABLE android_personal_provider_secrets (
 profile_id TEXT PRIMARY KEY REFERENCES android_personal_provider_profiles(id),
 profile_revision BIGINT NOT NULL CHECK(profile_revision>0),
 scope TEXT NOT NULL,
 sealed TEXT NOT NULL
);
CREATE TABLE android_personal_provider_operations (
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 operation_key TEXT NOT NULL,
 request_mac TEXT NOT NULL,
 receipt TEXT NOT NULL,
 PRIMARY KEY(owner_id,operation_key)
);
