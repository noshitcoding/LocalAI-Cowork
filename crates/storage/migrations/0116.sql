-- Immutable delivery intents and original signed retirement history.
-- This journal grants neither content reads nor a process/worker lease.
CREATE TABLE owner_managed_result_deliveries (
 id TEXT PRIMARY KEY,
 result_version_id TEXT NOT NULL REFERENCES owner_managed_result_versions(id),
 run_id TEXT NOT NULL REFERENCES runs(id),
 attempt_id TEXT NOT NULL REFERENCES owner_managed_completion_receipts(attempt_id),
 plan_id TEXT NOT NULL REFERENCES owner_execution_plans_v2(id),
 ordinal BIGINT NOT NULL CHECK(ordinal > 0),
 state TEXT NOT NULL CHECK(state IN ('prepared','committed','expired','accepted','retired','unconfirmed')),
 revision BIGINT NOT NULL CHECK(revision > 0),
 payload_sha256 TEXT NOT NULL,
 payload TEXT NOT NULL
);
CREATE UNIQUE INDEX owner_managed_result_delivery_current
 ON owner_managed_result_deliveries(result_version_id) WHERE state <> 'expired';
CREATE UNIQUE INDEX owner_managed_result_delivery_ordinal
 ON owner_managed_result_deliveries(result_version_id,ordinal);
CREATE INDEX owner_managed_result_deliveries_run
 ON owner_managed_result_deliveries(run_id,id);
