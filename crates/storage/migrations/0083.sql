CREATE TABLE android_personal_provider_versions (
 profile_id TEXT NOT NULL REFERENCES android_personal_provider_profiles(id),
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 revision BIGINT NOT NULL CHECK(revision>0),
 payload TEXT NOT NULL,
 scope TEXT,
 sealed TEXT,
 revoked BIGINT NOT NULL DEFAULT 0 CHECK(revoked IN(0,1)),
 PRIMARY KEY(profile_id,revision)
);
CREATE INDEX android_personal_provider_versions_owner ON android_personal_provider_versions(owner_id,profile_id);
INSERT INTO android_personal_provider_versions(profile_id,owner_id,revision,payload,scope,sealed,revoked)
 SELECT p.id,p.owner_id,p.revision,p.payload,s.scope,s.sealed,p.deleted
 FROM android_personal_provider_profiles p LEFT JOIN android_personal_provider_secrets s ON s.profile_id=p.id AND s.profile_revision=p.revision;
