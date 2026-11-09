CREATE TABLE file_changes (run_id TEXT NOT NULL REFERENCES runs(id), tool_call_id TEXT NOT NULL, payload TEXT NOT NULL, PRIMARY KEY(run_id,tool_call_id));
CREATE TABLE folder_owners (resource_id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id));
