CREATE TABLE native_key_retirements (
    cipher_id TEXT PRIMARY KEY,
    request_key TEXT NOT NULL UNIQUE,
    request_sha256 TEXT NOT NULL,
    payload TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('pending','removed')),
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);
