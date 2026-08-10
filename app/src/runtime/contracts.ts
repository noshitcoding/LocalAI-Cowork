import { z } from 'zod'

export const API_VERSION = 'v1' as const
export const SCHEMA_VERSION = 2 as const
export const MIN_COMPATIBLE_SCHEMA_VERSION = 1 as const

export const capabilitySchema = z.string().trim().min(1).max(100)
export type Capability = z.infer<typeof capabilitySchema>

export const executorTargetSchema = z.discriminatedUnion('kind', [
  z.object({
    kind: z.literal('server_linux'),
    pool_id: z.string().uuid().nullable().optional(),
  }),
  z.object({
    kind: z.literal('managed_windows_pool'),
    pool_id: z.string().uuid(),
  }),
  z.object({
    kind: z.literal('personal_device'),
    device_id: z.string().uuid(),
  }),
])
export type ExecutorTarget = z.infer<typeof executorTargetSchema>

export const runStateSchema = z.enum([
  'queued',
  'waiting_for_executor',
  'waiting_for_snapshot',
  'running',
  'waiting_approval',
  'waiting_input',
  'interrupted',
  'completed',
  'failed',
  'canceled',
  'expired',
])
export type RunState = z.infer<typeof runStateSchema>

export const projectPrivacySchema = z.enum(['private_local', 'team_managed'])
export type ProjectPrivacy = z.infer<typeof projectPrivacySchema>

export const frozenReferenceSchema = z.object({
  id: z.string().uuid(),
  revision: z.number().int().min(1),
})

export const runErrorSchema = z.object({
  code: z.string(),
  message: z.string(),
  retryable: z.boolean().default(false),
  details: z.unknown().optional(),
})

export const runSpecSchema = z.object({
  schema_version: z.number().int(),
  id: z.string().uuid(),
  thread_id: z.string().uuid(),
  project_id: z.string().uuid(),
  project: frozenReferenceSchema,
  project_privacy: projectPrivacySchema,
  task: frozenReferenceSchema.nullable().optional(),
  creator_user_id: z.string().uuid(),
  executor_target: executorTargetSchema,
  required_capabilities: z.array(capabilitySchema),
  input: z.unknown(),
  model_profile_id: z.string().uuid().nullable().optional(),
  snapshot_id: z.string().uuid().nullable().optional(),
  idempotency_key: z.string(),
  created_at: z.string().datetime({ offset: true }),
})

export const runRecordSchema = z.object({
  spec: runSpecSchema,
  state: runStateSchema,
  revision: z.number().int().min(1),
  etag: z.string(),
  assigned_executor_id: z.string().uuid().nullable().optional(),
  lease_expires_at: z.string().datetime({ offset: true }).nullable().optional(),
  started_at: z.string().datetime({ offset: true }).nullable().optional(),
  finished_at: z.string().datetime({ offset: true }).nullable().optional(),
  result: z.unknown().nullable().optional(),
  error: runErrorSchema.nullable().optional(),
  updated_at: z.string().datetime({ offset: true }),
})
export type RunRecord = z.infer<typeof runRecordSchema>

export const runEventKindSchema = z.enum([
  'created',
  'state_changed',
  'model_started',
  'model_delta',
  'model_completed',
  'tool_started',
  'tool_completed',
  'tool_failed',
  'checkpoint_created',
  'approval_requested',
  'approval_resolved',
  'input_requested',
  'input_received',
  'artifact_created',
  'desktop_session_changed',
  'executor_assigned',
  'executor_heartbeat',
  'warning',
  'failed',
  'completed',
])

export const runEventSchema = z.object({
  schema_version: z.number().int(),
  run_id: z.string().uuid(),
  sequence: z.number().int().positive(),
  event_id: z.string().uuid(),
  kind: runEventKindSchema,
  payload: z.unknown(),
  created_at: z.string().datetime({ offset: true }),
})
export type RunEvent = z.infer<typeof runEventSchema>

