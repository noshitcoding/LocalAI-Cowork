-- Explicit total physical placement. Existing instances receive no qualification.
CREATE TABLE management_disk_plans (
 id UUID PRIMARY KEY, organisation_id UUID NOT NULL REFERENCES management_organisations(id),
 instance_id UUID NOT NULL REFERENCES management_instances(id), host_id UUID NOT NULL REFERENCES management_executor_hosts(id),
 actor_user_id UUID NOT NULL REFERENCES users(id), request_id UUID NOT NULL,
 request_sha256 TEXT NOT NULL CHECK(request_sha256 ~ '^[0-9a-f]{64}$'),
 expected_instance_revision BIGINT NOT NULL CHECK(expected_instance_revision>0),
 expected_profile_revision BIGINT NOT NULL CHECK(expected_profile_revision>=0),
 source_authority JSONB NOT NULL, specification JSONB NOT NULL,
 specification_sha256 TEXT NOT NULL CHECK(specification_sha256 ~ '^[0-9a-f]{64}$'),
 executor_host_binding JSONB NOT NULL, execution_epoch UUID NOT NULL, challenge UUID NOT NULL UNIQUE,
 purpose TEXT NOT NULL CHECK(purpose IN ('activate','disable','release')),
 state TEXT NOT NULL DEFAULT 'prepared' CHECK(state IN ('prepared','observed','committed')),
 observation JSONB, observation_received_at TIMESTAMPTZ, receipt JSONB, created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
 UNIQUE(organisation_id,actor_user_id,request_id),
 CHECK((observation IS NULL)=(state='prepared')), CHECK((receipt IS NOT NULL)=(state='committed'))
);
CREATE TABLE management_disk_profiles (
 instance_id UUID PRIMARY KEY REFERENCES management_instances(id),
 organisation_id UUID NOT NULL REFERENCES management_organisations(id), host_id UUID NOT NULL REFERENCES management_executor_hosts(id),
 revision BIGINT NOT NULL CHECK(revision>0), plan_id UUID NOT NULL REFERENCES management_disk_plans(id),
 execution_epoch UUID NOT NULL, specification JSONB NOT NULL,
 specification_sha256 TEXT NOT NULL CHECK(specification_sha256 ~ '^[0-9a-f]{64}$'),
 executor_host_binding JSONB NOT NULL, receipt JSONB NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('active','disabled','released')),
 reserved_bytes BIGINT NOT NULL CHECK(reserved_bytes>0), observed_at TIMESTAMPTZ NOT NULL,
 observed_expires_at TIMESTAMPTZ NOT NULL, observation JSONB NOT NULL,
 observation_challenge UUID, challenge_expires_at TIMESTAMPTZ,
 CHECK((observation_challenge IS NULL)=(challenge_expires_at IS NULL)),
 CHECK(observed_expires_at>observed_at)
);
CREATE TABLE management_disk_reservations (
 placement_id UUID PRIMARY KEY, instance_id UUID NOT NULL REFERENCES management_instances(id),
 host_id UUID NOT NULL REFERENCES management_executor_hosts(id), kind TEXT NOT NULL CHECK(kind IN ('guest','controller','environment','snapshot')),
 filesystem_uuid UUID NOT NULL UNIQUE, machine_id UUID NOT NULL, size_bytes BIGINT NOT NULL CHECK(size_bytes>=1073741824),
 original_plan_id UUID NOT NULL REFERENCES management_disk_plans(id), allocation_sha256 TEXT CHECK(allocation_sha256 ~ '^[0-9a-f]{64}$'),
 allocation JSONB, release_receipt JSONB, state TEXT NOT NULL DEFAULT 'reserved' CHECK(state IN ('reserved','allocated','released')),
 CHECK((release_receipt IS NOT NULL)=(state='released'))
);
CREATE INDEX management_disk_reservations_host ON management_disk_reservations(host_id,state,instance_id);
CREATE TABLE management_disk_io_receipts (
 instance_id UUID NOT NULL REFERENCES management_instances(id), request_id UUID NOT NULL, purpose TEXT NOT NULL,
 request_sha256 TEXT NOT NULL CHECK(request_sha256 ~ '^[0-9a-f]{64}$'), profile_revision BIGINT NOT NULL CHECK(profile_revision>0),
 execution_epoch UUID NOT NULL, receipt JSONB NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
 PRIMARY KEY(instance_id,request_id)
);
