CREATE TABLE actor_chat_configurations (
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 thread_id TEXT NOT NULL REFERENCES android_personal_threads(id),
 project_id TEXT NOT NULL REFERENCES android_personal_projects(id),
 revision BIGINT NOT NULL CHECK(revision>0),
 payload TEXT NOT NULL,
 PRIMARY KEY(owner_id,thread_id)
);
CREATE TABLE actor_chat_configuration_receipts (
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 operation_key TEXT NOT NULL,
 thread_id TEXT NOT NULL REFERENCES android_personal_threads(id),
 request_sha TEXT NOT NULL,
 receipt TEXT NOT NULL,
 PRIMARY KEY(owner_id,operation_key)
);
