CREATE TABLE management_invitations (
    id UUID PRIMARY KEY,
    organisation_id UUID NOT NULL REFERENCES management_organisations(id),
    email TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('admin','member','auditor')),
    token_hash BYTEA NOT NULL UNIQUE CHECK (octet_length(token_hash)=32),
    invited_by UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    accepted_by UUID REFERENCES users(id),
    accepted_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    CHECK (expires_at>created_at AND expires_at<=created_at+interval '30 days'),
    CHECK ((accepted_by IS NULL)=(accepted_at IS NULL)),
    CHECK (accepted_at IS NULL OR revoked_at IS NULL)
);
CREATE INDEX management_invitations_organisation ON management_invitations(organisation_id,created_at DESC);
