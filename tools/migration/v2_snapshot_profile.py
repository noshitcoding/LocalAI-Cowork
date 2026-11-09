"""Reviewed exact V2 SQLite schema profile; no SQL is extracted or executed at runtime.

The immutable embedded SQL and complete source-file byte hashes bind this
adapter to central storage migrations 1..20 and identity/tenant/accounting DDL.
A changed source binding or database object requires a new reviewed profile.
"""
from __future__ import annotations
import contextlib
import sqlite3
from pathlib import Path
import v1_sqlite_export as v1
from migration_io import Refused
PROFILE = 'v2-storage-20-identity-1-tenant-1-money-ddl-1'
SOURCE_BINDINGS = {'crates/identity/src/migrations.rs': '95bb0a1a380f96a5b9de3fe96fd9d7041cee0ccbc4aeda7f645bd24f68f3478b',
 'crates/model-accounting/src/persistence.rs': '658e7f14b50605253459870beb93829ddac9192a813a5013ccb408ab1657a074',
 'crates/storage/migrations/0001.sql': 'a5530e762d1e976d5ec8dddd538e4e6f4bd921bebfff24c43203724906e73884',
 'crates/storage/migrations/0002.sql': '2952e7dbea02ead35d554b70177dbeee359f1482716974345cc49886c0a28342',
 'crates/storage/migrations/0003.sql': '6e0520d1068f1aca75905f73b1d8ee90ccf9ad02a9953d8b67ac3e755ed53a55',
 'crates/storage/migrations/0004.sql': '040d2e505063b0b69e63557856705588a129887460569c6a2791839f485b0ca3',
 'crates/storage/migrations/0005.sql': 'ff74208a762c5bfb3250eed516f0afc6ef8377f7a415a520d8edf95638794720',
 'crates/storage/migrations/0006.sql': '740e387d4ee93870cfeddd79634d43641b82b9672c313634945ae939beb47c6c',
 'crates/storage/migrations/0007.sql': '8f6b2956067d4e96adc177d4e6e9e9addeda3a269e076fb297daf443de2b4226',
 'crates/storage/migrations/0008.sql': '1bcfe93dfdfa0375023337d23db963e4d286c6551ebd4dce48f75a7f882c802f',
 'crates/storage/migrations/0009.sql': '2b45a91b97dcee91fd5fa49522812a83d3b7d40509bc4f2c41eefb0ba2602e0c',
 'crates/storage/migrations/0010.sql': '968449987bf3a3843a4f63cc8bb850de254a33bc2907947e43bfa03d893ae238',
 'crates/storage/migrations/0011.sql': 'b8334d03b6405d9ac282ef3db396254557ae908991a61db92e4679b42e9a8970',
 'crates/storage/migrations/0012.sql': '55fa7d525e6900697f9e270ee97783e2e578083d19bdb6f26fd05c5f597f382d',
 'crates/storage/migrations/0013.sql': 'c8bfc4f3c0ed659351d1f66b1f184574c80652243bed3f947c9f6a1a8372a478',
 'crates/storage/migrations/0014.sql': 'e3fbc1e706c4c825a9ed8ba9169630fbb4bc644282d210aa0b1cd8cf785ffc6d',
 'crates/storage/migrations/0015.sql': '6a6c561f40b3dbd363a1b536e4104c5d87e3fc7c9e83d30b281beb143f943197',
 'crates/storage/migrations/0016.sql': '715ad5b3fa1eb03245b76d5cd4bcbc86d41908c16b29597b338df648b1917017',
 'crates/storage/migrations/0017.sql': 'c8323af4cfde6ca15ff27065b183811965bb2c712ecbcabb0c64a13b2ffe4f84',
 'crates/storage/migrations/0018.sql': 'fb827300debef3172ab18b7a0065dea9f0e3841bd462a02c0acbf6e3339847cb',
 'crates/storage/migrations/0019.sql': '402d7852a6c106eee9b31e370de21d2883843c46a75d68e039d5ec17d5ebcd97',
 'crates/storage/migrations/0020.sql': '8d6b76478c694d57df26be4d6d2b6b043445ce3b7c724b33303b55aeb7f6a02c',
 'crates/storage/src/migrations.rs': '9e24d8838971bb66ca4678a40370e808264b9d3ad8b80f86db23a28071eca5a8',
 'crates/storage/src/model_money.rs': '669776150eb5a9b1e09b24d13cd9e974ed1b42f7ce2aabaa92416de72fb3228e',
 'crates/storage/src/tenant_projects.rs': '005c22fcb73069f18eff0f909eac25121eccf8f5cd29e0653c61c0678e0e789e'}

