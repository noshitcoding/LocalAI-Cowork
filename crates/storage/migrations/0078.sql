CREATE TABLE IF NOT EXISTS native_project_settings(project_id TEXT PRIMARY KEY REFERENCES projects(id), payload TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS native_chat_management(order_id TEXT PRIMARY KEY REFERENCES orders(id), archived BIGINT NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS workspace_management_operations(scope TEXT NOT NULL,request_key TEXT NOT NULL,request_sha256 TEXT NOT NULL,receipt TEXT NOT NULL,PRIMARY KEY(scope,request_key));
CREATE TABLE IF NOT EXISTS native_workspace_tasks(id TEXT PRIMARY KEY,project_id TEXT NOT NULL REFERENCES projects(id),order_id TEXT NOT NULL REFERENCES orders(id),revision BIGINT NOT NULL,archived BIGINT NOT NULL DEFAULT 0,payload TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS native_workspace_task_runs(task_id TEXT NOT NULL REFERENCES native_workspace_tasks(id),request_key TEXT NOT NULL,request_sha256 TEXT NOT NULL,task_snapshot TEXT NOT NULL,run_id TEXT,created_at BIGINT NOT NULL,PRIMARY KEY(task_id,request_key));
CREATE INDEX IF NOT EXISTS native_workspace_tasks_project ON native_workspace_tasks(project_id,archived);
