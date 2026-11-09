CREATE TABLE process_attempts (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES runs(id),
    tool_call_id TEXT NOT NULL,
    revision BIGINT NOT NULL,
    payload TEXT NOT NULL,
    UNIQUE(run_id,tool_call_id)
);
