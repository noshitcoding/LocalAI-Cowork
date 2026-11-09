CREATE TABLE execution_target_registry_guard (
    id BIGINT PRIMARY KEY CHECK(id=1)
);
INSERT INTO execution_target_registry_guard(id) VALUES(1);
CREATE TABLE execution_targets (
    target_id TEXT PRIMARY KEY,
    revision BIGINT NOT NULL CHECK(revision>0),
    identity_sha256 TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('registered','active','revoked')),
    payload TEXT NOT NULL
);
CREATE TABLE execution_target_revisions (
    target_id TEXT NOT NULL REFERENCES execution_targets(target_id),
    revision BIGINT NOT NULL CHECK(revision>0),
    payload TEXT NOT NULL,
    PRIMARY KEY(target_id,revision)
);
