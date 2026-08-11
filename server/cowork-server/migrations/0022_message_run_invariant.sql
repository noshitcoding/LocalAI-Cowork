-- A user chat message and its durable run are a one-to-one pair. Assistant
-- output is persisted once when that run completes. Tool/system messages may
-- still be emitted more than once for a run in future protocol revisions.
CREATE UNIQUE INDEX messages_user_assistant_run_unique
    ON messages (run_id, role)
    WHERE run_id IS NOT NULL
      AND role IN ('user', 'assistant')
      AND deleted_at IS NULL;
