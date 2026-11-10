CREATE TABLE external_memory_providers (
 scope TEXT NOT NULL,id TEXT NOT NULL,revision BIGINT NOT NULL,payload TEXT NOT NULL,
 secret_scope TEXT,sealed TEXT,server_managed BIGINT NOT NULL,deleted BIGINT NOT NULL DEFAULT 0,
 PRIMARY KEY(scope,id)
);
CREATE TABLE external_memory_operations (
 scope TEXT NOT NULL,request_id TEXT NOT NULL,request_sha TEXT NOT NULL,receipt TEXT NOT NULL,
 PRIMARY KEY(scope,request_id)
);
CREATE TABLE external_memory_retrievals (
 scope TEXT NOT NULL,request_id TEXT NOT NULL,provider_id TEXT NOT NULL,provider_revision BIGINT NOT NULL,
 request_sha TEXT NOT NULL,receipt TEXT,
 PRIMARY KEY(scope,request_id)
);
