CREATE TABLE workflow_execution_clocks (
 run_id TEXT PRIMARY KEY REFERENCES runs(id), revision BIGINT NOT NULL,
 last_run_revision BIGINT NOT NULL, highwater_ms BIGINT NOT NULL, payload TEXT NOT NULL
);
