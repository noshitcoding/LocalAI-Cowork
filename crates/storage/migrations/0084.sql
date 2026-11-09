CREATE TABLE native_user_memories (
 document_id TEXT PRIMARY KEY REFERENCES productivity_objects(id) ON DELETE CASCADE,
 project_id TEXT NOT NULL REFERENCES projects(id),
 revision BIGINT NOT NULL CHECK(revision>0)
);
CREATE INDEX native_user_memories_project ON native_user_memories(project_id,document_id);
