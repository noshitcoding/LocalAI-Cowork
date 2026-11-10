CREATE TABLE android_auth_security_policy (
 id BIGINT PRIMARY KEY CHECK(id=1),
 revision BIGINT NOT NULL CHECK(revision>0),
 admin_required BIGINT NOT NULL CHECK(admin_required IN(0,1))
);
INSERT INTO android_auth_security_policy(id,revision,admin_required) VALUES(1,1,0);
CREATE TABLE android_auth_session_metadata (
 session_id TEXT PRIMARY KEY REFERENCES identity_sessions(id),
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 device_id TEXT NOT NULL,
 created_at BIGINT NOT NULL,
 last_used_at BIGINT NOT NULL,
 revoked_at BIGINT NOT NULL DEFAULT 0,
 revoke_reason TEXT NOT NULL DEFAULT '',
 verified_at BIGINT NOT NULL DEFAULT 0,
 factor_revision BIGINT NOT NULL DEFAULT 0,
 factor_method TEXT NOT NULL DEFAULT ''
);
CREATE INDEX android_auth_session_owner ON android_auth_session_metadata(owner_id,created_at);
CREATE TABLE android_auth_totp (
 owner_id TEXT PRIMARY KEY REFERENCES identity_subjects(id),
 revision BIGINT NOT NULL CHECK(revision>0),
 scope TEXT NOT NULL,
 sealed TEXT NOT NULL,
 enabled BIGINT NOT NULL CHECK(enabled IN(0,1)),
 enrollment_session_id TEXT NOT NULL REFERENCES identity_sessions(id),
 pending_expires_at BIGINT NOT NULL,
 last_counter BIGINT NOT NULL DEFAULT -1,
 failed_attempts BIGINT NOT NULL DEFAULT 0,
 failure_window_started_at BIGINT NOT NULL DEFAULT 0,
 blocked_until BIGINT NOT NULL DEFAULT 0,
 updated_at BIGINT NOT NULL
);
CREATE TABLE android_auth_recovery_codes (
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 factor_revision BIGINT NOT NULL CHECK(factor_revision>0),
 digest TEXT NOT NULL,
 used_at BIGINT NOT NULL DEFAULT 0,
 PRIMARY KEY(owner_id,digest)
);
CREATE TABLE android_auth_operations (
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 session_id TEXT NOT NULL REFERENCES identity_sessions(id),
 operation_id TEXT NOT NULL,
 purpose TEXT NOT NULL,
 scope TEXT NOT NULL,
 sealed TEXT NOT NULL,
 created_at BIGINT NOT NULL,
 PRIMARY KEY(owner_id,operation_id)
);
CREATE TABLE android_auth_invitations (
 id TEXT PRIMARY KEY,
 issuer_id TEXT NOT NULL REFERENCES identity_subjects(id),
 issuer_revision BIGINT NOT NULL CHECK(issuer_revision>0),
 email TEXT NOT NULL,
 token_digest TEXT NOT NULL UNIQUE,
 created_at BIGINT NOT NULL,
 expires_at BIGINT NOT NULL,
 accepted_at BIGINT NOT NULL DEFAULT 0,
 revoked_at BIGINT NOT NULL DEFAULT 0,
 accepted_user_id TEXT REFERENCES identity_subjects(id),
 accepted_session_id TEXT REFERENCES identity_sessions(id),
 accept_operation_id TEXT
);
CREATE INDEX android_auth_invitation_issuer ON android_auth_invitations(issuer_id,expires_at);
CREATE TABLE android_admin_operations (
 actor_id TEXT NOT NULL REFERENCES identity_subjects(id),
 scope_id TEXT NOT NULL,
 operation_id TEXT NOT NULL,
 request_sha256 TEXT NOT NULL,
 receipt TEXT NOT NULL,
 created_at BIGINT NOT NULL,
 PRIMARY KEY(actor_id,scope_id,operation_id)
);