export const runArtifactSchema = z.object({
  schema_version: z.number().int(),
  id: z.string().uuid(),
  run_id: z.string().uuid(),
  revision: z.number().int().positive(),
  kind: z.string(),
  media_type: z.string(),
  name: z.string(),
  digest: z.string().regex(/^[0-9a-f]{64}$/),
  size_bytes: z.number().int().nonnegative(),
  metadata: z.unknown(),
  created_at: z.string().datetime({ offset: true }),
  deleted_at: z.string().datetime({ offset: true }).nullable().optional(),
})
export type RunArtifact = z.infer<typeof runArtifactSchema>

export const pushConfigurationSchema = z.object({
  schema_version: z.number().int(),
  fcm_enabled: z.boolean(),
  web_push_public_key: z.string().nullable(),
})
export type PushConfiguration = z.infer<typeof pushConfigurationSchema>

export const pushSubscriptionRecordSchema = z.object({
  schema_version: z.number().int(),
  id: z.string().uuid(),
  device_id: z.string().uuid(),
  provider: z.enum(['fcm', 'web_push']),
  created_at: z.string().datetime({ offset: true }),
  last_success_at: z.string().datetime({ offset: true }).nullable(),
})
export type PushSubscriptionRecord = z.infer<typeof pushSubscriptionRecordSchema>

export const desktopSessionStateSchema = z.enum([
  'starting',
  'agent_controlled',
  'user_controlled',
  'paused',
  'ended',
  'failed',
])

export const desktopSessionSchema = z.object({
  schema_version: z.number().int(),
  id: z.string().uuid(),
  run_id: z.string().uuid(),
  executor_id: z.string().uuid(),
  state: desktopSessionStateSchema,
  stream_protocol: z.literal('rfb.binary.v1'),
  dimensions: z.object({
    width: z.number().int().min(640).max(3840),
    height: z.number().int().min(480).max(2160),
    scale_factor: z.number().positive(),
  }).nullable().optional(),
  controller_user_id: z.string().uuid().nullable().optional(),
  created_at: z.string().datetime({ offset: true }),
  ended_at: z.string().datetime({ offset: true }).nullable().optional(),
})
export type DesktopSession = z.infer<typeof desktopSessionSchema>

export const reauthenticationGrantSchema = z.object({
  schema_version: z.number().int(),
  token: z.string().min(32),
  purpose: z.literal('desktop_control'),
  expires_at: z.string().datetime({ offset: true }),
})
export type ReauthenticationGrant = z.infer<typeof reauthenticationGrantSchema>

export const desktopStreamTicketSchema = z.object({
  schema_version: z.number().int(),
  token: z.string().min(32),
  control: z.boolean(),
  expires_at: z.string().datetime({ offset: true }),
})
export type DesktopStreamTicket = z.infer<typeof desktopStreamTicketSchema>

export const terminalSessionTicketSchema = z.object({
  schema_version: z.number().int(),
  session_id: z.string().uuid(),
  token: z.string().min(32),
  expires_at: z.string().datetime({ offset: true }),
  protocol: z.literal('terminal.binary.v1'),
})
export type TerminalSessionTicket = z.infer<typeof terminalSessionTicketSchema>

export const approvalRequestSchema = z.object({
  schema_version: z.number().int(),
  id: z.string().uuid(),
  run_id: z.string().uuid(),
  revision: z.number().int().positive(),
  etag: z.string(),
  requested_action: z.unknown(),
  state: z.enum(['pending', 'approved', 'rejected', 'expired']),
  requested_at: z.string().datetime({ offset: true }),
  expires_at: z.string().datetime({ offset: true }),
  resolved_by_user_id: z.string().uuid().nullable(),
  resolved_at: z.string().datetime({ offset: true }).nullable(),
})
export type ApprovalRequest = z.infer<typeof approvalRequestSchema>

export const runInputRequestSchema = z.object({
  schema_version: z.number().int(),
  id: z.string().uuid(),
  run_id: z.string().uuid(),
  revision: z.number().int().positive(),
  etag: z.string(),
  prompt: z.unknown(),
  state: z.enum(['pending', 'submitted', 'expired']),
  response: z.unknown().nullable(),
  requested_at: z.string().datetime({ offset: true }),
  expires_at: z.string().datetime({ offset: true }),
  responded_by_user_id: z.string().uuid().nullable(),
  responded_at: z.string().datetime({ offset: true }).nullable(),
})
export type RunInputRequest = z.infer<typeof runInputRequestSchema>

