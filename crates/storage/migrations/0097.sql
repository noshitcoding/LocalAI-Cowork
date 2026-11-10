CREATE TABLE actor_skill_learning (
    owner_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    skill_id TEXT NOT NULL,
    skill_revision BIGINT NOT NULL,
    run_id TEXT NOT NULL REFERENCES android_private_server_runs(id) ON DELETE CASCADE,
    recorded_at BIGINT NOT NULL,
    version_id TEXT NOT NULL,
    payload_sha256 TEXT NOT NULL,
    PRIMARY KEY(owner_id,skill_id,skill_revision,run_id)
);
CREATE INDEX actor_skill_learning_history ON actor_skill_learning(owner_id,project_id,skill_id,recorded_at);
