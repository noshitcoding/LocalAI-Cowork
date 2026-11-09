-- One-shot account RPC intents. Unknown login/logout effects are never replayed.
CREATE TABLE codex_account_call_guard (id BIGINT PRIMARY KEY CHECK(id=1));
INSERT INTO codex_account_call_guard(id) VALUES(1);
CREATE TABLE codex_account_calls (
 scope TEXT NOT NULL, operation_key TEXT NOT NULL, account_id TEXT NOT NULL,
 request_sha256 TEXT NOT NULL, response TEXT, created_at BIGINT NOT NULL,
 PRIMARY KEY(scope,operation_key)
);
