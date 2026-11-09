-- Private purpose-bound commands. A consumed unknown stop is never resent.
CREATE TABLE api_cluster_desktop_controls (
    ticket_id uuid PRIMARY KEY,
    group_id uuid NOT NULL,
    origin_node_id uuid NOT NULL,
    origin_process_epoch uuid NOT NULL,
    owner_node_id uuid NOT NULL,
    owner_process_epoch uuid NOT NULL,
    desktop_session_id uuid NOT NULL REFERENCES desktop_sessions(id) ON DELETE CASCADE,
    run_id uuid NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES users(id),
    auth_session_id uuid NOT NULL REFERENCES auth_sessions(id),
    purpose text NOT NULL CHECK (purpose = 'stop'),
    binding_sha256 text NOT NULL CHECK (binding_sha256 ~ '^[0-9a-f]{64}$'),
    expires_at timestamptz NOT NULL,
    claimed_at timestamptz,
    completed_at timestamptz,
    FOREIGN KEY (group_id, origin_node_id) REFERENCES api_cluster_nodes(group_id,node_id),
    FOREIGN KEY (group_id, owner_node_id) REFERENCES api_cluster_nodes(group_id,node_id),
    CHECK (completed_at IS NULL OR claimed_at IS NOT NULL)
);
CREATE INDEX api_cluster_desktop_controls_expiry ON api_cluster_desktop_controls(expires_at);