export const projectRecordSchema = z.object({
  schema_version: z.number().int(),
  id: z.string().uuid(),
  revision: z.number().int().positive(),
  etag: z.string(),
  owner_user_id: z.string().uuid(),
  team_id: z.string().uuid().nullable(),
  privacy: projectPrivacySchema,
  name: z.string(),
  description: z.string(),
  preferred_executor_target: executorTargetSchema.nullable(),
  current_version_id: z.string().uuid().nullable(),
  policy: z.unknown(),
  created_at: z.string().datetime({ offset: true }),
  updated_at: z.string().datetime({ offset: true }),
  deleted_at: z.string().datetime({ offset: true }).nullable(),
})
export type ProjectRecord = z.infer<typeof projectRecordSchema>

export const threadRecordSchema = z.object({
  schema_version: z.number().int(),
  id: z.string().uuid(),
  revision: z.number().int().positive(),
  etag: z.string(),
  project_id: z.string().uuid(),
  forked_from_thread_id: z.string().uuid().nullable(),
  forked_from_message_id: z.string().uuid().nullable(),
  title: z.string(),
  deleted_at: z.string().datetime({ offset: true }).nullable(),
})
export type ThreadRecord = z.infer<typeof threadRecordSchema>

export const messageRoleSchema = z.enum(['user', 'assistant', 'system', 'tool'])
export type MessageRole = z.infer<typeof messageRoleSchema>

export const messageRecordSchema = z.object({
  schema_version: z.number().int(),
  id: z.string().uuid(),
  revision: z.number().int().positive(),
  etag: z.string(),
  thread_id: z.string().uuid(),
  author_user_id: z.string().uuid().nullable(),
  role: messageRoleSchema,
  content: z.unknown(),
  run_id: z.string().uuid().nullable(),
  created_at: z.string().datetime({ offset: true }),
  updated_at: z.string().datetime({ offset: true }),
  deleted_at: z.string().datetime({ offset: true }).nullable(),
})
export type MessageRecord = z.infer<typeof messageRecordSchema>

export const threadMessageRunSchema = z.object({
  schema_version: z.number().int(),
  message: messageRecordSchema,
  run: runRecordSchema,
})
export type ThreadMessageRun = z.infer<typeof threadMessageRunSchema>

export const supportGrantRecordSchema = z.object({
  schema_version: z.number().int(),
  id: z.string().uuid(),
  granted_by: z.string().uuid(),
  support_user_id: z.string().uuid(),
  project_id: z.string().uuid().nullable(),
  thread_id: z.string().uuid().nullable(),
  reason: z.string(),
  created_at: z.string().datetime({ offset: true }),
  expires_at: z.string().datetime({ offset: true }),
  revoked_at: z.string().datetime({ offset: true }).nullable(),
})
export type SupportGrantRecord = z.infer<typeof supportGrantRecordSchema>

export const quotaScopeTypeSchema = z.enum(['user', 'team'])
export type QuotaScopeType = z.infer<typeof quotaScopeTypeSchema>

export const quotaLimitsRecordSchema = z.object({
  schema_version: z.number().int(),
  scope_type: quotaScopeTypeSchema,
  scope_id: z.string().uuid(),
  storage_bytes: z.number().int().nonnegative().nullable(),
  concurrent_runs: z.number().int().nonnegative().nullable(),
  monthly_tokens: z.number().int().nonnegative().nullable(),
  monthly_cost_micros: z.number().int().nonnegative().nullable(),
  hard_cost_limit: z.boolean(),
  updated_at: z.string().datetime({ offset: true }),
})
export type QuotaLimitsRecord = z.infer<typeof quotaLimitsRecordSchema>

export const quotaUsageRecordSchema = z.object({
  schema_version: z.number().int(),
  scope_type: quotaScopeTypeSchema,
  scope_id: z.string().uuid(),
  period_start: z.string().date(),
  storage_bytes: z.number().int().nonnegative(),
  running_runs: z.number().int().nonnegative(),
  tokens: z.number().int().nonnegative(),
  cost_micros: z.number().int().nonnegative(),
  updated_at: z.string().datetime({ offset: true }),
})
export type QuotaUsageRecord = z.infer<typeof quotaUsageRecordSchema>

