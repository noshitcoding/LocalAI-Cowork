CREATE TABLE android_auth_transport_operations (
 operation_id TEXT PRIMARY KEY,
 purpose TEXT NOT NULL,
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 source_session_id TEXT NOT NULL REFERENCES identity_sessions(id),
 source_session_revision BIGINT NOT NULL CHECK(source_session_revision>0),
 target_session_id TEXT NOT NULL REFERENCES identity_sessions(id),
 target_session_revision BIGINT NOT NULL CHECK(target_session_revision>0),
 source_grant_id TEXT,
 device_id TEXT NOT NULL,
 source_token_sha256 TEXT NOT NULL,
 request_mac TEXT NOT NULL,
 replay_until BIGINT NOT NULL,
 scope TEXT NOT NULL,
 sealed TEXT NOT NULL,
 created_at BIGINT NOT NULL
);
CREATE UNIQUE INDEX android_auth_transport_source ON android_auth_transport_operations(source_grant_id) WHERE source_grant_id IS NOT NULL;
CREATE INDEX android_auth_transport_owner ON android_auth_transport_operations(owner_id,created_at);
CREATE TABLE android_auth_management_operations (
 operation_id TEXT PRIMARY KEY,
 request_sha256 TEXT NOT NULL,
 receipt TEXT NOT NULL,
 created_at BIGINT NOT NULL
);
CREATE TABLE android_auth_passkey_proofs (
 session_id TEXT PRIMARY KEY REFERENCES identity_sessions(id),
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 passkey_id TEXT NOT NULL,
 credential_revision BIGINT NOT NULL CHECK(credential_revision>0),
 verified_at BIGINT NOT NULL
);
CREATE TABLE native_auth_transport_journal (
 operation_id TEXT PRIMARY KEY,
 instance_id TEXT NOT NULL,
 endpoint TEXT NOT NULL,
 purpose TEXT NOT NULL CHECK(purpose IN('native_token','registration_ack')),
 state TEXT NOT NULL CHECK(state IN('prepared','response_saved','acknowledged','abandoned')),
 request_sha256 TEXT NOT NULL,
 actor_id TEXT,
 session_id TEXT,
 scope TEXT NOT NULL,
 sealed TEXT NOT NULL,
 created_at BIGINT NOT NULL,
 updated_at BIGINT NOT NULL
);
CREATE INDEX native_auth_transport_pending ON native_auth_transport_journal(instance_id,state,endpoint);
