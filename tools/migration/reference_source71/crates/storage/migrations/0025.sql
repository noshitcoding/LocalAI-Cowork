-- Team-only encrypted integration settings. No personal/tool admission changes.
CREATE TABLE IF NOT EXISTS team_integration_schema_guard(version BIGINT PRIMARY KEY CHECK(version=1));
INSERT INTO team_integration_schema_guard(version) VALUES(1) ON CONFLICT(version) DO NOTHING;
CREATE TABLE IF NOT EXISTS team_mcp_servers(id TEXT PRIMARY KEY,team_id TEXT NOT NULL REFERENCES identity_organizations(id),name TEXT NOT NULL,revision BIGINT NOT NULL CHECK(revision>0),enabled BIGINT NOT NULL CHECK(enabled IN(0,1)),sealed_config TEXT NOT NULL,catalog TEXT,catalog_sha256 TEXT,sealed_session TEXT);
CREATE TABLE IF NOT EXISTS team_github(team_id TEXT PRIMARY KEY REFERENCES identity_organizations(id),revision BIGINT NOT NULL CHECK(revision>0),hostname TEXT NOT NULL,enabled BIGINT NOT NULL CHECK(enabled IN(0,1)),key_configured BIGINT NOT NULL CHECK(key_configured IN(0,1)),sealed_config TEXT NOT NULL);
