CREATE TABLE mcp_servers (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id),
    revision BIGINT NOT NULL,
    payload TEXT NOT NULL
);
CREATE TABLE mcp_server_requests (
    project_id TEXT NOT NULL REFERENCES projects(id),
    request_key TEXT NOT NULL,
    request_sha256 TEXT NOT NULL,
    payload TEXT NOT NULL,
    PRIMARY KEY(project_id,request_key)
);
CREATE TABLE mcp_attempts (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES runs(id),
    tool_call_id TEXT NOT NULL,
    revision BIGINT NOT NULL,
    payload TEXT NOT NULL,
    UNIQUE(run_id,tool_call_id)
);
