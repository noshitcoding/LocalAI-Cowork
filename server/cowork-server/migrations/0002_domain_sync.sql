CREATE TABLE users (
    id UUID PRIMARY KEY,
    revision BIGINT NOT NULL DEFAULT 1,
    etag TEXT NOT NULL,
    email TEXT NOT NULL,
    display_name TEXT NOT NULL,
    platform_admin BOOLEAN NOT NULL DEFAULT FALSE,
    password_hash TEXT,
    totp_secret_ciphertext BYTEA,
    totp_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    recovery_codes_hashes JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);
CREATE UNIQUE INDEX users_email_active_unique ON users (lower(email)) WHERE deleted_at IS NULL;

CREATE TABLE user_passkeys (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    credential_id BYTEA NOT NULL UNIQUE,
    public_key BYTEA NOT NULL,
    sign_count BIGINT NOT NULL DEFAULT 0,
    transports JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ
);

CREATE TABLE oidc_identities (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    issuer TEXT NOT NULL,
    subject TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (issuer, subject)
);

CREATE TABLE auth_sessions (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_id UUID NOT NULL,
    refresh_token_hash BYTEA NOT NULL UNIQUE,
    refresh_family_id UUID NOT NULL,
    previous_token_hash BYTEA,
    user_agent TEXT,
    ip_address INET,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    revoke_reason TEXT
);
CREATE INDEX auth_sessions_user_idx ON auth_sessions (user_id, expires_at) WHERE revoked_at IS NULL;

CREATE TABLE invitations (
    id UUID PRIMARY KEY,
    email TEXT NOT NULL,
    token_hash BYTEA NOT NULL UNIQUE,
    invited_by UUID NOT NULL REFERENCES users(id),
    expires_at TIMESTAMPTZ NOT NULL,
    accepted_by UUID REFERENCES users(id),
    accepted_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE teams (
    id UUID PRIMARY KEY,
    revision BIGINT NOT NULL DEFAULT 1,
    etag TEXT NOT NULL,
    name TEXT NOT NULL,
    owner_user_id UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE TABLE team_members (
    team_id UUID NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'member')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (team_id, user_id)
);

CREATE TABLE projects (
    id UUID PRIMARY KEY,
    revision BIGINT NOT NULL DEFAULT 1,
    etag TEXT NOT NULL,
    owner_user_id UUID NOT NULL REFERENCES users(id),
    team_id UUID REFERENCES teams(id),
    privacy TEXT NOT NULL CHECK (privacy IN ('private_local', 'team_managed')),
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    preferred_executor_target JSONB,
    current_version_id UUID,
    policy JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    CHECK (
        (privacy = 'private_local' AND team_id IS NULL)
        OR (privacy = 'team_managed' AND team_id IS NOT NULL)
    )
);
CREATE INDEX projects_owner_idx ON projects (owner_user_id) WHERE deleted_at IS NULL;
CREATE INDEX projects_team_idx ON projects (team_id) WHERE deleted_at IS NULL;

CREATE TABLE project_members (
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('viewer', 'runner', 'editor')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, user_id)
);

CREATE TABLE threads (
    id UUID PRIMARY KEY,
    revision BIGINT NOT NULL DEFAULT 1,
    etag TEXT NOT NULL,
    project_id UUID NOT NULL REFERENCES projects(id),
    created_by UUID NOT NULL REFERENCES users(id),
    forked_from_thread_id UUID REFERENCES threads(id),
    forked_from_message_id UUID,
    title TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);
CREATE INDEX threads_project_idx ON threads (project_id, updated_at DESC) WHERE deleted_at IS NULL;

CREATE TABLE messages (
    id UUID PRIMARY KEY,
    revision BIGINT NOT NULL DEFAULT 1,
    etag TEXT NOT NULL,
    thread_id UUID NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    author_user_id UUID REFERENCES users(id),
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'system', 'tool')),
    content JSONB NOT NULL,
    run_id UUID REFERENCES runs(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);
ALTER TABLE threads
    ADD CONSTRAINT threads_fork_message_fk
    FOREIGN KEY (forked_from_message_id) REFERENCES messages(id);
CREATE INDEX messages_thread_idx ON messages (thread_id, created_at) WHERE deleted_at IS NULL;

CREATE TABLE task_definitions (
    id UUID NOT NULL,
    revision BIGINT NOT NULL,
    etag TEXT NOT NULL,
    project_id UUID NOT NULL REFERENCES projects(id),
    name TEXT NOT NULL,
    instructions TEXT NOT NULL,
    required_capabilities JSONB NOT NULL DEFAULT '[]'::jsonb,
    default_executor_target JSONB,
    config JSONB NOT NULL DEFAULT '{}'::jsonb,
    released BOOLEAN NOT NULL DEFAULT FALSE,
    created_by UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (id, revision)
);
CREATE UNIQUE INDEX task_definitions_current_release
    ON task_definitions (id) WHERE released AND deleted_at IS NULL;

