CREATE TABLE IF NOT EXISTS mcp_oauth_profiles (
 id TEXT PRIMARY KEY, owner_kind TEXT NOT NULL CHECK(owner_kind IN('native','personal','team')),
 owner_id TEXT NOT NULL, project_id TEXT NOT NULL, server_id TEXT NOT NULL,
 revision BIGINT NOT NULL CHECK(revision>0), epoch TEXT NOT NULL,
 enabled BIGINT NOT NULL CHECK(enabled IN(0,1)), binding_sha256 TEXT NOT NULL,
 payload TEXT NOT NULL, secret_scope TEXT NOT NULL, sealed_client TEXT,
 active_token_id TEXT, pending_exchange_id TEXT, created_at BIGINT NOT NULL, updated_at BIGINT NOT NULL,
 UNIQUE(owner_kind,owner_id,project_id,server_id)
);
CREATE TABLE IF NOT EXISTS mcp_oauth_configuration_operations (
 owner_kind TEXT NOT NULL, owner_id TEXT NOT NULL, project_id TEXT NOT NULL,
 request_key TEXT NOT NULL, request_sha256 TEXT NOT NULL, request_key_id TEXT NOT NULL,
 profile_id TEXT NOT NULL REFERENCES mcp_oauth_profiles(id), receipt TEXT NOT NULL,
 PRIMARY KEY(owner_kind,owner_id,project_id,request_key)
);
CREATE TABLE IF NOT EXISTS mcp_oauth_flows (
 id TEXT PRIMARY KEY, profile_id TEXT NOT NULL REFERENCES mcp_oauth_profiles(id),
 profile_revision BIGINT NOT NULL, profile_epoch TEXT NOT NULL, binding_sha256 TEXT NOT NULL,
 original_actor TEXT NOT NULL, redirect_uri TEXT NOT NULL, metadata_sha256 TEXT NOT NULL,
 state_sha256 TEXT NOT NULL UNIQUE, state TEXT NOT NULL CHECK(state IN('awaiting_callback','callback_received','exchange_admitted','connected','denied','unknown','expired')),
 secret_scope TEXT NOT NULL, sealed_flow TEXT NOT NULL, sealed_callback TEXT,
 callback_sha256 TEXT, expires_at BIGINT NOT NULL, receipt TEXT, created_at BIGINT NOT NULL
);
CREATE TABLE IF NOT EXISTS mcp_oauth_tokens (
 id TEXT PRIMARY KEY, profile_id TEXT NOT NULL REFERENCES mcp_oauth_profiles(id),
 profile_revision BIGINT NOT NULL, profile_epoch TEXT NOT NULL, binding_sha256 TEXT NOT NULL,
 grant_id TEXT NOT NULL, revision BIGINT NOT NULL CHECK(revision>0),
 state TEXT NOT NULL CHECK(state IN('current','refresh_admitted','retired','revoked','unknown')),
 expires_at BIGINT NOT NULL, scopes TEXT NOT NULL, secret_scope TEXT NOT NULL, sealed_token TEXT NOT NULL,
 original_exchange_id TEXT NOT NULL UNIQUE, created_at BIGINT NOT NULL,
 UNIQUE(profile_id,grant_id,revision)
);
CREATE TABLE IF NOT EXISTS mcp_oauth_exchanges (
 id TEXT PRIMARY KEY, profile_id TEXT NOT NULL REFERENCES mcp_oauth_profiles(id),
 profile_revision BIGINT NOT NULL, profile_epoch TEXT NOT NULL,
 kind TEXT NOT NULL CHECK(kind IN('authorization_code','refresh_token')),
 source_id TEXT NOT NULL, request_sha256 TEXT NOT NULL, authority_sha256 TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN('admitted','received','committed','rejected','unknown')),
 secret_scope TEXT NOT NULL, sealed_response TEXT, response_sha256 TEXT,
 receipt TEXT, created_at BIGINT NOT NULL, updated_at BIGINT NOT NULL,
 UNIQUE(profile_id,kind,source_id)
);
CREATE TABLE IF NOT EXISTS mcp_async_tasks (
 id TEXT PRIMARY KEY, owner_kind TEXT NOT NULL CHECK(owner_kind IN('native','personal','team','offline')),
 owner_id TEXT NOT NULL, project_id TEXT NOT NULL, run_id TEXT NOT NULL, tool_call_id TEXT NOT NULL,
 server_id TEXT NOT NULL, server_revision BIGINT NOT NULL, binding_sha256 TEXT NOT NULL,
 spec_sha256 TEXT NOT NULL, connection_id TEXT NOT NULL, connection_epoch BIGINT NOT NULL,
 protocol TEXT NOT NULL CHECK(protocol='2025-11-25'), original_rpc_id TEXT NOT NULL,
 original_request_sha256 TEXT NOT NULL, transport_request_sha256 TEXT NOT NULL,
 task_id_sha256 TEXT NOT NULL, revision BIGINT NOT NULL CHECK(revision>0),
 state TEXT NOT NULL CHECK(state IN('working','input_required','completed','failed','cancelled','expired','unknown')),
 expires_at BIGINT, poll_interval_ms BIGINT NOT NULL CHECK(poll_interval_ms>0),
 secret_scope TEXT NOT NULL, sealed_ack TEXT NOT NULL, sealed_checkpoint TEXT, checkpoint_key_id TEXT,
 final_result_scope TEXT, sealed_final_result TEXT, final_result_sha256 TEXT,
 pending_operation_id TEXT, created_at BIGINT NOT NULL, updated_at BIGINT NOT NULL,
 UNIQUE(owner_kind,run_id,tool_call_id)
);
CREATE TABLE IF NOT EXISTS mcp_async_task_operations (
 id TEXT PRIMARY KEY, task_record_id TEXT NOT NULL REFERENCES mcp_async_tasks(id),
 expected_revision BIGINT NOT NULL, operation TEXT NOT NULL CHECK(operation IN('get','result','cancel')),
 rpc_request_id TEXT NOT NULL, request_sha256 TEXT NOT NULL, authority_sha256 TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN('admitted','received','committed','unknown')),
 secret_scope TEXT NOT NULL, sealed_response TEXT, response_sha256 TEXT, receipt TEXT,
 created_at BIGINT NOT NULL, updated_at BIGINT NOT NULL,
 UNIQUE(task_record_id,expected_revision), UNIQUE(task_record_id,rpc_request_id)
);
CREATE INDEX IF NOT EXISTS mcp_async_task_run ON mcp_async_tasks(owner_kind,owner_id,project_id,run_id);
CREATE INDEX IF NOT EXISTS mcp_oauth_profile_scope ON mcp_oauth_profiles(owner_kind,owner_id,project_id,enabled);
CREATE TABLE IF NOT EXISTS mcp_task_control_requests (
 task_record_id TEXT NOT NULL REFERENCES mcp_async_tasks(id), request_key TEXT NOT NULL,
 expected_revision BIGINT NOT NULL, operation TEXT NOT NULL CHECK(operation='cancel'),
 state TEXT NOT NULL CHECK(state IN('requested','admitted','completed','unknown','stale')),
 original_actor TEXT NOT NULL, receipt TEXT NOT NULL, created_at BIGINT NOT NULL,
 PRIMARY KEY(task_record_id,request_key), UNIQUE(task_record_id,expected_revision)
);
CREATE TABLE IF NOT EXISTS mcp_task_interactions (
 id TEXT PRIMARY KEY, task_record_id TEXT NOT NULL REFERENCES mcp_async_tasks(id),
 operation_id TEXT NOT NULL REFERENCES mcp_async_task_operations(id),
 server_rpc_id TEXT NOT NULL, method TEXT NOT NULL CHECK(method IN('sampling/createMessage','elicitation/create')),
 request_sha256 TEXT NOT NULL, authority_sha256 TEXT NOT NULL,
 revision BIGINT NOT NULL CHECK(revision>0),
 state TEXT NOT NULL CHECK(state IN('awaiting_user','approved','declined','sampling_admitted','sampling_received','awaiting_release','response_admitted','responded','unknown','expired')),
 request_scope TEXT NOT NULL, sealed_request TEXT NOT NULL,
 decision_scope TEXT, sealed_decision TEXT, decision_actor TEXT,
 model_attempt_id TEXT, model_intent_scope TEXT, sealed_model_intent TEXT, model_request_sha256 TEXT,
 model_response_scope TEXT, sealed_model_response TEXT, model_response_sha256 TEXT,
 response_scope TEXT, sealed_response TEXT, response_sha256 TEXT, receipt TEXT,
 expires_at BIGINT NOT NULL, created_at BIGINT NOT NULL, updated_at BIGINT NOT NULL,
 UNIQUE(task_record_id,server_rpc_id), UNIQUE(model_attempt_id)
);
CREATE TABLE IF NOT EXISTS mcp_task_interaction_decisions (
 interaction_id TEXT NOT NULL REFERENCES mcp_task_interactions(id), request_key TEXT NOT NULL,
 expected_revision BIGINT NOT NULL, request_sha256 TEXT NOT NULL, request_key_id TEXT NOT NULL,
 secret_scope TEXT NOT NULL, sealed_decision TEXT NOT NULL,
 original_actor TEXT NOT NULL, receipt TEXT NOT NULL, created_at BIGINT NOT NULL,
 PRIMARY KEY(interaction_id,request_key), UNIQUE(interaction_id,expected_revision)
);
CREATE TABLE IF NOT EXISTS mcp_task_sampling_claims (
 interaction_id TEXT PRIMARY KEY REFERENCES mcp_task_interactions(id),
 run_id TEXT NOT NULL, node_scope_sha256 TEXT NOT NULL, original_checkpoint_sha256 TEXT NOT NULL,
 reserved_tokens BIGINT NOT NULL CHECK(reserved_tokens>0), charged_tokens BIGINT NOT NULL CHECK(charged_tokens>0),
 state TEXT NOT NULL CHECK(state IN('admitted','received','unknown')),
 original_request_sha256 TEXT NOT NULL, original_response_sha256 TEXT,
 created_at BIGINT NOT NULL, updated_at BIGINT NOT NULL
);
CREATE TABLE IF NOT EXISTS android_private_mcp_closures (
 run_id TEXT NOT NULL REFERENCES android_private_server_runs(id), entity_id TEXT NOT NULL,
 session_revision BIGINT NOT NULL CHECK(session_revision>0), original_lease_id TEXT NOT NULL,
 connection_id TEXT NOT NULL, connection_epoch BIGINT NOT NULL CHECK(connection_epoch>0),
 binding_sha256 TEXT NOT NULL, checkpoint_sha256 TEXT NOT NULL,
 source_version_id TEXT NOT NULL REFERENCES android_private_loop_versions(id),
 receipt_version_id TEXT REFERENCES android_private_loop_versions(id),
 state TEXT NOT NULL CHECK(state IN('prepared','closed')), created_at BIGINT NOT NULL,
 PRIMARY KEY(run_id,entity_id,session_revision)
);
CREATE TABLE IF NOT EXISTS offline_mcp_calls (
 id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES offline_local_runs(id),
 grant_id TEXT NOT NULL REFERENCES offline_local_grants(id), device_id TEXT NOT NULL,
 tool_id TEXT NOT NULL, tool_request_sha256 TEXT NOT NULL, configuration_sha256 TEXT NOT NULL,
 connection_id TEXT NOT NULL, connection_epoch BIGINT NOT NULL CHECK(connection_epoch>0),
 original_rpc_id TEXT NOT NULL, transport_request_sha256 TEXT NOT NULL,
 scope TEXT NOT NULL, sealed_source TEXT NOT NULL, source_sha256 TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN('prepared','admitted','received','unknown')),
 dispatch_admitted BIGINT NOT NULL CHECK(dispatch_admitted IN(0,1)),
 task_record_id TEXT REFERENCES mcp_async_tasks(id),
 sealed_receipt TEXT, receipt_scope TEXT, receipt_sha256 TEXT,
 created_at BIGINT NOT NULL, updated_at BIGINT NOT NULL,
 CHECK((sealed_receipt IS NULL AND receipt_scope IS NULL AND receipt_sha256 IS NULL) OR
       (sealed_receipt IS NOT NULL AND receipt_scope IS NOT NULL AND receipt_sha256 IS NOT NULL)),
 CHECK(state<>'prepared' OR (dispatch_admitted=0 AND sealed_receipt IS NULL AND task_record_id IS NULL)),
 CHECK(state NOT IN('admitted','received') OR dispatch_admitted=1),
 CHECK(state<>'received' OR sealed_receipt IS NOT NULL),
 CHECK(task_record_id IS NULL OR (dispatch_admitted=1 AND sealed_receipt IS NOT NULL)),
 UNIQUE(run_id,tool_id), UNIQUE(run_id,tool_request_sha256)
);
