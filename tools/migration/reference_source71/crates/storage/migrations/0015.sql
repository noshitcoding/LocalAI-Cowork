CREATE TABLE native_secret_state (
    id TEXT PRIMARY KEY,
    revision BIGINT NOT NULL,
    payload TEXT NOT NULL
);
CREATE TABLE native_key_operations (
    id TEXT PRIMARY KEY,
    request_key TEXT UNIQUE NOT NULL,
    request_sha256 TEXT NOT NULL,
    payload TEXT NOT NULL
);
