-- Private routing metadata only. Public Desktop and Executor contracts stay unchanged.
CREATE TABLE api_cluster_binding (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    group_id uuid NOT NULL,
    protocol_version integer NOT NULL CHECK (protocol_version = 1)
);

CREATE TABLE api_cluster_nodes (
    group_id uuid NOT NULL,
    node_id uuid NOT NULL,
    process_epoch uuid NOT NULL,
    certificate_sha256 bytea NOT NULL CHECK (octet_length(certificate_sha256) = 32),
    lease_until timestamptz NOT NULL,
    draining boolean NOT NULL DEFAULT false,
    PRIMARY KEY (group_id, node_id)
);

CREATE TABLE api_cluster_executor_owners (
    executor_id uuid PRIMARY KEY REFERENCES executors(id) ON DELETE CASCADE,
    group_id uuid NOT NULL,
    node_id uuid NOT NULL,
    process_epoch uuid NOT NULL,
    connection_epoch uuid NOT NULL,
    credential_id uuid NOT NULL REFERENCES executor_credentials(id),
    lease_until timestamptz NOT NULL,
    FOREIGN KEY (group_id, node_id) REFERENCES api_cluster_nodes(group_id, node_id)
);

CREATE TABLE api_cluster_desktop_owners (
    desktop_session_id uuid PRIMARY KEY REFERENCES desktop_sessions(id) ON DELETE CASCADE,
    group_id uuid NOT NULL,
    node_id uuid NOT NULL,
    process_epoch uuid NOT NULL,
    executor_id uuid REFERENCES executors(id),
    connection_epoch uuid,
    credential_id uuid REFERENCES executor_credentials(id),
    FOREIGN KEY (group_id, node_id) REFERENCES api_cluster_nodes(group_id, node_id),
    CHECK ((executor_id IS NULL AND connection_epoch IS NULL AND credential_id IS NULL)
        OR (executor_id IS NOT NULL AND connection_epoch IS NOT NULL AND credential_id IS NOT NULL))
);

ALTER TABLE desktop_stream_tickets ADD COLUMN auth_session_id uuid REFERENCES auth_sessions(id);

CREATE TABLE api_cluster_desktop_pending (
    stream_id uuid PRIMARY KEY,
    group_id uuid NOT NULL,
    origin_node_id uuid NOT NULL,
    origin_process_epoch uuid NOT NULL,
    owner_node_id uuid NOT NULL,
    owner_process_epoch uuid NOT NULL,
    executor_id uuid REFERENCES executors(id),
    connection_epoch uuid,
    credential_id uuid REFERENCES executor_credentials(id),
    desktop_session_id uuid NOT NULL REFERENCES desktop_sessions(id) ON DELETE CASCADE,
    run_id uuid NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES users(id),
    auth_session_id uuid NOT NULL REFERENCES auth_sessions(id),
    control boolean NOT NULL,
    expires_at timestamptz NOT NULL,
    user_claimed_at timestamptz,
    command_claimed_at timestamptz,
    command_sent_at timestamptz,
    callback_claimed_at timestamptz,
    revoked_at timestamptz,
    FOREIGN KEY (group_id, origin_node_id) REFERENCES api_cluster_nodes(group_id, node_id),
    FOREIGN KEY (group_id, owner_node_id) REFERENCES api_cluster_nodes(group_id, node_id),
    CHECK ((executor_id IS NULL AND connection_epoch IS NULL AND credential_id IS NULL)
        OR (executor_id IS NOT NULL AND connection_epoch IS NOT NULL AND credential_id IS NOT NULL))
);
CREATE INDEX api_cluster_pending_expiry ON api_cluster_desktop_pending(expires_at);
