CREATE TABLE codex_model_profiles (
 scope TEXT NOT NULL, profile_id TEXT NOT NULL, account_id TEXT NOT NULL,
 payload TEXT NOT NULL, PRIMARY KEY(scope,profile_id)
);
CREATE TABLE codex_model_profile_operations (
 scope TEXT NOT NULL, operation_key TEXT NOT NULL, request_sha TEXT NOT NULL,
 receipt TEXT NOT NULL, PRIMARY KEY(scope,operation_key)
);