export const quotaStatusSchema = z.object({
  schema_version: z.number().int(),
  limits: quotaLimitsRecordSchema,
  usage: quotaUsageRecordSchema,
})
export type QuotaStatus = z.infer<typeof quotaStatusSchema>

export type SetQuotaLimitsRequest = {
  storage_bytes: number | null
  concurrent_runs: number | null
  monthly_tokens: number | null
  monthly_cost_micros: number | null
  hard_cost_limit: boolean
}

export const operationsSnapshotSchema = z.object({
  schema_version: z.number().int().positive(),
  generated_at: z.string().datetime({ offset: true }),
  application: z.object({
    build_version: z.string(),
    api_version: z.string(),
    minimum_compatible_schema_version: z.number().int().positive(),
    database_migration_version: z.number().int().nonnegative(),
    object_store_configured: z.boolean(),
    runner_configured: z.boolean(),
    push_configured: z.boolean(),
    passkeys_configured: z.boolean(),
    oidc_configured: z.boolean(),
  }),
  database: z.object({
    users: z.number().int().nonnegative(),
    teams: z.number().int().nonnegative(),
    projects: z.number().int().nonnegative(),
    threads: z.number().int().nonnegative(),
    audit_events: z.number().int().nonnegative(),
  }),
  workload: z.object({
    runs_by_state: z.record(z.string(), z.number().int().nonnegative()),
    schedules_enabled: z.number().int().nonnegative(),
    schedules_overdue: z.number().int().nonnegative(),
    approvals_waiting: z.number().int().nonnegative(),
    input_requests_waiting: z.number().int().nonnegative(),
    executors_registered: z.number().int().nonnegative(),
    executors_recently_seen: z.number().int().nonnegative(),
    active_support_grants: z.number().int().nonnegative(),
  }),
  storage: z.object({
    snapshots_by_state: z.record(z.string(), z.number().int().nonnegative()),
    ready_chunk_plaintext_bytes: z.number().int().nonnegative(),
    ready_chunk_ciphertext_bytes: z.number().int().nonnegative(),
    unreferenced_chunks: z.number().int().nonnegative(),
    live_artifact_bytes: z.number().int().nonnegative(),
  }),
  delivery: z.object({
    active_push_subscriptions: z.number().int().nonnegative(),
    pending_push_deliveries: z.number().int().nonnegative(),
    failed_push_deliveries: z.number().int().nonnegative(),
  }),
})
export type OperationsSnapshot = z.infer<typeof operationsSnapshotSchema>

export const taskDefinitionSchema = z.object({
  schema_version: z.number().int(),
  id: z.string().uuid(),
  revision: z.number().int().positive(),
  etag: z.string(),
  project_id: z.string().uuid(),
  name: z.string(),
  instructions: z.string(),
  required_capabilities: z.array(capabilitySchema),
  default_target: executorTargetSchema.nullable(),
  config: z.unknown(),
  released: z.boolean(),
  created_at: z.string().datetime({ offset: true }),
  deleted_at: z.string().datetime({ offset: true }).nullable(),
})
export type TaskDefinition = z.infer<typeof taskDefinitionSchema>

export const scheduleRecordSchema = z.object({
  schema_version: z.number().int(),
  id: z.string().uuid(),
  revision: z.number().int().positive(),
  etag: z.string(),
  task_id: z.string().uuid(),
  project_id: z.string().uuid(),
  thread_id: z.string().uuid(),
  cron: z.string(),
  timezone: z.string(),
  executor_target: executorTargetSchema,
  input: z.unknown(),
  model_profile_id: z.string().uuid().nullable(),
  enabled: z.boolean(),
  next_run_at: z.string().datetime({ offset: true }).nullable(),
  last_triggered_at: z.string().datetime({ offset: true }).nullable(),
  blocked_reason: z.string().nullable(),
  created_at: z.string().datetime({ offset: true }),
  updated_at: z.string().datetime({ offset: true }),
  deleted_at: z.string().datetime({ offset: true }).nullable(),
})
export type ScheduleRecord = z.infer<typeof scheduleRecordSchema>

