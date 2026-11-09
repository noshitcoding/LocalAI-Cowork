-- Executor credentials are deliberately separated from synchronized MCP
-- metadata. This table holds only the Linux control-plane binding; clients
-- receive the allowlisted metadata columns and never the encrypted payload.
CREATE TABLE server_mcp_bindings (
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    mcp_entity_id UUID NOT NULL,
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision >= 1),
    etag TEXT NOT NULL,
    owner_user_id UUID NOT NULL REFERENCES users(id),
    team_id UUID REFERENCES teams(id),
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 256),
    transport TEXT NOT NULL DEFAULT 'stdio' CHECK (transport = 'stdio'),
    executable_hint TEXT NOT NULL,
    argument_count INTEGER NOT NULL CHECK (argument_count BETWEEN 0 AND 256),
    environment_keys JSONB NOT NULL DEFAULT '[]'::jsonb,
    encrypted_binding BYTEA NOT NULL,
    encrypted_data_key BYTEA NOT NULL,
    binding_nonce BYTEA NOT NULL CHECK (octet_length(binding_nonce) = 12),
    binding_wrap_nonce BYTEA NOT NULL CHECK (octet_length(binding_wrap_nonce) = 12),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, mcp_entity_id),
    UNIQUE (project_id, name),
    CHECK (jsonb_typeof(environment_keys) = 'array')
);

CREATE INDEX server_mcp_bindings_project_updated_idx
    ON server_mcp_bindings (project_id, updated_at DESC, mcp_entity_id);
