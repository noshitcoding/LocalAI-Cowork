CREATE TABLE android_device_original_receipts (
    owner_id TEXT NOT NULL REFERENCES identity_subjects(id),
    operation_key TEXT NOT NULL CHECK(length(operation_key) BETWEEN 1 AND 200),
    project_id TEXT NOT NULL,
    run_id TEXT NOT NULL UNIQUE REFERENCES android_scoped_runs(id),
    request_sha256 TEXT NOT NULL CHECK(length(request_sha256)=64),
    receipt TEXT NOT NULL CHECK(length(receipt) BETWEEN 1 AND 4194304),
    receipt_sha256 TEXT NOT NULL CHECK(length(receipt_sha256)=64),
    created_at BIGINT NOT NULL,
    PRIMARY KEY(owner_id,operation_key)
);
CREATE INDEX android_device_original_receipts_scope ON android_device_original_receipts(owner_id,project_id,created_at);
