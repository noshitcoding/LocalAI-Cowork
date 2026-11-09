CREATE TABLE workflow_external_evidence (
    run_id TEXT NOT NULL,
    tool_call_id TEXT NOT NULL,
    preparation_sha256 TEXT NOT NULL,
    result_sha256 TEXT,
    payload TEXT NOT NULL,
    PRIMARY KEY (run_id, tool_call_id)
);
