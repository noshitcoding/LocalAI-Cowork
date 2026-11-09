-- Private input custody for an existing confirmed run and process attempt.
-- These records never create another run or reconstruct dispatch authority.
CREATE TABLE owner_managed_input_transfers (
 attempt_id TEXT PRIMARY KEY REFERENCES process_attempts(id),
 run_id TEXT NOT NULL REFERENCES runs(id),
 plan_id TEXT NOT NULL REFERENCES owner_execution_plans_v2(id),
 grant_sha256 TEXT NOT NULL,
 custody_sha256 TEXT NOT NULL,
 payload TEXT NOT NULL
);
