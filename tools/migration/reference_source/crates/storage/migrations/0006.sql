ALTER TABLE runs ADD COLUMN execution_kind TEXT NOT NULL DEFAULT 'agent' CHECK (execution_kind IN ('agent','file_action'));
CREATE INDEX runs_execution_kind_state ON runs(execution_kind,state);
