CREATE TABLE automation_file_watches (
 trigger_id TEXT PRIMARY KEY REFERENCES automation_triggers(id),
 trigger_revision BIGINT NOT NULL,
 revision BIGINT NOT NULL,
 next_scan_at BIGINT NOT NULL,
 lease_id TEXT,
 lease_until BIGINT NOT NULL,
 payload TEXT NOT NULL
);
CREATE INDEX automation_file_watches_due ON automation_file_watches(next_scan_at,lease_until,trigger_id);
