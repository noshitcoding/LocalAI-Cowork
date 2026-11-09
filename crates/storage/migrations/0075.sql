-- Independent lost-original-host retirement. No old record or allocation is adopted.
CREATE TABLE team_host_trust_profiles (
 id TEXT PRIMARY KEY, team_id TEXT NOT NULL REFERENCES identity_organizations(id),
 host_id TEXT NOT NULL, engine_id TEXT NOT NULL, node_name TEXT, node_uid TEXT,
 profile_sha256 TEXT NOT NULL, profile TEXT NOT NULL, public_key_der TEXT NOT NULL, public_key_sha256 TEXT NOT NULL,
 vm_id TEXT NOT NULL, execution_epoch TEXT NOT NULL, revision BIGINT NOT NULL CHECK(revision>0),
 active BIGINT NOT NULL CHECK(active IN(0,1)), retired_at BIGINT, inspected_at BIGINT NOT NULL,
 activated_at BIGINT, creator_id TEXT REFERENCES identity_subjects(id), UNIQUE(team_id,vm_id,execution_epoch), UNIQUE(vm_id),
 CHECK((node_name IS NULL AND node_uid IS NULL) OR (node_name IS NOT NULL AND node_uid IS NOT NULL))
);
CREATE INDEX team_host_trust_original ON team_host_trust_profiles(team_id,host_id,engine_id,active);
CREATE TABLE team_host_trust_operations (
 actor_id TEXT REFERENCES identity_subjects(id), operation_id TEXT NOT NULL,
 profile_id TEXT NOT NULL REFERENCES team_host_trust_profiles(id), request_sha256 TEXT NOT NULL,
 receipt TEXT NOT NULL, created_at BIGINT NOT NULL, PRIMARY KEY(profile_id,operation_id)
);
CREATE TABLE team_container_host_admissions (
 nonce TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES team_runs(id), team_id TEXT NOT NULL REFERENCES identity_organizations(id),
 order_id TEXT NOT NULL REFERENCES team_orders(id), host_id TEXT NOT NULL, engine_id TEXT NOT NULL,
 profile_id TEXT REFERENCES team_host_trust_profiles(id), profile_revision BIGINT CHECK(profile_revision>0), profile_sha256 TEXT,
 approval_id TEXT NOT NULL REFERENCES team_tool_approvals(id), approval_request_sha256 TEXT NOT NULL, original_lease_owner TEXT NOT NULL,
 original_binding TEXT NOT NULL, source_record TEXT NOT NULL, source_sha256 TEXT NOT NULL,
 received_container_id TEXT, received_record TEXT, received_record_sha256 TEXT, original_created_at BIGINT NOT NULL,
 dispatch_admitted BIGINT NOT NULL DEFAULT 0 CHECK(dispatch_admitted IN(0,1)),
 retired_receipt_sha256 TEXT, normal_removed_receipt TEXT, UNIQUE(run_id,approval_id,nonce),
 CHECK((profile_id IS NULL AND profile_revision IS NULL AND profile_sha256 IS NULL) OR (profile_id IS NOT NULL AND profile_revision IS NOT NULL AND profile_sha256 IS NOT NULL)),
 CHECK((received_container_id IS NULL AND received_record IS NULL AND received_record_sha256 IS NULL) OR
       (received_container_id IS NOT NULL AND received_record IS NOT NULL AND received_record_sha256 IS NOT NULL))
);
CREATE INDEX team_container_host_run ON team_container_host_admissions(run_id,nonce);
CREATE TABLE team_lost_host_retirement_operations (
 run_id TEXT NOT NULL REFERENCES team_runs(id), operation_id TEXT NOT NULL,
 actor_id TEXT NOT NULL REFERENCES identity_subjects(id), nonce TEXT NOT NULL REFERENCES team_container_host_admissions(nonce),
 request_sha256 TEXT NOT NULL, intent_sha256 TEXT NOT NULL, original_intent TEXT NOT NULL,
 original_record_sha256 TEXT NOT NULL, original_profile_sha256 TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN('admitted','received','closed')), witness TEXT, witness_sha256 TEXT, receipt TEXT,
 original_created_at BIGINT NOT NULL, PRIMARY KEY(run_id,operation_id), UNIQUE(nonce),
 CHECK((witness IS NULL AND witness_sha256 IS NULL AND receipt IS NULL AND state='admitted') OR
       (witness IS NOT NULL AND witness_sha256 IS NOT NULL AND receipt IS NOT NULL AND state IN('received','closed')))
);
CREATE TABLE team_lost_host_retirement_members (
 operation_run_id TEXT NOT NULL, operation_id TEXT NOT NULL, nonce TEXT NOT NULL REFERENCES team_container_host_admissions(nonce),
 run_id TEXT NOT NULL REFERENCES team_runs(id), order_id TEXT NOT NULL REFERENCES team_orders(id),
 source_record TEXT NOT NULL, source_record_sha256 TEXT NOT NULL, closure_receipt TEXT, closed BIGINT NOT NULL DEFAULT 0 CHECK(closed IN(0,1)),
 PRIMARY KEY(operation_run_id,operation_id,nonce), UNIQUE(nonce),
 FOREIGN KEY(operation_run_id,operation_id) REFERENCES team_lost_host_retirement_operations(run_id,operation_id),
 CHECK(closed=0 OR closure_receipt IS NOT NULL)
);
CREATE INDEX team_lost_host_members_run ON team_lost_host_retirement_members(run_id,nonce);
