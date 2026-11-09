-- Instance-local distributed coordination. Offline work retains its original
-- assignment and capacity; connectivity loss never creates a second dispatch.
CREATE TABLE enterprise_device_grants (
    id UUID PRIMARY KEY,
    owner_user_id UUID NOT NULL REFERENCES users(id),
    auth_session_id UUID NOT NULL REFERENCES auth_sessions(id),
    device_id UUID NOT NULL REFERENCES executors(id),
    chat_id UUID NOT NULL REFERENCES threads(id),
    revision BIGINT NOT NULL CHECK(revision > 0),
    request_id UUID NOT NULL,
    request_sha256 TEXT NOT NULL,
    initial_receipt JSONB NOT NULL,
    policy JSONB NOT NULL,
    policy_sha256 TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN('pending','active','stopped','revoked')),
    confirmation_credential_id UUID REFERENCES executor_credentials(id),
    confirmation JSONB,
    confirmed_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE(owner_user_id,request_id)
);
CREATE TABLE enterprise_workflows (
    id UUID PRIMARY KEY,
    owner_user_id UUID NOT NULL REFERENCES users(id),
    auth_session_id UUID NOT NULL REFERENCES auth_sessions(id),
    chat_id UUID NOT NULL REFERENCES threads(id),
    project_id UUID NOT NULL REFERENCES projects(id),
    request_id UUID NOT NULL,
    request_sha256 TEXT NOT NULL,
    original JSONB NOT NULL,
    initial_receipt JSONB NOT NULL,
    revision BIGINT NOT NULL DEFAULT 1 CHECK(revision > 0),
    stop_auth_session_id UUID REFERENCES auth_sessions(id),
    state TEXT NOT NULL CHECK(state IN('active','held','stopping','completed','failed')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE(owner_user_id,request_id)
);
CREATE TABLE enterprise_workflow_steps (
    workflow_id UUID NOT NULL REFERENCES enterprise_workflows(id),
    id UUID NOT NULL,
    ordinal BIGINT NOT NULL CHECK(ordinal >= 0),
    dependencies JSONB NOT NULL,
    destination JSONB NOT NULL,
    request JSONB NOT NULL,
    request_sha256 TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN('pending','admitted','running','offline_running','held','completed','failed','halted','canceled')),
    device_grant_id UUID REFERENCES enterprise_device_grants(id),
    device_grant_revision BIGINT,
    child_run_id UUID UNIQUE REFERENCES runs(id),
    dispatch_auth_session_id UUID REFERENCES auth_sessions(id),
    remote_request_id UUID UNIQUE,
    receipt JSONB,
    original_effects_unknown BOOLEAN NOT NULL DEFAULT FALSE,
    stop_admitted BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY(workflow_id,id),
    UNIQUE(workflow_id,ordinal),
    CHECK((device_grant_id IS NULL)=(device_grant_revision IS NULL))
);
CREATE TABLE enterprise_workflow_continuations (
    workflow_id UUID NOT NULL REFERENCES enterprise_workflows(id),
    revision BIGINT NOT NULL CHECK(revision > 1),
    request_id UUID NOT NULL UNIQUE,
    auth_session_id UUID NOT NULL REFERENCES auth_sessions(id),
    receipt JSONB NOT NULL,
    PRIMARY KEY(workflow_id,revision)
);
CREATE TABLE enterprise_workflow_reassignments (
    workflow_id UUID NOT NULL REFERENCES enterprise_workflows(id),
    revision BIGINT NOT NULL,
    step_id UUID NOT NULL,
    request_id UUID NOT NULL UNIQUE,
    auth_session_id UUID NOT NULL REFERENCES auth_sessions(id),
    original_step JSONB NOT NULL,
    replacement JSONB NOT NULL,
    receipt JSONB NOT NULL,
    PRIMARY KEY(workflow_id,revision)
);
CREATE TABLE enterprise_execution_operations (
    owner_user_id UUID NOT NULL REFERENCES users(id),
    request_id UUID NOT NULL,
    kind TEXT NOT NULL,
    target_id UUID NOT NULL,
    request_sha256 TEXT NOT NULL,
    receipt JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(owner_user_id,request_id)
);
CREATE TABLE enterprise_execution_requests (
    owner_user_id UUID NOT NULL REFERENCES users(id),
    request_id UUID NOT NULL,
    kind TEXT NOT NULL,
    target_id UUID NOT NULL,
    request_sha256 TEXT NOT NULL,
    admitted_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(owner_user_id,request_id)
);
CREATE TABLE enterprise_transfer_grants (
    id UUID PRIMARY KEY,
    owner_user_id UUID NOT NULL REFERENCES users(id),
    auth_session_id UUID NOT NULL REFERENCES auth_sessions(id),
    chat_id UUID NOT NULL REFERENCES threads(id),
    revision BIGINT NOT NULL CHECK(revision > 0),
    request_id UUID NOT NULL,
    request_sha256 TEXT NOT NULL,
    initial_receipt JSONB NOT NULL,
    policy JSONB NOT NULL,
    source_grant_id UUID REFERENCES enterprise_device_grants(id),
    destination_grant_id UUID REFERENCES enterprise_device_grants(id),
    environment_pins JSONB NOT NULL DEFAULT '{}',
    state TEXT NOT NULL CHECK(state IN('active','revoked')),
    reserved_bytes BIGINT NOT NULL DEFAULT 0 CHECK(reserved_bytes >= 0),
    expires_at TIMESTAMPTZ NOT NULL,
    UNIQUE(owner_user_id,request_id)
);
CREATE TABLE enterprise_transfers (
    id UUID PRIMARY KEY,
    grant_id UUID NOT NULL REFERENCES enterprise_transfer_grants(id),
    grant_revision BIGINT NOT NULL CHECK(grant_revision > 0),
    owner_user_id UUID NOT NULL REFERENCES users(id),
    request_id UUID NOT NULL,
    request_sha256 TEXT NOT NULL,
    initial_receipt JSONB NOT NULL,
    relative_path TEXT NOT NULL,
    original_destination_sha256 TEXT,
    size BIGINT NOT NULL CHECK(size >= 0 AND size <= 16777216),
    sha256 TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN('receiving','ready','conflict','completed','retained')),
    ciphertext BYTEA,
    encrypted_data_key BYTEA,
    nonce BYTEA,
    wrap_nonce BYTEA,
    receipt JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE(owner_user_id,request_id),
    CHECK((ciphertext IS NULL)=(encrypted_data_key IS NULL)),
    CHECK((ciphertext IS NULL)=(nonce IS NULL)),
    CHECK((ciphertext IS NULL)=(wrap_nonce IS NULL))
);
CREATE TABLE enterprise_transfer_environment_operations (
    transfer_id UUID NOT NULL REFERENCES enterprise_transfers(id),
    leg TEXT NOT NULL CHECK(leg IN('source','destination')),
    request_id UUID NOT NULL UNIQUE,
    original_request JSONB NOT NULL,
    request_sha256 TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN('admitted','completed','held')),
    receipt JSONB,
    receipt_ciphertext BYTEA,
    receipt_encrypted_data_key BYTEA,
    receipt_nonce BYTEA,
    receipt_wrap_nonce BYTEA,
    receipt_bytes BIGINT NOT NULL DEFAULT 0 CHECK(receipt_bytes >= 0 AND receipt_bytes <= 2097152),
    PRIMARY KEY(transfer_id,leg)
);
CREATE TABLE enterprise_offline_permits (
    id UUID PRIMARY KEY,
    run_id UUID NOT NULL REFERENCES runs(id),
    device_id UUID NOT NULL REFERENCES executors(id),
    credential_id UUID NOT NULL REFERENCES executor_credentials(id),
    grant_id UUID NOT NULL REFERENCES enterprise_device_grants(id),
    grant_revision BIGINT NOT NULL CHECK(grant_revision > 0),
    original_lease_token UUID NOT NULL,
    original_spec_sha256 TEXT NOT NULL,
    valid_until TIMESTAMPTZ NOT NULL,
    receipt JSONB NOT NULL,
    reconciled_at TIMESTAMPTZ,
    UNIQUE(run_id,original_lease_token)
);
CREATE TABLE enterprise_offline_events (
    permit_id UUID NOT NULL REFERENCES enterprise_offline_permits(id),
    sequence BIGINT NOT NULL CHECK(sequence > 0),
    event_id UUID NOT NULL UNIQUE,
    source_sha256 TEXT NOT NULL,
    original JSONB NOT NULL,
    receipt JSONB NOT NULL,
    PRIMARY KEY(permit_id,sequence)
);
CREATE FUNCTION enterprise_execution_writer_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS(SELECT 1 FROM enterprise_workflow_steps WHERE child_run_id=OLD.id)
       AND current_setting('cowork.enterprise_execution_version',true) IS DISTINCT FROM '1' THEN
        RAISE EXCEPTION 'enterprise execution requires its current original-grant writer';
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER enterprise_execution_writer BEFORE UPDATE ON runs
    FOR EACH ROW EXECUTE FUNCTION enterprise_execution_writer_guard();
