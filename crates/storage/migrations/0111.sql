-- Actual local FileBroker target, independent of ProcessRunner registrations.
-- Native anchor/object provenance is a private digest, never a public ID.
CREATE TABLE broker_controller (
    singleton BIGINT PRIMARY KEY CHECK(singleton=1),
    id TEXT NOT NULL UNIQUE,
    generation BIGINT NOT NULL CHECK(generation>0),
    origin_sha256 TEXT NOT NULL
);
