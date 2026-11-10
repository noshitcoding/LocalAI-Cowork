-- Derived scheduling index only. Authority remains the validated workflow/run.
CREATE TABLE workflow_timer_wakes (
 run_id TEXT PRIMARY KEY REFERENCES runs(id),
 progress_revision BIGINT NOT NULL CHECK(progress_revision >= 0),
 progress_sha256 TEXT NOT NULL,
 wake_at BIGINT NOT NULL CHECK(wake_at >= 0)
);
CREATE INDEX workflow_timer_wakes_due ON workflow_timer_wakes(wake_at, run_id);