CENTRAL = [(1,
  'CREATE TABLE IF NOT EXISTS schema_migrations (version BIGINT PRIMARY KEY, checksum TEXT NOT NULL);\n'
  'CREATE TABLE IF NOT EXISTS projects (id TEXT PRIMARY KEY, payload TEXT NOT NULL);\n'
  'CREATE TABLE IF NOT EXISTS providers (id TEXT PRIMARY KEY, payload TEXT NOT NULL);\n'
  'CREATE TABLE IF NOT EXISTS orders (id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id), '
  'revision BIGINT NOT NULL, payload TEXT NOT NULL);\n'
  'CREATE TABLE IF NOT EXISTS runs (id TEXT PRIMARY KEY, order_id TEXT NOT NULL REFERENCES orders(id), '
  'idempotency_key TEXT NOT NULL, request_hash TEXT NOT NULL, revision BIGINT NOT NULL, state TEXT NOT NULL, '
  'payload TEXT NOT NULL, UNIQUE(order_id,idempotency_key));\n'
  'CREATE TABLE IF NOT EXISTS events (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), '
  'sequence BIGINT NOT NULL, payload TEXT NOT NULL, UNIQUE(run_id,sequence));\n'
  'CREATE TABLE IF NOT EXISTS messages (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), '
  'order_id TEXT NOT NULL REFERENCES orders(id), created_at BIGINT NOT NULL, payload TEXT NOT NULL);\n'
  'CREATE TABLE IF NOT EXISTS artifacts (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), '
  'order_id TEXT NOT NULL REFERENCES orders(id), payload TEXT NOT NULL);\n'
  'CREATE INDEX IF NOT EXISTS orders_project ON orders(project_id);\n'
  'CREATE INDEX IF NOT EXISTS runs_state ON runs(state);\n'
  'CREATE INDEX IF NOT EXISTS messages_order ON messages(order_id,created_at);\n'
  'CREATE INDEX IF NOT EXISTS artifacts_order ON artifacts(order_id);\n',
  'a5530e762d1e976d5ec8dddd538e4e6f4bd921bebfff24c43203724906e73884'),
 (2,
  'ALTER TABLE messages ADD COLUMN position BIGINT NOT NULL DEFAULT 0;\n'
  'ALTER TABLE runs ADD COLUMN updated_at BIGINT NOT NULL DEFAULT 0;\n'
  'CREATE TABLE order_history (order_id TEXT PRIMARY KEY REFERENCES orders(id), source_order_id TEXT NOT '
  'NULL REFERENCES orders(id), idempotency_key TEXT NOT NULL, request_hash TEXT NOT NULL, payload TEXT NOT '
  'NULL, UNIQUE(source_order_id,idempotency_key));\n'
  'CREATE TABLE search_documents (id TEXT PRIMARY KEY, order_id TEXT NOT NULL REFERENCES orders(id), kind '
  'TEXT NOT NULL, original TEXT NOT NULL, normalized TEXT NOT NULL, created_at BIGINT NOT NULL);\n'
  'CREATE INDEX search_documents_order ON search_documents(order_id);\n'
  'CREATE INDEX search_documents_created ON search_documents(created_at,id);\n'
  'CREATE INDEX runs_activity ON runs(state,updated_at,id);\n',
  '2952e7dbea02ead35d554b70177dbeee359f1482716974345cc49886c0a28342'),
 (3,
  'CREATE TABLE file_changes (run_id TEXT NOT NULL REFERENCES runs(id), tool_call_id TEXT NOT NULL, payload '
  'TEXT NOT NULL, PRIMARY KEY(run_id,tool_call_id));\n'
  'CREATE TABLE folder_owners (resource_id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id));\n',
  '6e0520d1068f1aca75905f73b1d8ee90ccf9ad02a9953d8b67ac3e755ed53a55'),
 (4,
  'CREATE TABLE workspaces (order_id TEXT PRIMARY KEY REFERENCES orders(id), payload TEXT NOT NULL);\n'
  'CREATE TABLE workspace_snapshots (run_id TEXT PRIMARY KEY REFERENCES runs(id), order_id TEXT NOT NULL '
  'REFERENCES orders(id), payload TEXT NOT NULL);\n'
  'CREATE INDEX workspace_snapshots_order ON workspace_snapshots(order_id);\n',
  '040d2e505063b0b69e63557856705588a129887460569c6a2791839f485b0ca3'),
 (5,
  'CREATE TABLE file_operations (run_id TEXT NOT NULL REFERENCES runs(id), tool_call_id TEXT NOT NULL, '
  'payload TEXT NOT NULL, PRIMARY KEY (run_id,tool_call_id));\n',
  'ff74208a762c5bfb3250eed516f0afc6ef8377f7a415a520d8edf95638794720'),
 (6,
  "ALTER TABLE runs ADD COLUMN execution_kind TEXT NOT NULL DEFAULT 'agent' CHECK (execution_kind IN "
  "('agent','file_action'));\n"
  'CREATE INDEX runs_execution_kind_state ON runs(execution_kind,state);\n',
  '740e387d4ee93870cfeddd79634d43641b82b9672c313634945ae939beb47c6c'),
 (7,
  'CREATE TABLE file_uploads (\n'
  '    id TEXT PRIMARY KEY,\n'
  '    order_id TEXT NOT NULL REFERENCES orders(id),\n'
  '    idempotency_key TEXT NOT NULL,\n'
  '    request_hash TEXT NOT NULL,\n'
  '    revision BIGINT NOT NULL,\n'
  '    created_at BIGINT NOT NULL,\n'
  '    payload TEXT NOT NULL,\n'
  '    UNIQUE(order_id, idempotency_key)\n'
  ');\n'
  'CREATE INDEX file_uploads_order ON file_uploads(order_id, created_at, id);\n',
  '8f6b2956067d4e96adc177d4e6e9e9addeda3a269e076fb297daf443de2b4226'),
 (8,
  'CREATE TABLE native_authority_schema_guard (version BIGINT PRIMARY KEY);\n'
  'INSERT INTO native_authority_schema_guard(version) VALUES(1);\n',
  '1bcfe93dfdfa0375023337d23db963e4d286c6551ebd4dce48f75a7f882c802f'),
 (9,
  'CREATE TABLE model_accounting_schema_guard (version BIGINT PRIMARY KEY);\n'
  'INSERT INTO model_accounting_schema_guard(version) VALUES(1);\n',
  '2b45a91b97dcee91fd5fa49522812a83d3b7d40509bc4f2c41eefb0ba2602e0c'),
 (10,
  'CREATE TABLE environments (\n'
  '    id TEXT PRIMARY KEY,\n'
  '    project_id TEXT NOT NULL REFERENCES projects(id),\n'
  '    idempotency_key TEXT NOT NULL,\n'
  '    request_hash TEXT NOT NULL,\n'
  '    revision BIGINT NOT NULL,\n'
  '    payload TEXT NOT NULL,\n'
  '    UNIQUE(project_id,idempotency_key)\n'
  ');\n',
  '968449987bf3a3843a4f63cc8bb850de254a33bc2907947e43bfa03d893ae238'),
 (11,
  'CREATE TABLE process_attempts (\n'
  '    id TEXT PRIMARY KEY,\n'
  '    run_id TEXT NOT NULL REFERENCES runs(id),\n'
  '    tool_call_id TEXT NOT NULL,\n'
  '    revision BIGINT NOT NULL,\n'
  '    payload TEXT NOT NULL,\n'
  '    UNIQUE(run_id,tool_call_id)\n'
  ');\n',
  'b8334d03b6405d9ac282ef3db396254557ae908991a61db92e4679b42e9a8970'),
 (12,
  'CREATE TABLE mcp_servers (\n'
  '    id TEXT PRIMARY KEY,\n'
  '    project_id TEXT NOT NULL REFERENCES projects(id),\n'
  '    revision BIGINT NOT NULL,\n'
  '    payload TEXT NOT NULL\n'
  ');\n'
  'CREATE TABLE mcp_server_requests (\n'
  '    project_id TEXT NOT NULL REFERENCES projects(id),\n'
  '    request_key TEXT NOT NULL,\n'
  '    request_sha256 TEXT NOT NULL,\n'
  '    payload TEXT NOT NULL,\n'
  '    PRIMARY KEY(project_id,request_key)\n'
  ');\n'
  'CREATE TABLE mcp_attempts (\n'
  '    id TEXT PRIMARY KEY,\n'
  '    run_id TEXT NOT NULL REFERENCES runs(id),\n'
  '    tool_call_id TEXT NOT NULL,\n'
  '    revision BIGINT NOT NULL,\n'
  '    payload TEXT NOT NULL,\n'
  '    UNIQUE(run_id,tool_call_id)\n'
  ');\n',
  '55fa7d525e6900697f9e270ee97783e2e578083d19bdb6f26fd05c5f597f382d'),
 (13,
  'CREATE TABLE mcp_connections (\n'
  '    id TEXT PRIMARY KEY,\n'
  '    run_id TEXT NOT NULL REFERENCES runs(id),\n'
  '    server_id TEXT NOT NULL REFERENCES mcp_servers(id),\n'
  '    revision BIGINT NOT NULL,\n'
  '    payload TEXT NOT NULL,\n'
  '    UNIQUE(run_id,server_id)\n'
  ');\n',
  'c8bfc4f3c0ed659351d1f66b1f184574c80652243bed3f947c9f6a1a8372a478'),
 (14,
  'CREATE TABLE mcp_credentials (\n'
  '    project_id TEXT NOT NULL REFERENCES projects(id),\n'
  '    reference TEXT NOT NULL,\n'
  '    revision BIGINT NOT NULL,\n'
  '    payload TEXT NOT NULL,\n'
  '    PRIMARY KEY(project_id, reference)\n'
  ');\n',
  'e3fbc1e706c4c825a9ed8ba9169630fbb4bc644282d210aa0b1cd8cf785ffc6d'),
 (15,
  'CREATE TABLE native_secret_state (\n'
  '    id TEXT PRIMARY KEY,\n'
  '    revision BIGINT NOT NULL,\n'
  '    payload TEXT NOT NULL\n'
  ');\n'
  'CREATE TABLE native_key_operations (\n'
  '    id TEXT PRIMARY KEY,\n'
  '    request_key TEXT UNIQUE NOT NULL,\n'
  '    request_sha256 TEXT NOT NULL,\n'
  '    payload TEXT NOT NULL\n'
  ');\n',
  '6a6c561f40b3dbd363a1b536e4104c5d87e3fc7c9e83d30b281beb143f943197'),
 (16,
  'CREATE TABLE workflows (\r\n'
  ' id TEXT PRIMARY KEY,\r\n'
  ' project_id TEXT NOT NULL REFERENCES projects(id),\r\n'
  ' name TEXT NOT NULL,\r\n'
  ' revision BIGINT NOT NULL CHECK(revision >= 1),\r\n'
  ' revoked BIGINT NOT NULL CHECK(revoked IN (0,1)),\r\n'
  ' request_key TEXT NOT NULL,\r\n'
  ' request_sha256 TEXT NOT NULL,\r\n'
  ' UNIQUE(project_id,request_key),\r\n'
  ' UNIQUE(id,project_id)\r\n'
  ');\r\n'
  'CREATE TABLE workflow_versions (\r\n'
  ' id TEXT PRIMARY KEY,\r\n'
  ' workflow_id TEXT NOT NULL,\r\n'
  ' project_id TEXT NOT NULL,\r\n'
  ' revision BIGINT NOT NULL,\r\n'
  ' name TEXT NOT NULL,\r\n'
  ' sha256 TEXT NOT NULL,\r\n'
  ' created_at BIGINT NOT NULL,\r\n'
  ' payload TEXT NOT NULL,\r\n'
  ' UNIQUE(workflow_id,revision),\r\n'
  ' UNIQUE(id,workflow_id,project_id),\r\n'
  ' FOREIGN KEY(workflow_id,project_id) REFERENCES workflows(id,project_id)\r\n'
  ');\r\n'
  'CREATE TABLE workflow_changes (\r\n'
  ' workflow_id TEXT NOT NULL REFERENCES workflows(id),\r\n'
  ' request_key TEXT NOT NULL,\r\n'
  ' request_sha256 TEXT NOT NULL,\r\n'
  ' revision BIGINT NOT NULL,\r\n'
  ' payload TEXT NOT NULL,\r\n'
  ' PRIMARY KEY(workflow_id,request_key),\r\n'
  ' UNIQUE(workflow_id,revision)\r\n'
  ');\r\n'
  'CREATE TABLE workflow_runs (\r\n'
  ' run_id TEXT PRIMARY KEY REFERENCES runs(id),\r\n'
  ' workflow_id TEXT NOT NULL,\r\n'
  ' version_id TEXT NOT NULL,\r\n'
  ' project_id TEXT NOT NULL,\r\n'
  ' revision BIGINT NOT NULL CHECK(revision >= 0),\r\n'
  ' plan_sha256 TEXT NOT NULL,\r\n'
  ' progress_sha256 TEXT NOT NULL,\r\n'
  ' payload TEXT NOT NULL,\r\n'
  ' FOREIGN KEY(version_id,workflow_id,project_id) REFERENCES '
  'workflow_versions(id,workflow_id,project_id)\r\n'
  ');\r\n',
  '715ad5b3fa1eb03245b76d5cd4bcbc86d41908c16b29597b338df648b1917017'),
 (17,
  'CREATE TABLE workflow_attention (\r\n'
  ' run_id TEXT PRIMARY KEY REFERENCES workflow_runs(run_id),\r\n'
  ' run_revision BIGINT NOT NULL CHECK(run_revision >= 1),\r\n'
  ' workflow_revision BIGINT NOT NULL CHECK(workflow_revision >= 0),\r\n'
  ' actionable_count BIGINT NOT NULL CHECK(actionable_count >= 0),\r\n'
  ' valid_until BIGINT,\r\n'
  ' CHECK((actionable_count=0 AND valid_until IS NULL) OR (actionable_count>0 AND valid_until IS NOT '
  'NULL))\r\n'
  ');\r\n'
  'CREATE INDEX workflow_attention_actionable ON workflow_attention(actionable_count,valid_until);\r\n',
  'c8323af4cfde6ca15ff27065b183811965bb2c712ecbcabb0c64a13b2ffe4f84'),
 (18,
  'CREATE TABLE workflow_write_keys (\r\n'
  ' namespace TEXT NOT NULL,\r\n'
  ' target_id TEXT NOT NULL,\r\n'
  ' request_key TEXT NOT NULL,\r\n'
  ' project_id TEXT NOT NULL REFERENCES projects(id),\r\n'
  ' request_sha256 TEXT NOT NULL,\r\n'
  ' request_payload TEXT NOT NULL,\r\n'
  ' operation_id TEXT,\r\n'
  " state TEXT NOT NULL CHECK(state IN ('reserved','accepted','rejected','discarded')),\r\n"
  ' receipt TEXT,\r\n'
  ' PRIMARY KEY(namespace,target_id,request_key)\r\n'
  ');\r\n'
  'CREATE TABLE workflow_drafts (\r\n'
  ' id TEXT PRIMARY KEY,\r\n'
  ' project_id TEXT NOT NULL REFERENCES projects(id),\r\n'
  ' revision BIGINT NOT NULL CHECK(revision >= 1),\r\n'
  ' snapshot_id TEXT NOT NULL,\r\n'
  ' payload TEXT NOT NULL,\r\n'
  ' UNIQUE(id,project_id)\r\n'
  ');\r\n'
  'CREATE TABLE workflow_draft_snapshots (\r\n'
  ' id TEXT PRIMARY KEY,\r\n'
  ' draft_id TEXT NOT NULL,\r\n'
  ' project_id TEXT NOT NULL,\r\n'
  ' payload_bytes BIGINT NOT NULL CHECK(payload_bytes >= 0),\r\n'
  ' payload TEXT NOT NULL,\r\n'
  ' FOREIGN KEY(draft_id,project_id) REFERENCES workflow_drafts(id,project_id),\r\n'
  ' UNIQUE(id,draft_id,project_id)\r\n'
  ');\r\n'
  'CREATE TABLE workflow_draft_saves (\r\n'
  ' draft_id TEXT NOT NULL,\r\n'
  ' request_key TEXT NOT NULL,\r\n'
  ' request_sha256 TEXT NOT NULL,\r\n'
  ' payload TEXT NOT NULL,\r\n'
  ' PRIMARY KEY(draft_id,request_key),\r\n'
  ' FOREIGN KEY(draft_id) REFERENCES workflow_drafts(id)\r\n'
  ');\r\n'
  'CREATE TABLE workflow_operations (\r\n'
  ' id TEXT PRIMARY KEY,\r\n'
  ' project_id TEXT NOT NULL REFERENCES projects(id),\r\n'
  ' intent_key TEXT NOT NULL,\r\n'
  ' request_sha256 TEXT NOT NULL,\r\n'
  " state TEXT NOT NULL CHECK(state IN ('pending','accepted','rejected','discarded')),\r\n"
  ' acknowledged_at BIGINT,\r\n'
  ' payload TEXT NOT NULL,\r\n'
  ' UNIQUE(project_id,intent_key)\r\n'
  ');\r\n'
  'CREATE INDEX workflow_drafts_project ON workflow_drafts(project_id,id);\r\n'
  'CREATE INDEX workflow_operations_project ON workflow_operations(project_id,id);\r\n'
  'CREATE TABLE workflow_journal_quotas (\r\n'
  ' project_id TEXT PRIMARY KEY REFERENCES projects(id),\r\n'
  ' drafts BIGINT NOT NULL DEFAULT 0 CHECK(drafts >= 0 AND drafts <= 32),\r\n'
  ' snapshots BIGINT NOT NULL DEFAULT 0 CHECK(snapshots >= 0 AND snapshots <= 4096),\r\n'
  ' snapshot_bytes BIGINT NOT NULL DEFAULT 0 CHECK(snapshot_bytes >= 0 AND snapshot_bytes <= 33554432),\r\n'
  ' operations BIGINT NOT NULL DEFAULT 0 CHECK(operations >= 0 AND operations <= 512)\r\n'
  ');\r\n',
  'fb827300debef3172ab18b7a0065dea9f0e3841bd462a02c0acbf6e3339847cb'),
 (19,
  'CREATE TABLE workflow_input_drafts (\n'
  ' run_id TEXT NOT NULL REFERENCES runs(id), request_id TEXT NOT NULL, owner TEXT NOT NULL,\n'
  ' project_id TEXT NOT NULL REFERENCES projects(id), revision BIGINT NOT NULL, snapshot_id TEXT NOT NULL, '
  'payload TEXT NOT NULL,\n'
  ' PRIMARY KEY(run_id,request_id,owner)\n'
  ');\n'
  'CREATE TABLE workflow_input_draft_snapshots (\n'
  ' id TEXT PRIMARY KEY, run_id TEXT NOT NULL, request_id TEXT NOT NULL, owner TEXT NOT NULL,\n'
  ' project_id TEXT NOT NULL REFERENCES projects(id), payload_bytes BIGINT NOT NULL, payload TEXT NOT NULL,\n'
  ' FOREIGN KEY(run_id,request_id,owner) REFERENCES workflow_input_drafts(run_id,request_id,owner)\n'
  ');\n'
  'CREATE TABLE workflow_input_draft_saves (\n'
  ' run_id TEXT NOT NULL, request_id TEXT NOT NULL, owner TEXT NOT NULL, request_key TEXT NOT NULL,\n'
  ' request_sha256 TEXT NOT NULL, payload TEXT NOT NULL,\n'
  ' PRIMARY KEY(run_id,request_id,owner,request_key),\n'
  ' FOREIGN KEY(run_id,request_id,owner) REFERENCES workflow_input_drafts(run_id,request_id,owner)\n'
  ');\n'
  'CREATE TABLE workflow_input_answer_intents (\n'
  ' id TEXT PRIMARY KEY, run_id TEXT NOT NULL, request_id TEXT NOT NULL, owner TEXT NOT NULL,\n'
  ' project_id TEXT NOT NULL REFERENCES projects(id), request_key TEXT NOT NULL,\n'
  ' prepare_sha256 TEXT NOT NULL, request_sha256 TEXT NOT NULL, state TEXT NOT NULL, payload TEXT NOT NULL,\n'
  ' UNIQUE(run_id,owner,request_key),\n'
  ' FOREIGN KEY(run_id,request_id,owner) REFERENCES workflow_input_drafts(run_id,request_id,owner)\n'
  ');\n'
  'CREATE INDEX workflow_input_intent_scope ON workflow_input_answer_intents(run_id,request_id,owner);\n'
  'CREATE TABLE workflow_input_quotas (\n'
  ' project_id TEXT PRIMARY KEY REFERENCES projects(id), heads BIGINT NOT NULL DEFAULT 0,\n'
  ' snapshots BIGINT NOT NULL DEFAULT 0, snapshot_bytes BIGINT NOT NULL DEFAULT 0, intents BIGINT NOT NULL '
  'DEFAULT 0\n'
  ');\r\n',
  '402d7852a6c106eee9b31e370de21d2883843c46a75d68e039d5ec17d5ebcd97'),
 (20,
  'CREATE TABLE workflow_execution_clocks (\n'
  ' run_id TEXT PRIMARY KEY REFERENCES runs(id), revision BIGINT NOT NULL,\n'
  ' last_run_revision BIGINT NOT NULL, highwater_ms BIGINT NOT NULL, payload TEXT NOT NULL\n'
  ');\n',
  '8d6b76478c694d57df26be4d6d2b6b043445ce3b7c724b33303b55aeb7f6a02c')]

