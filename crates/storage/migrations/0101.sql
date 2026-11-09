CREATE TABLE automation_triggers (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES projects(id),
 task_id TEXT NOT NULL REFERENCES native_workspace_tasks(id),
 revision BIGINT NOT NULL,
 enabled BIGINT NOT NULL,
 payload TEXT NOT NULL,
 task_snapshot TEXT NOT NULL
);
CREATE INDEX automation_triggers_project ON automation_triggers(project_id,id);
CREATE TABLE automation_events (
 sequence BIGINT NOT NULL,
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES projects(id),
 trigger_id TEXT NOT NULL REFERENCES automation_triggers(id),
 task_id TEXT NOT NULL REFERENCES native_workspace_tasks(id),
 source TEXT NOT NULL,
 event_key TEXT NOT NULL,
 request_hash TEXT NOT NULL,
 state TEXT NOT NULL,
 received_at BIGINT NOT NULL,
 run_key TEXT NOT NULL UNIQUE,
 run_id TEXT REFERENCES runs(id),
 run_request TEXT,
 run_request_hash TEXT,
 payload TEXT NOT NULL,
 ingress_receipt TEXT NOT NULL,
 UNIQUE(trigger_id,source,event_key),
 UNIQUE(project_id,sequence)
);
CREATE INDEX automation_events_queue ON automation_events(state,sequence);
CREATE INDEX automation_events_history ON automation_events(project_id,trigger_id,sequence);
CREATE TABLE automation_webhook_grants (
 trigger_id TEXT PRIMARY KEY REFERENCES automation_triggers(id),
 token_hash TEXT NOT NULL UNIQUE,
 enabled BIGINT NOT NULL,
 expires_at BIGINT NOT NULL,
 payload TEXT NOT NULL
);
