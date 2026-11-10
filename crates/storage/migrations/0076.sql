-- Owner-private reusable content and immutable versions. No template grants rights.
CREATE TABLE library_config_guard (id BIGINT PRIMARY KEY CHECK(id=1));
INSERT INTO library_config_guard(id) VALUES(1);
CREATE TABLE library_config_entries (
 scope TEXT NOT NULL, kind TEXT NOT NULL, id TEXT NOT NULL,
 revision BIGINT NOT NULL CHECK(revision>0), payload TEXT NOT NULL,
 deleted BIGINT NOT NULL CHECK(deleted IN(0,1)),
 PRIMARY KEY(scope,kind,id)
);
CREATE TABLE library_config_versions (
 scope TEXT NOT NULL, kind TEXT NOT NULL, id TEXT NOT NULL,
 revision BIGINT NOT NULL CHECK(revision>0), payload TEXT NOT NULL,
 PRIMARY KEY(scope,kind,id,revision)
);
CREATE TABLE library_config_operations (
 scope TEXT NOT NULL, operation_key TEXT NOT NULL, request_sha256 TEXT NOT NULL,
 receipt TEXT NOT NULL, created_at BIGINT NOT NULL,
 PRIMARY KEY(scope,operation_key)
);
CREATE TABLE library_config_preferences (
 scope TEXT PRIMARY KEY, revision BIGINT NOT NULL CHECK(revision>0), payload TEXT NOT NULL
);
