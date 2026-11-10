CREATE TABLE workflow_attention (
 run_id TEXT PRIMARY KEY REFERENCES workflow_runs(run_id),
 run_revision BIGINT NOT NULL CHECK(run_revision >= 1),
 workflow_revision BIGINT NOT NULL CHECK(workflow_revision >= 0),
 actionable_count BIGINT NOT NULL CHECK(actionable_count >= 0),
 valid_until BIGINT,
 CHECK((actionable_count=0 AND valid_until IS NULL) OR (actionable_count>0 AND valid_until IS NOT NULL))
);
CREATE INDEX workflow_attention_actionable ON workflow_attention(actionable_count,valid_until);