export const totpStatusSchema = z.object({
  schema_version: z.number().int(),
  enabled: z.boolean(),
  unused_recovery_codes: z.number().int().nonnegative(),
})
export type TotpStatus = z.infer<typeof totpStatusSchema>

export const totpSetupSchema = z.object({
  schema_version: z.number().int(),
  secret: z.string().min(16),
  otpauth_uri: z.string().startsWith('otpauth://totp/'),
  expires_at: z.string().datetime({ offset: true }),
})
export type TotpSetup = z.infer<typeof totpSetupSchema>

export const totpRecoveryCodesSchema = z.object({
  schema_version: z.number().int(),
  recovery_codes: z.array(z.string()).length(10),
})
export type TotpRecoveryCodes = z.infer<typeof totpRecoveryCodesSchema>

export const authSessionRecordSchema = z.object({
  schema_version: z.number().int(),
  id: z.string().uuid(),
  device_id: z.string().uuid(),
  current: z.boolean(),
  active: z.boolean(),
  created_at: z.string().datetime({ offset: true }),
  last_used_at: z.string().datetime({ offset: true }),
  expires_at: z.string().datetime({ offset: true }),
  revoked_at: z.string().datetime({ offset: true }).nullable(),
  revoke_reason: z.string().nullable(),
})
export type AuthSessionRecord = z.infer<typeof authSessionRecordSchema>

export const passkeyChallengeSchema = z.object({
  schema_version: z.number().int(),
  challenge_id: z.string().uuid(),
  public_key: z.unknown(),
  expires_at: z.string().datetime({ offset: true }),
})
export type PasskeyChallenge = z.infer<typeof passkeyChallengeSchema>

export const passkeyRecordSchema = z.object({
  schema_version: z.number().int(),
  id: z.string().uuid(),
  label: z.string().min(1),
  created_at: z.string().datetime({ offset: true }),
  last_used_at: z.string().datetime({ offset: true }).nullable(),
})
export type PasskeyRecord = z.infer<typeof passkeyRecordSchema>

export const capabilityDescriptorSchema = z.object({
  schema_version: z.number().int(),
  name: capabilitySchema,
  version: z.string(),
  attributes: z.record(z.string(), z.unknown()).default({}),
})
export type CapabilityDescriptor = z.infer<typeof capabilityDescriptorSchema>

export const executorKindSchema = z.enum([
  'server_linux',
  'managed_windows',
  'personal_device',
])

export const personalDeviceRemoteControlModeSchema = z.enum([
  'off',
  'confirm_each_session',
  'unattended',
])
export type PersonalDeviceRemoteControlMode = z.infer<typeof personalDeviceRemoteControlModeSchema>

export const executorRegistrationSchema = z.object({
  schema_version: z.number().int(),
  executor_id: z.string().uuid(),
  kind: executorKindSchema,
  pool_id: z.string().uuid().nullable().optional(),
  owner_user_id: z.string().uuid().nullable().optional(),
  display_name: z.string(),
  protocol_version: z.number().int(),
  capabilities: z.array(capabilityDescriptorSchema),
  labels: z.record(z.string(), z.string()).default({}),
  personal_device_remote_control: personalDeviceRemoteControlModeSchema.optional(),
  max_concurrent_runs: z.number().int().positive(),
})
export type ExecutorRegistration = z.infer<typeof executorRegistrationSchema>

export const executorRecordSchema = z.object({
  registration: executorRegistrationSchema,
  online: z.boolean(),
  draining: z.boolean(),
  active_runs: z.number().int().nonnegative(),
  last_seen_at: z.string().datetime({ offset: true }),
})
export type ExecutorRecord = z.infer<typeof executorRecordSchema>

export const capabilityCatalogSchema = z.object({
  schema_version: z.number().int(),
  server_linux: z.array(capabilityDescriptorSchema),
  executors: z.array(executorRecordSchema),
})
export type CapabilityCatalog = z.infer<typeof capabilityCatalogSchema>

export const versionResponseSchema = z.object({
  api_version: z.string(),
  schema_version: z.number().int(),
  minimum_compatible_schema_version: z.number().int(),
  build_version: z.string(),
})
export type VersionResponse = z.infer<typeof versionResponseSchema>

