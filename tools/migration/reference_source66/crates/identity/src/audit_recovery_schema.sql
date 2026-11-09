CREATE TABLE IF NOT EXISTS audit_v1_receipts(scope TEXT NOT NULL,request_id TEXT NOT NULL,request_sha256 TEXT NOT NULL,payload TEXT NOT NULL,payload_sha256 TEXT NOT NULL,PRIMARY KEY(scope,request_id));
CREATE TABLE IF NOT EXISTS audit_retired_chains(scope TEXT NOT NULL,request_id TEXT NOT NULL,request_sha256 TEXT NOT NULL,original_base64 TEXT NOT NULL,original_sha256 TEXT NOT NULL,receipt TEXT NOT NULL,receipt_sha256 TEXT NOT NULL,PRIMARY KEY(scope,request_id));
CREATE TABLE IF NOT EXISTS audit_retired_sources(scope TEXT NOT NULL,event_id TEXT NOT NULL,PRIMARY KEY(scope,event_id));
