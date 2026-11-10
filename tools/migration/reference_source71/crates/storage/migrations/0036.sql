CREATE TABLE native_mcp_factories(server_id TEXT PRIMARY KEY,project_id TEXT NOT NULL REFERENCES projects(id),revision BIGINT NOT NULL,payload TEXT NOT NULL);
