CREATE TABLE organisation_mfa_policies (
    organisation_id UUID PRIMARY KEY REFERENCES management_organisations(id),
    revision BIGINT NOT NULL DEFAULT 1 CHECK(revision>0),
    epoch UUID NOT NULL,
    required BOOLEAN NOT NULL DEFAULT FALSE,
    max_age_seconds INTEGER NOT NULL DEFAULT 43200 CHECK(max_age_seconds BETWEEN 300 AND 43200),
    allow_validated_idp BOOLEAN NOT NULL DEFAULT TRUE,
    updated_by UUID REFERENCES users(id),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE organisation_mfa_operations (
    organisation_id UUID NOT NULL REFERENCES management_organisations(id),
    actor_user_id UUID NOT NULL REFERENCES users(id),
    request_id UUID NOT NULL,
    expected_revision BIGINT NOT NULL CHECK(expected_revision>0),
    request_sha256 TEXT NOT NULL CHECK(request_sha256 ~ '^[0-9a-f]{64}$'),
    receipt_bytes BYTEA NOT NULL CHECK(octet_length(receipt_bytes) BETWEEN 1 AND 16384),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY(organisation_id,actor_user_id,request_id),
    UNIQUE(organisation_id,expected_revision)
);
CREATE TABLE run_mfa_admissions (
    run_id UUID PRIMARY KEY REFERENCES runs(id),
    owner_user_id UUID NOT NULL REFERENCES users(id),
    auth_session_id UUID NOT NULL REFERENCES auth_sessions(id),
    spec_sha256 TEXT NOT NULL CHECK(spec_sha256 ~ '^[0-9a-f]{64}$'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX run_mfa_admissions_owner_session ON run_mfa_admissions(owner_user_id,auth_session_id);
CREATE TABLE run_mfa_continuations (
    run_id UUID NOT NULL REFERENCES runs(id),
    queued_revision BIGINT NOT NULL CHECK(queued_revision>0),
    owner_user_id UUID NOT NULL REFERENCES users(id),
    auth_session_id UUID NOT NULL REFERENCES auth_sessions(id),
    request_id UUID NOT NULL,
    spec_sha256 TEXT NOT NULL CHECK(spec_sha256 ~ '^[0-9a-f]{64}$'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY(run_id,queued_revision),
    UNIQUE(run_id,request_id)
);
CREATE TABLE schedule_mfa_admissions (
    schedule_id UUID PRIMARY KEY REFERENCES schedules(id),
    owner_user_id UUID NOT NULL REFERENCES users(id),
    auth_session_id UUID NOT NULL REFERENCES auth_sessions(id),
    revision BIGINT NOT NULL CHECK(revision>0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX schedule_mfa_admissions_owner_session ON schedule_mfa_admissions(owner_user_id,auth_session_id);
ALTER TABLE terminal_stream_tickets ADD COLUMN auth_session_id UUID REFERENCES auth_sessions(id);
ALTER TABLE terminal_sessions ADD COLUMN auth_session_id UUID REFERENCES auth_sessions(id);

INSERT INTO organisation_mfa_policies(organisation_id,epoch)
    SELECT id,gen_random_uuid() FROM management_organisations;

ALTER TABLE management_instance_features ADD COLUMN mfa_version SMALLINT NOT NULL DEFAULT 0 CHECK(mfa_version BETWEEN 0 AND 1);
CREATE INDEX run_mfa_admissions_session ON run_mfa_admissions(auth_session_id);
CREATE INDEX run_mfa_continuations_session ON run_mfa_continuations(auth_session_id);
CREATE INDEX schedule_mfa_admissions_session ON schedule_mfa_admissions(auth_session_id);
CREATE INDEX terminal_stream_tickets_auth_session ON terminal_stream_tickets(auth_session_id);
CREATE INDEX terminal_sessions_auth_session ON terminal_sessions(auth_session_id);
