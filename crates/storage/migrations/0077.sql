CREATE TABLE provider_generation_settings (
    provider_id TEXT PRIMARY KEY REFERENCES providers(id),
    payload TEXT NOT NULL
);
CREATE TABLE provider_setting_requests (
    request_key TEXT PRIMARY KEY,
    request_sha256 TEXT NOT NULL,
    payload TEXT NOT NULL
);
CREATE TABLE provider_retirements (
    provider_id TEXT PRIMARY KEY,
    retired_at BIGINT NOT NULL
);
