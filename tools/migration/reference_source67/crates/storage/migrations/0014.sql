CREATE TABLE mcp_credentials (
    project_id TEXT NOT NULL REFERENCES projects(id),
    reference TEXT NOT NULL,
    revision BIGINT NOT NULL,
    payload TEXT NOT NULL,
    PRIMARY KEY(project_id, reference)
);
