CREATE TABLE IF NOT EXISTS native_project_retirements(project_id TEXT PRIMARY KEY REFERENCES projects(id),retired_at BIGINT NOT NULL);
