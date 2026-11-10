CREATE TABLE IF NOT EXISTS personal_provider_guard(id BIGINT PRIMARY KEY CHECK(id=1));
INSERT INTO personal_provider_guard(id) VALUES(1) ON CONFLICT(id) DO NOTHING;
CREATE TABLE IF NOT EXISTS personal_provider_credentials(provider_id TEXT PRIMARY KEY REFERENCES providers(id), payload TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS personal_provider_requests(request_key TEXT PRIMARY KEY, request_sha256 TEXT NOT NULL, provider_id TEXT NOT NULL UNIQUE REFERENCES providers(id), created_at BIGINT NOT NULL);
