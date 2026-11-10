CREATE TABLE mcp_connections (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES runs(id),
    server_id TEXT NOT NULL REFERENCES mcp_servers(id),
    revision BIGINT NOT NULL,
    payload TEXT NOT NULL,
    UNIQUE(run_id,server_id)
);
