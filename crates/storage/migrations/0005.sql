CREATE TABLE file_operations (run_id TEXT NOT NULL REFERENCES runs(id), tool_call_id TEXT NOT NULL, payload TEXT NOT NULL, PRIMARY KEY (run_id,tool_call_id));