EXTRA_DDL = ['CREATE TABLE IF NOT EXISTS identity_schema_guard (version BIGINT PRIMARY KEY CHECK(version = 1))',
 'CREATE TABLE IF NOT EXISTS identity_installation (id BIGINT PRIMARY KEY CHECK(id = 1), bootstrap_sha256 '
 'TEXT NOT NULL, settlement_sha256 TEXT NOT NULL, bootstrapped BIGINT NOT NULL CHECK(bootstrapped IN (0,1)), '
 'utc_highwater BIGINT NOT NULL, revision BIGINT NOT NULL CHECK(revision > 0))',
 'CREATE TABLE IF NOT EXISTS identity_subjects (id TEXT PRIMARY KEY, username TEXT NOT NULL UNIQUE, '
 'display_name TEXT NOT NULL, active BIGINT NOT NULL CHECK(active IN (0,1)), platform_admin BIGINT NOT NULL '
 'CHECK(platform_admin IN (0,1)), revision BIGINT NOT NULL CHECK(revision > 0), password_salt TEXT NOT NULL, '
 'password_hash TEXT NOT NULL)',
 'CREATE TABLE IF NOT EXISTS identity_sessions (id TEXT PRIMARY KEY, identity_id TEXT NOT NULL REFERENCES '
 'identity_subjects(id), token_sha256 TEXT NOT NULL UNIQUE, expires_at BIGINT NOT NULL, revoked BIGINT NOT '
 'NULL CHECK(revoked IN (0,1)), revision BIGINT NOT NULL CHECK(revision > 0))',
 'CREATE TABLE IF NOT EXISTS identity_organizations (id TEXT PRIMARY KEY, name TEXT NOT NULL, state TEXT NOT '
 "NULL CHECK(state IN ('active','suspended','closed')), revision BIGINT NOT NULL CHECK(revision > 0), "
 'authorization_epoch BIGINT NOT NULL CHECK(authorization_epoch > 0))',
 'CREATE TABLE IF NOT EXISTS identity_memberships (id TEXT PRIMARY KEY, organization_id TEXT NOT NULL '
 'REFERENCES identity_organizations(id), identity_id TEXT NOT NULL REFERENCES identity_subjects(id), role '
 "TEXT NOT NULL CHECK(role IN ('owner','admin','member','viewer')), active BIGINT NOT NULL CHECK(active IN "
 '(0,1)), revision BIGINT NOT NULL CHECK(revision > 0))',
 'CREATE UNIQUE INDEX IF NOT EXISTS identity_live_membership ON '
 'identity_memberships(organization_id,identity_id) WHERE active = 1',
 'CREATE TABLE IF NOT EXISTS identity_resources (id TEXT PRIMARY KEY, organization_id TEXT NOT NULL '
 'REFERENCES identity_organizations(id), kind TEXT NOT NULL CHECK(kind IN '
 "('project','order','artifact','workflow','knowledge')), revision BIGINT NOT NULL CHECK(revision > 0))",
 'CREATE TABLE IF NOT EXISTS identity_content_grants (resource_id TEXT NOT NULL REFERENCES '
 'identity_resources(id), membership_id TEXT NOT NULL REFERENCES identity_memberships(id), permissions '
 'BIGINT NOT NULL CHECK(permissions > 0 AND permissions <= 15), active BIGINT NOT NULL CHECK(active IN '
 '(0,1)), revision BIGINT NOT NULL CHECK(revision > 0), PRIMARY KEY(resource_id,membership_id))',
 'CREATE TABLE IF NOT EXISTS identity_quotas (scope TEXT PRIMARY KEY, revision BIGINT NOT NULL '
 'CHECK(revision > 0), limit_payload TEXT NOT NULL, held_payload TEXT NOT NULL, consumed_payload TEXT NOT '
 'NULL)',
 'CREATE TABLE IF NOT EXISTS identity_reservations (id TEXT PRIMARY KEY, organization_id TEXT NOT NULL '
 'REFERENCES identity_organizations(id), identity_id TEXT NOT NULL REFERENCES identity_subjects(id), '
 'intent_key TEXT NOT NULL, request_sha256 TEXT NOT NULL, payload TEXT NOT NULL, initial_receipt TEXT NOT '
 'NULL, content_permit TEXT NOT NULL, UNIQUE(organization_id,identity_id,intent_key))',
 'CREATE TABLE IF NOT EXISTS identity_operations (actor_id TEXT NOT NULL REFERENCES identity_subjects(id), '
 'organization_id TEXT NOT NULL, intent_key TEXT NOT NULL, request_sha256 TEXT NOT NULL, receipt TEXT NOT '
 'NULL, PRIMARY KEY(actor_id,organization_id,intent_key))',
 'CREATE TABLE IF NOT EXISTS identity_audit (id TEXT PRIMARY KEY, sequence BIGINT NOT NULL UNIQUE '
 'CHECK(sequence > 0), organization_id TEXT, actor_id TEXT NOT NULL REFERENCES identity_subjects(id), action '
 'TEXT NOT NULL, target_id TEXT NOT NULL, observed_at BIGINT NOT NULL)',
 'CREATE TABLE IF NOT EXISTS accounting_ledgers (id TEXT PRIMARY KEY, format_version BIGINT NOT NULL, '
 'revision BIGINT NOT NULL, payload_sha256 TEXT NOT NULL, payload TEXT NOT NULL)',
 'CREATE TABLE IF NOT EXISTS accounting_operations (id TEXT PRIMARY KEY, ledger_id TEXT NOT NULL, '
 'format_version BIGINT NOT NULL, kind TEXT NOT NULL, request_sha256 TEXT NOT NULL, accepted_ledger_revision '
 'BIGINT NOT NULL, payload_sha256 TEXT NOT NULL, payload TEXT NOT NULL)',
 'CREATE TABLE IF NOT EXISTS tenant_project_schema_guard (version BIGINT PRIMARY KEY CHECK(version = 1))',
 'CREATE TABLE IF NOT EXISTS tenant_projects (id TEXT PRIMARY KEY REFERENCES identity_resources(id), '
 'organization_id TEXT NOT NULL REFERENCES identity_organizations(id), creator_identity_id TEXT NOT NULL '
 'REFERENCES identity_subjects(id), creation_key TEXT NOT NULL, request_sha256 TEXT NOT NULL, '
 'creation_payload TEXT NOT NULL, revision BIGINT NOT NULL CHECK(revision > 0), payload TEXT NOT NULL, '
 'initial_receipt TEXT NOT NULL, UNIQUE(organization_id,creator_identity_id,creation_key))',
 'CREATE INDEX IF NOT EXISTS tenant_project_organization ON tenant_projects(organization_id,id)',
 'CREATE TABLE IF NOT EXISTS accounting_personal_installation (id TEXT PRIMARY KEY, payload_sha256 TEXT NOT '
 'NULL, payload TEXT NOT NULL)',
 'CREATE TABLE IF NOT EXISTS accounting_provider_deployments (id TEXT PRIMARY KEY, payload_sha256 TEXT NOT '
 'NULL, payload TEXT NOT NULL)',
 'CREATE TABLE IF NOT EXISTS accounting_personal_runs (id TEXT PRIMARY KEY, payload_sha256 TEXT NOT NULL, '
 'payload TEXT NOT NULL)',
 'CREATE TABLE IF NOT EXISTS accounting_model_bindings (id TEXT PRIMARY KEY, payload_sha256 TEXT NOT NULL, '
 'payload TEXT NOT NULL)']

