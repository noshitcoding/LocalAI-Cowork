CREATE TABLE IF NOT EXISTS audit_heads(scope TEXT PRIMARY KEY,chain_id TEXT NOT NULL,sequence BIGINT NOT NULL CHECK(sequence>=0),head_sha256 TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS audit_records(scope TEXT NOT NULL REFERENCES audit_heads(scope),chain_id TEXT NOT NULL,sequence BIGINT NOT NULL CHECK(sequence>0),event_id TEXT NOT NULL,payload TEXT NOT NULL,sha256 TEXT NOT NULL,PRIMARY KEY(scope,sequence),UNIQUE(scope,event_id));
CREATE TABLE IF NOT EXISTS audit_archive_receipts(scope TEXT NOT NULL,request_id TEXT NOT NULL,request_sha256 TEXT NOT NULL,payload TEXT NOT NULL,PRIMARY KEY(scope,request_id));
CREATE TABLE IF NOT EXISTS audit_preserved_history(scope TEXT NOT NULL,request_id TEXT NOT NULL,request_sha256 TEXT NOT NULL,content_sha256 TEXT NOT NULL,original_base64 TEXT NOT NULL,receipt TEXT NOT NULL,PRIMARY KEY(scope,request_id));
CREATE TABLE IF NOT EXISTS audit_legacy_boundary(id BIGINT PRIMARY KEY CHECK(id=1),identity_sequence BIGINT NOT NULL);
INSERT INTO audit_legacy_boundary(id,identity_sequence) SELECT 1,COALESCE(MAX(sequence),0) FROM identity_audit WHERE 1=1 ON CONFLICT(id) DO NOTHING;
CREATE TABLE IF NOT EXISTS audit_source_cursors(scope TEXT PRIMARY KEY,identity_sequence BIGINT NOT NULL DEFAULT 0);
