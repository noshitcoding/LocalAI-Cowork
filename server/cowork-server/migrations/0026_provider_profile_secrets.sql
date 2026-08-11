ALTER TABLE provider_profiles
    ADD COLUMN encrypted_data_key BYTEA,
    ADD COLUMN secret_nonce BYTEA,
    ADD COLUMN secret_wrap_nonce BYTEA;

ALTER TABLE provider_profiles
    ADD CONSTRAINT provider_profiles_secret_envelope_check CHECK (
        (encrypted_secret IS NULL
            AND encrypted_data_key IS NULL
            AND secret_nonce IS NULL
            AND secret_wrap_nonce IS NULL)
        OR
        (encrypted_secret IS NOT NULL
            AND encrypted_data_key IS NOT NULL
            AND octet_length(secret_nonce) = 12
            AND octet_length(secret_wrap_nonce) = 12)
    );
