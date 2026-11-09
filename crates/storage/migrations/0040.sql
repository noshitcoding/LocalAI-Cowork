CREATE TABLE knowledge_revisions (
    scope TEXT NOT NULL,
    document_id TEXT NOT NULL,
    revision BIGINT NOT NULL CHECK(revision>0),
    payload_sha256 TEXT NOT NULL,
    payload TEXT NOT NULL,
    PRIMARY KEY(scope,document_id,revision)
);
