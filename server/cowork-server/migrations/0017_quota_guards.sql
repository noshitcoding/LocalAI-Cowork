ALTER TABLE quota_limits
    ADD CONSTRAINT quota_limits_storage_nonnegative CHECK (storage_bytes IS NULL OR storage_bytes >= 0),
    ADD CONSTRAINT quota_limits_runs_nonnegative CHECK (concurrent_runs IS NULL OR concurrent_runs >= 0),
    ADD CONSTRAINT quota_limits_tokens_nonnegative CHECK (monthly_tokens IS NULL OR monthly_tokens >= 0),
    ADD CONSTRAINT quota_limits_cost_nonnegative CHECK (monthly_cost_micros IS NULL OR monthly_cost_micros >= 0);

ALTER TABLE quota_usage
    ADD CONSTRAINT quota_usage_nonnegative CHECK (
        storage_bytes >= 0 AND running_runs >= 0 AND tokens >= 0 AND cost_micros >= 0
    );

CREATE INDEX quota_usage_period_idx ON quota_usage (period_start, scope_type, scope_id);
