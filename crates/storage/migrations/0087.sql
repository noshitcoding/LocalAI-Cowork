CREATE TABLE owner_connectors(id TEXT PRIMARY KEY,project_id TEXT NOT NULL REFERENCES projects(id),revision BIGINT NOT NULL,payload TEXT NOT NULL);
CREATE TABLE owner_connector_versions(server_id TEXT PRIMARY KEY,connector_id TEXT NOT NULL REFERENCES owner_connectors(id),project_id TEXT NOT NULL REFERENCES projects(id),is_current BIGINT NOT NULL);
CREATE TABLE owner_connector_requests(project_id TEXT NOT NULL,request_key TEXT NOT NULL,request_sha256 TEXT NOT NULL,payload TEXT NOT NULL,PRIMARY KEY(project_id,request_key));
