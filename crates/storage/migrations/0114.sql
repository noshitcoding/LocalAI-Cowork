-- Original signed execution history for an existing managed input/process attempt.
-- Recording an observation creates no dispatch, worker lease or content right.
CREATE TABLE owner_managed_completion_receipts (
 attempt_id TEXT PRIMARY KEY REFERENCES owner_managed_input_transfers(attempt_id),
 run_id TEXT NOT NULL REFERENCES runs(id),
 plan_id TEXT NOT NULL REFERENCES owner_execution_plans_v2(id),
 receipt_sha256 TEXT NOT NULL,
 payload TEXT NOT NULL
);
