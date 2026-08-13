ALTER TABLE snapshot_manifests
    ADD COLUMN source_run_id UUID REFERENCES runs(id) ON DELETE SET NULL;

CREATE UNIQUE INDEX snapshot_manifests_source_run_unique
    ON snapshot_manifests (source_run_id)
    WHERE source_run_id IS NOT NULL;

ALTER TABLE run_artifacts
    ADD COLUMN source_event_id UUID;

CREATE UNIQUE INDEX run_artifacts_source_event_unique
    ON run_artifacts (run_id, source_event_id)
    WHERE source_event_id IS NOT NULL;

ALTER TABLE approval_requests
    ADD COLUMN source_request_id UUID;
CREATE UNIQUE INDEX approval_requests_source_request_unique
    ON approval_requests (run_id, source_request_id)
    WHERE source_request_id IS NOT NULL;

ALTER TABLE run_input_requests
    ADD COLUMN source_request_id UUID;
CREATE UNIQUE INDEX run_input_requests_source_request_unique
    ON run_input_requests (run_id, source_request_id)
    WHERE source_request_id IS NOT NULL;
