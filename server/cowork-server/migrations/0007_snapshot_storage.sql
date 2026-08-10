ALTER TABLE snapshot_chunks
    ADD COLUMN wrap_nonce BYTEA,
    ADD COLUMN status TEXT NOT NULL DEFAULT 'ready'
        CHECK (status IN ('uploading', 'ready', 'deleting'));

-- Existing development rows predate wrapped-key nonces. Production migrations
-- only create such rows through the API below, which always supplies one.

ALTER TABLE snapshot_manifests
    ADD COLUMN warnings JSONB NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN committed_at TIMESTAMPTZ;

ALTER TABLE runs ADD COLUMN snapshot_id UUID REFERENCES snapshot_manifests(id);
UPDATE runs
SET snapshot_id = (spec ->> 'snapshot_id')::uuid
WHERE spec ->> 'snapshot_id' IS NOT NULL;
CREATE INDEX runs_waiting_snapshot_idx ON runs (snapshot_id)
    WHERE state = 'waiting_for_snapshot';
