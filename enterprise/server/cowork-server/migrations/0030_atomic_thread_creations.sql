ALTER TABLE schedules
    ADD COLUMN idempotency_key TEXT;

CREATE UNIQUE INDEX schedules_created_by_idempotency_key_unique
    ON schedules (created_by, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
