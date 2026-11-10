CREATE TABLE server_key_guard(id BIGINT PRIMARY KEY CHECK(id=1));
INSERT INTO server_key_guard(id) VALUES(1);
CREATE TABLE server_key_registry(id BIGINT PRIMARY KEY CHECK(id=1),revision BIGINT NOT NULL CHECK(revision>0),payload TEXT NOT NULL);
CREATE TABLE server_key_operations(request_id TEXT PRIMARY KEY,input_sha256 TEXT NOT NULL,payload TEXT NOT NULL,state TEXT NOT NULL CHECK(state IN('prepared','confirmed','complete','cancelled')),receipt TEXT,completed_at BIGINT NOT NULL);