def source_paths(root=None):
    if root is None:
        bundled = Path(__file__).parent / 'reference_source'
        # Fixed historical bytes must not follow later changes to live product
        # code. Old archived source trees without a bundle retain their fallback.
        root = bundled if bundled.is_dir() else Path(__file__).resolve().parents[2]
    root = Path(root)
    if not root.is_absolute() or '..' in root.parts:
        raise Refused('Absolute original historical profile source root required')
    return {root / relative: expected for relative, expected in SOURCE_BINDINGS.items()}


def reference():
    conn = sqlite3.connect(':memory:')
    try:
        conn.execute('PRAGMA foreign_keys=ON')
        for version, sql, checksum in CENTRAL:
            for statement in sql.split(';'):
                if statement.strip(): conn.execute(statement)
            if version == 2:
                conn.execute('CREATE UNIQUE INDEX messages_position ON messages(order_id,position)')
            conn.execute('INSERT INTO schema_migrations VALUES(?,?)', (version,checksum))
        for sql in EXTRA_DDL: conn.execute(sql)
        conn.execute('INSERT INTO identity_schema_guard VALUES(1)')
        conn.execute('INSERT INTO tenant_project_schema_guard VALUES(1)')
        conn.commit()
        return conn
    except BaseException:
        conn.close()
        raise


