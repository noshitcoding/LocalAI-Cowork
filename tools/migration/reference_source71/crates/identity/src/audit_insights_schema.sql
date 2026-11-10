CREATE TABLE IF NOT EXISTS audit_metric_heads(scope TEXT PRIMARY KEY,sequence BIGINT NOT NULL CHECK(sequence>=0));
CREATE TABLE IF NOT EXISTS audit_metric_facts(scope TEXT NOT NULL REFERENCES audit_metric_heads(scope),sequence BIGINT NOT NULL CHECK(sequence>0),id TEXT NOT NULL,payload TEXT NOT NULL,payload_sha256 TEXT NOT NULL,observed_at BIGINT NOT NULL,PRIMARY KEY(scope,sequence),UNIQUE(scope,id));
CREATE INDEX IF NOT EXISTS audit_metric_window ON audit_metric_facts(scope,observed_at,sequence);
CREATE TABLE IF NOT EXISTS audit_run_skills(run_id TEXT PRIMARY KEY,payload TEXT NOT NULL,payload_sha256 TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS audit_attempt_skills(run_id TEXT NOT NULL,attempt_id TEXT NOT NULL,payload TEXT NOT NULL,payload_sha256 TEXT NOT NULL,PRIMARY KEY(run_id,attempt_id));
CREATE TABLE IF NOT EXISTS audit_run_starts(scope TEXT NOT NULL,run_id TEXT NOT NULL,started_at BIGINT NOT NULL,PRIMARY KEY(scope,run_id));
CREATE TABLE IF NOT EXISTS audit_metric_source_cursors(scope TEXT NOT NULL,source TEXT NOT NULL,cursor TEXT NOT NULL,PRIMARY KEY(scope,source));
