CREATE TABLE workspaces (order_id TEXT PRIMARY KEY REFERENCES orders(id), payload TEXT NOT NULL);
CREATE TABLE workspace_snapshots (run_id TEXT PRIMARY KEY REFERENCES runs(id), order_id TEXT NOT NULL REFERENCES orders(id), payload TEXT NOT NULL);
CREATE INDEX workspace_snapshots_order ON workspace_snapshots(order_id);