CREATE TABLE schedules (
    id UUID PRIMARY KEY,
    revision BIGINT NOT NULL DEFAULT 1,
    etag TEXT NOT NULL,
    task_id UUID NOT NULL,
    project_id UUID NOT NULL REFERENCES projects(id),
    thread_id UUID NOT NULL REFERENCES threads(id),
    cron_expression TEXT NOT NULL,
    timezone TEXT NOT NULL,
    executor_target JSONB NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    next_run_at TIMESTAMPTZ,
    last_triggered_at TIMESTAMPTZ,
    blocked_reason TEXT,
    created_by UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);
CREATE INDEX schedules_due_idx ON schedules (next_run_at)
    WHERE enabled AND deleted_at IS NULL;

CREATE TABLE snapshot_manifests (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id),
    created_by UUID NOT NULL REFERENCES users(id),
    key_scope_type TEXT NOT NULL CHECK (key_scope_type IN ('user', 'team')),
    key_scope_id UUID NOT NULL,
    encryption_key_id TEXT NOT NULL,
    total_bytes BIGINT NOT NULL CHECK (total_bytes >= 0),
    file_count BIGINT NOT NULL CHECK (file_count >= 0),
    manifest JSONB NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('uploading', 'ready', 'expired', 'deleting')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ
);
CREATE INDEX snapshot_manifests_expiry_idx ON snapshot_manifests (expires_at)
    WHERE status IN ('ready', 'expired');

CREATE TABLE snapshot_chunks (
    key_scope_type TEXT NOT NULL CHECK (key_scope_type IN ('user', 'team')),
    key_scope_id UUID NOT NULL,
    plaintext_digest BYTEA NOT NULL,
    object_key TEXT NOT NULL UNIQUE,
    plaintext_size BIGINT NOT NULL CHECK (plaintext_size >= 0),
    ciphertext_size BIGINT NOT NULL CHECK (ciphertext_size >= 0),
    wrapped_data_key BYTEA NOT NULL,
    nonce BYTEA NOT NULL,
    ref_count BIGINT NOT NULL DEFAULT 0 CHECK (ref_count >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_referenced_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (key_scope_type, key_scope_id, plaintext_digest)
);

CREATE TABLE snapshot_manifest_chunks (
    manifest_id UUID NOT NULL REFERENCES snapshot_manifests(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    chunk_index INTEGER NOT NULL CHECK (chunk_index >= 0),
    key_scope_type TEXT NOT NULL,
    key_scope_id UUID NOT NULL,
    plaintext_digest BYTEA NOT NULL,
    PRIMARY KEY (manifest_id, path, chunk_index),
    FOREIGN KEY (key_scope_type, key_scope_id, plaintext_digest)
        REFERENCES snapshot_chunks(key_scope_type, key_scope_id, plaintext_digest)
);

CREATE TABLE project_versions (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id),
    revision BIGINT NOT NULL,
    parent_version_id UUID REFERENCES project_versions(id),
    merge_base_version_id UUID REFERENCES project_versions(id),
    snapshot_manifest_id UUID NOT NULL REFERENCES snapshot_manifests(id),
    created_by_user_id UUID REFERENCES users(id),
    created_by_run_id UUID REFERENCES runs(id),
    diff_summary JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, revision)
);
ALTER TABLE projects
    ADD CONSTRAINT projects_current_version_fk
    FOREIGN KEY (current_version_id) REFERENCES project_versions(id);

CREATE TABLE run_artifacts (
    id UUID PRIMARY KEY,
    run_id UUID NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    revision BIGINT NOT NULL DEFAULT 1,
    kind TEXT NOT NULL,
    media_type TEXT NOT NULL,
    name TEXT NOT NULL,
    object_key TEXT NOT NULL,
    digest BYTEA NOT NULL,
    size_bytes BIGINT NOT NULL CHECK (size_bytes >= 0),
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);
CREATE INDEX run_artifacts_run_idx ON run_artifacts (run_id, created_at);

CREATE TABLE approval_requests (
    id UUID PRIMARY KEY,
    run_id UUID NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    revision BIGINT NOT NULL DEFAULT 1,
    etag TEXT NOT NULL,
    requested_action JSONB NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending', 'approved', 'rejected', 'expired')),
    requested_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    resolved_by UUID REFERENCES users(id),
    resolved_at TIMESTAMPTZ
);
CREATE INDEX approval_requests_pending_idx ON approval_requests (expires_at)
    WHERE state = 'pending';

