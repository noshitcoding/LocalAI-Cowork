ALTER TABLE run_artifacts
    ADD COLUMN key_scope_type TEXT CHECK (key_scope_type IN ('user', 'team')),
    ADD COLUMN key_scope_id UUID,
    ADD COLUMN encrypted_data_key BYTEA,
    ADD COLUMN nonce BYTEA,
    ADD COLUMN wrap_nonce BYTEA;
