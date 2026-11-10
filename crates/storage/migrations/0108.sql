CREATE TABLE owner_execution_plan_targets (
    plan_id TEXT PRIMARY KEY REFERENCES owner_execution_plans(id),
    target_id TEXT NOT NULL,
    target_revision BIGINT NOT NULL CHECK(target_revision>0),
    identity_sha256 TEXT NOT NULL,
    FOREIGN KEY(target_id,target_revision) REFERENCES execution_target_revisions(target_id,revision)
);