def inspect(conn):
    with contextlib.closing(reference()) as expected:
        if v1.signature(v1.schema(conn)) != v1.signature(v1.schema(expected)):
            raise Refused('Unknown or changed complete V2 schema profile')
    actual = conn.execute('SELECT version,checksum FROM schema_migrations ORDER BY version').fetchall()
    if actual != [(v,sha) for v,_,sha in CENTRAL]:
        raise Refused('V2 migration sequence or byte checksums differ')
    for table in ('identity_schema_guard','tenant_project_schema_guard',
                  'native_authority_schema_guard','model_accounting_schema_guard'):
        rows = conn.execute('SELECT version,typeof(version) FROM '+table).fetchall()
        if rows != [(1,'integer')]: raise Refused('Unknown V2 guarded schema version')
    for name in ('user_version','application_id'):
        if conn.execute('PRAGMA '+name).fetchone()[0] != 0:
            raise Refused('Unknown V2 SQLite metadata version')
    if conn.execute('PRAGMA integrity_check(1)').fetchall() != [('ok',)]:
        raise Refused('V2 integrity check failed')
    if conn.execute('PRAGMA foreign_key_check').fetchone() is not None:
        raise Refused('V2 snapshot foreign key violation')
    return {'profile':PROFILE, 'schema_sha256':v1.signature(v1.schema(conn)),
            'source_bindings':SOURCE_BINDINGS}
