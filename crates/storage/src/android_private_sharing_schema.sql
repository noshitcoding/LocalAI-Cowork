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
