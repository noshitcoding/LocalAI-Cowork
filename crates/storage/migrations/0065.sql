CREATE TABLE android_private_schedules (
 id TEXT PRIMARY KEY, owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
 project_id TEXT NOT NULL REFERENCES android_personal_projects(id),
 thread_id TEXT NOT NULL REFERENCES android_personal_threads(id),
 revision BIGINT NOT NULL CHECK(revision>0), execution_revision BIGINT NOT NULL CHECK(execution_revision>0),
 grant_id TEXT, enabled BIGINT NOT NULL CHECK(enabled IN(0,1)), deleted BIGINT NOT NULL CHECK(deleted IN(0,1)),
 next_run BIGINT, active_occurrence_id TEXT, payload TEXT NOT NULL
);
CREATE INDEX android_private_schedule_due ON android_private_schedules(enabled,deleted,next_run);
CREATE TABLE android_private_schedule_grants (
 id TEXT PRIMARY KEY, schedule_id TEXT NOT NULL REFERENCES android_private_schedules(id),
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id), execution_revision BIGINT NOT NULL,
 active BIGINT NOT NULL CHECK(active IN(0,1)), source TEXT NOT NULL, source_sha256 TEXT NOT NULL, created_at BIGINT NOT NULL
);
CREATE TABLE android_private_schedule_occurrences (
 id TEXT PRIMARY KEY, schedule_id TEXT NOT NULL REFERENCES android_private_schedules(id),
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id), grant_id TEXT NOT NULL REFERENCES android_private_schedule_grants(id),
 execution_revision BIGINT NOT NULL, sequence BIGINT NOT NULL UNIQUE,
 run_id TEXT NOT NULL UNIQUE, creation_key TEXT NOT NULL UNIQUE, family TEXT NOT NULL CHECK(family IN('server_linux','personal_device')),
 request_sha256 TEXT NOT NULL, spec_sha256 TEXT NOT NULL, request TEXT NOT NULL,
 status TEXT NOT NULL CHECK(status IN('queued','running','completed','failed','canceled','unknown','blocked')),
 due_at BIGINT NOT NULL, triggered_at BIGINT NOT NULL, completed_at BIGINT, error TEXT,
 missed_occurrences BIGINT NOT NULL, catch_up_truncated BIGINT NOT NULL,
 UNIQUE(schedule_id,execution_revision,due_at)
);
CREATE INDEX android_private_schedule_history ON android_private_schedule_occurrences(schedule_id,sequence);
CREATE TABLE android_private_schedule_operations (
 owner_id TEXT NOT NULL REFERENCES identity_subjects(id), operation_key TEXT NOT NULL,
 purpose TEXT NOT NULL, request_sha256 TEXT NOT NULL, receipt TEXT NOT NULL, created_at BIGINT NOT NULL,
 PRIMARY KEY(owner_id,operation_key)
);