export const syncOperationSchema = z.enum(['upsert', 'delete'])
export type SyncOperation = z.infer<typeof syncOperationSchema>

export const syncChangeSchema = z.object({
  schema_version: z.number().int().positive(),
  operation_id: z.string().uuid(),
  device_id: z.string().uuid(),
  entity_type: z.string(),
  entity_id: z.string().uuid(),
  base_revision: z.number().int().nonnegative(),
  operation: syncOperationSchema,
  payload: z.unknown().nullable(),
  client_timestamp: z.string().datetime({ offset: true }),
})
export type SyncChange = z.infer<typeof syncChangeSchema>

export const syncedEntitySchema = z.object({
  schema_version: z.number().int().positive(),
  entity_type: z.string(),
  entity_id: z.string().uuid(),
  revision: z.number().int().positive(),
  etag: z.string(),
  payload: z.unknown().nullable(),
  tombstone: z.boolean(),
  updated_at: z.string().datetime({ offset: true }),
})
export type SyncedEntity = z.infer<typeof syncedEntitySchema>

export const syncedEntityPageSchema = z.object({
  schema_version: z.number().int().positive(),
  items: z.array(syncedEntitySchema),
  next_after: z.string().uuid().nullable(),
  watermark_cursor: z.number().int().nonnegative(),
})
export type SyncedEntityPage = z.infer<typeof syncedEntityPageSchema>

export const syncApplyResultSchema = z.object({
  schema_version: z.number().int().positive(),
  operation_id: z.string().uuid(),
  status: z.enum(['applied', 'conflict']),
  entity: syncedEntitySchema.nullable(),
})
export type SyncApplyResult = z.infer<typeof syncApplyResultSchema>

export const pushSyncChangesResponseSchema = z.object({
  schema_version: z.number().int().positive(),
  results: z.array(syncApplyResultSchema),
})
export type PushSyncChangesResponse = z.infer<typeof pushSyncChangesResponseSchema>

export const serverSyncChangeSchema = z.object({
  schema_version: z.number().int().positive(),
  cursor: z.number().int().positive(),
  entity_type: z.string(),
  entity_id: z.string().uuid(),
  revision: z.number().int().positive(),
  operation: syncOperationSchema,
  payload: z.unknown().nullable(),
  created_at: z.string().datetime({ offset: true }),
})
export type ServerSyncChange = z.infer<typeof serverSyncChangeSchema>

export const pullSyncChangesResponseSchema = z.object({
  schema_version: z.number().int().positive(),
  changes: z.array(serverSyncChangeSchema),
  next_cursor: z.number().int().nonnegative(),
})
export type PullSyncChangesResponse = z.infer<typeof pullSyncChangesResponseSchema>

export const listRunsResponseSchema = z.object({
  items: z.array(runRecordSchema),
  next_cursor: z.string().nullable().optional(),
})

export interface CreateRunRequest {
  thread_id: string
  project_id: string
  project_revision: number
  project_privacy: ProjectPrivacy
  task?: { id: string; revision: number } | null
  executor_target: ExecutorTarget
  required_capabilities: Capability[]
  input: unknown
  model_profile_id?: string | null
  snapshot_id?: string | null
  idempotency_key: string
}

export interface CreateThreadMessageRequest {
  content: unknown
  run: CreateRunRequest
}

export function assertProtocolCompatible(version: VersionResponse): void {
  if (!protocolVersionsCompatible(
    SCHEMA_VERSION,
    MIN_COMPATIBLE_SCHEMA_VERSION,
    version.schema_version,
    version.minimum_compatible_schema_version,
  )) {
    throw new Error(
      `Incompatible Open Cowork protocol: client=${SCHEMA_VERSION}, server=${version.schema_version}`,
    )
  }
}

export function protocolVersionsCompatible(
  clientVersion: number,
  clientMinimum: number,
  serverVersion: number,
  serverMinimum: number,
): boolean {
  return clientVersion >= serverMinimum && clientVersion <= serverVersion
    && serverVersion >= clientMinimum && serverVersion <= clientVersion + 1
}
