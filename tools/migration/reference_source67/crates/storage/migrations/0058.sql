CREATE TABLE IF NOT EXISTS audit_origin_guard(id BIGINT PRIMARY KEY CHECK(id=1));
INSERT INTO audit_origin_guard(id) VALUES(1) ON CONFLICT(id) DO NOTHING;
CREATE TABLE IF NOT EXISTS audit_origin_epoch(id BIGINT PRIMARY KEY CHECK(id=1),instance_id TEXT NOT NULL,key_id TEXT NOT NULL,epoch BIGINT NOT NULL CHECK(epoch>=0),public_key_sha256 TEXT NOT NULL,pending_request_id TEXT);
CREATE TABLE server_audit_rotation_plans(request_id TEXT PRIMARY KEY REFERENCES server_key_operations(request_id),payload TEXT NOT NULL,payload_sha256 TEXT NOT NULL);
