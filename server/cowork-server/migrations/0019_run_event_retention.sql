ALTER TABLE runs
    ADD COLUMN next_event_sequence BIGINT NOT NULL DEFAULT 1
        CHECK (next_event_sequence > 0);

UPDATE runs run
SET next_event_sequence = sequence.next_sequence
FROM (
    SELECT run.id, COALESCE(MAX(event.sequence), 0) + 1 AS next_sequence
    FROM runs run
    LEFT JOIN run_events event ON event.run_id = run.id
    GROUP BY run.id
) sequence
WHERE sequence.id = run.id;

CREATE INDEX run_events_retention_idx ON run_events (created_at, run_id, sequence);
