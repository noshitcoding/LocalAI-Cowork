CREATE TABLE team_sync_clock(id BIGINT PRIMARY KEY CHECK(id=1),sequence BIGINT NOT NULL);
INSERT INTO team_sync_clock(id,sequence) VALUES(1,0);
CREATE TABLE team_sync_current(team_id TEXT NOT NULL,project_id TEXT NOT NULL,kind TEXT NOT NULL,entity_id TEXT NOT NULL,digest TEXT NOT NULL,cursor BIGINT NOT NULL,payload TEXT NOT NULL,PRIMARY KEY(team_id,project_id,kind,entity_id));
CREATE TABLE team_sync_changes(cursor BIGINT PRIMARY KEY,team_id TEXT NOT NULL,project_id TEXT NOT NULL,kind TEXT NOT NULL,entity_id TEXT NOT NULL,payload TEXT NOT NULL,tombstone BIGINT NOT NULL,updated_at BIGINT NOT NULL);
CREATE INDEX team_sync_scope ON team_sync_changes(team_id,project_id,cursor);
CREATE TABLE team_sync_operations(team_id TEXT NOT NULL,project_id TEXT NOT NULL,identity_id TEXT NOT NULL,idempotency_key TEXT NOT NULL,request_sha256 TEXT NOT NULL,receipt TEXT NOT NULL,PRIMARY KEY(team_id,project_id,identity_id,idempotency_key));
CREATE TABLE mcp_resolution_reviews(run_id TEXT NOT NULL,tool_call_id TEXT NOT NULL,evidence_sha256 TEXT NOT NULL,payload TEXT NOT NULL,created_at BIGINT NOT NULL,PRIMARY KEY(run_id,tool_call_id));