CREATE TABLE desktop_sessions (
    id UUID PRIMARY KEY,
    run_id UUID NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    executor_id UUID NOT NULL REFERENCES executors(id),
    state TEXT NOT NULL CHECK (state IN ('starting', 'agent_controlled', 'user_controlled', 'paused', 'ended', 'failed')),
    stream_protocol TEXT NOT NULL,
    dimensions JSONB,
    controller_user_id UUID REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    ended_at TIMESTAMPTZ
);

CREATE TABLE provider_profiles (
    id UUID PRIMARY KEY,
    revision BIGINT NOT NULL DEFAULT 1,
    etag TEXT NOT NULL,
    owner_user_id UUID REFERENCES users(id),
    team_id UUID REFERENCES teams(id),
    name TEXT NOT NULL,
    provider_kind TEXT NOT NULL,
    model_defaults JSONB NOT NULL DEFAULT '{}'::jsonb,
    encrypted_secret BYTEA,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    CHECK ((owner_user_id IS NULL) <> (team_id IS NULL))
);

CREATE TABLE device_provider_bindings (
    profile_id UUID NOT NULL REFERENCES provider_profiles(id) ON DELETE CASCADE,
    device_id UUID NOT NULL,
    endpoint_ciphertext BYTEA,
    available BOOLEAN NOT NULL DEFAULT FALSE,
    checked_at TIMESTAMPTZ,
    PRIMARY KEY (profile_id, device_id)
);

CREATE TABLE sync_operations (
    operation_id UUID PRIMARY KEY,
    device_id UUID NOT NULL,
    user_id UUID NOT NULL REFERENCES users(id),
    entity_type TEXT NOT NULL,
    entity_id UUID NOT NULL,
    base_revision BIGINT NOT NULL,
    operation TEXT NOT NULL CHECK (operation IN ('upsert', 'delete')),
    payload JSONB,
    client_timestamp TIMESTAMPTZ NOT NULL,
    server_revision BIGINT,
    result JSONB,
    accepted_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX sync_operations_entity_idx ON sync_operations (entity_type, entity_id, accepted_at);

CREATE TABLE sync_changes (
    cursor BIGSERIAL PRIMARY KEY,
    user_id UUID REFERENCES users(id),
    team_id UUID REFERENCES teams(id),
    entity_type TEXT NOT NULL,
    entity_id UUID NOT NULL,
    revision BIGINT NOT NULL,
    operation TEXT NOT NULL CHECK (operation IN ('upsert', 'delete')),
    payload JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (user_id IS NOT NULL OR team_id IS NOT NULL)
);
CREATE INDEX sync_changes_user_cursor_idx ON sync_changes (user_id, cursor) WHERE user_id IS NOT NULL;
CREATE INDEX sync_changes_team_cursor_idx ON sync_changes (team_id, cursor) WHERE team_id IS NOT NULL;

CREATE TABLE sync_device_cursors (
    device_id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id),
    last_cursor BIGINT NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE quota_limits (
    scope_type TEXT NOT NULL CHECK (scope_type IN ('user', 'team')),
    scope_id UUID NOT NULL,
    storage_bytes BIGINT,
    concurrent_runs INTEGER,
    monthly_tokens BIGINT,
    monthly_cost_micros BIGINT,
    hard_cost_limit BOOLEAN NOT NULL DEFAULT TRUE,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (scope_type, scope_id)
);

CREATE TABLE quota_usage (
    scope_type TEXT NOT NULL CHECK (scope_type IN ('user', 'team')),
    scope_id UUID NOT NULL,
    period_start DATE NOT NULL,
    storage_bytes BIGINT NOT NULL DEFAULT 0,
    running_runs INTEGER NOT NULL DEFAULT 0,
    tokens BIGINT NOT NULL DEFAULT 0,
    cost_micros BIGINT NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (scope_type, scope_id, period_start)
);

CREATE TABLE support_grants (
    id UUID PRIMARY KEY,
    granted_by UUID NOT NULL REFERENCES users(id),
    support_user_id UUID NOT NULL REFERENCES users(id),
    project_id UUID REFERENCES projects(id),
    thread_id UUID REFERENCES threads(id),
    reason TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    CHECK (expires_at <= created_at + interval '24 hours'),
    CHECK (project_id IS NOT NULL OR thread_id IS NOT NULL)
);
CREATE INDEX support_grants_active_idx ON support_grants (support_user_id, expires_at)
    WHERE revoked_at IS NULL;

CREATE TABLE notification_endpoints (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('fcm', 'webpush')),
    endpoint_ciphertext BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ
);
