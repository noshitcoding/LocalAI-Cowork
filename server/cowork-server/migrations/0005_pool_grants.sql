ALTER TABLE executor_pools
    ADD COLUMN created_by_user_id UUID REFERENCES users(id);

ALTER TABLE executor_pools
    ADD CONSTRAINT executor_pools_team_fk
    FOREIGN KEY (team_id) REFERENCES teams(id);

CREATE TABLE executor_pool_project_grants (
    pool_id UUID NOT NULL REFERENCES executor_pools(id) ON DELETE CASCADE,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    granted_by_user_id UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (pool_id, project_id)
);

CREATE INDEX executor_pool_project_grants_project_idx
    ON executor_pool_project_grants (project_id, pool_id);
