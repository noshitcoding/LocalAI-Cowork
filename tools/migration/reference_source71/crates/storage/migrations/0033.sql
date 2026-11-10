CREATE TABLE IF NOT EXISTS offline_local_tool_configs(grant_id TEXT PRIMARY KEY REFERENCES offline_local_grants(id),payload TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS operator_device_configuration(id TEXT PRIMARY KEY,revision BIGINT NOT NULL,payload TEXT NOT NULL);
