-- Private receiving/result versions of existing original managed attempts.
-- A row or signed completion creates neither read nor execution authority.
CREATE TABLE owner_managed_result_versions (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES projects(id),
 order_id TEXT NOT NULL REFERENCES orders(id),
 run_id TEXT NOT NULL REFERENCES runs(id),
 attempt_id TEXT NOT NULL UNIQUE REFERENCES owner_managed_completion_receipts(attempt_id),
 plan_id TEXT NOT NULL REFERENCES owner_execution_plans_v2(id),
 state TEXT NOT NULL CHECK(state IN ('receiving','ready','retired')),
 revision BIGINT NOT NULL CHECK(revision > 0),
 payload_sha256 TEXT NOT NULL,
 payload TEXT NOT NULL
);
CREATE INDEX owner_managed_result_versions_run ON owner_managed_result_versions(run_id,id);
