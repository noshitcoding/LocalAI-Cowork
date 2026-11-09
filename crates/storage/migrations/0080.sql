CREATE TABLE personal_provider_credential_versions (
    provider_id TEXT NOT NULL REFERENCES providers(id),
    revision BIGINT NOT NULL,
    profile TEXT NOT NULL,
    payload TEXT,
    PRIMARY KEY(provider_id, revision)
);
