import { z } from 'zod'

import {
  MIN_COMPATIBLE_SCHEMA_VERSION,
  SCHEMA_VERSION,
  approvalRequestSchema,
  authSessionRecordSchema,
  capabilityCatalogSchema,
  capabilityDescriptorSchema,
  capabilitySchema,
  desktopSessionSchema,
  desktopStreamTicketSchema,
  executorKindSchema,
  executorRecordSchema,
  executorRegistrationSchema,
  executorTargetSchema,
  listRunsResponseSchema,
  messageRecordSchema,
  messageRoleSchema,
  operationsSnapshotSchema,
  passkeyChallengeSchema,
  passkeyRecordSchema,
  projectPrivacySchema,
  projectRecordSchema,
  pushConfigurationSchema,
  pushSubscriptionRecordSchema,
  quotaLimitsRecordSchema,
  quotaScopeTypeSchema,
  quotaStatusSchema,
  quotaUsageRecordSchema,
  reauthenticationGrantSchema,
  runArtifactSchema,
  runEventSchema,
  runInputRequestSchema,
  runRecordSchema,
  runSpecSchema,
  runStateSchema,
  scheduleRecordSchema,
  supportGrantRecordSchema,
  taskDefinitionSchema,
  terminalSessionTicketSchema,
  threadRecordSchema,
  threadMessageRunSchema,
  totpRecoveryCodesSchema,
  totpSetupSchema,
  totpStatusSchema,
  versionResponseSchema,
} from './contracts'

export const protocolSchemaVersion = SCHEMA_VERSION
export const minimumCompatibleSchemaVersion = MIN_COMPATIBLE_SCHEMA_VERSION

const uuid = z.string().uuid()
const timestamp = z.string().datetime({ offset: true })
const revision = z.number().int().positive()
const jsonValue = z.unknown()

export const executorPoolSchema = z.object({
  schema_version: z.number().int().positive(), id: uuid, revision, etag: z.string(),
  name: z.string(), kind: executorKindSchema, team_id: uuid.nullable(), policy: jsonValue,
  deleted_at: timestamp.nullable(),
})

export const snapshotChunkSchema = z.object({
  digest: z.string().regex(/^[0-9a-f]{64}$/), plaintext_size: z.number().int().nonnegative(),
  ciphertext_size: z.number().int().nonnegative(),
})
export const snapshotFileSchema = z.object({
  path: z.string(), size: z.number().int().nonnegative(), mode: z.number().int().nonnegative(),
  modified_at: timestamp, chunks: z.array(snapshotChunkSchema),
})
export const snapshotManifestSchema = z.object({
  schema_version: z.number().int().positive(), id: uuid, project_id: uuid,
  total_bytes: z.number().int().nonnegative(), files: z.array(snapshotFileSchema),
  encryption_key_id: z.string(), created_at: timestamp, expires_at: timestamp.nullable(),
})
export const snapshotUploadSessionSchema = z.object({
  schema_version: z.number().int().positive(), manifest_id: uuid,
  missing_chunks: z.array(z.string().regex(/^[0-9a-f]{64}$/)),
  max_chunk_bytes: z.number().int().positive(), expires_at: timestamp.nullable(),
  warnings: z.array(z.string()),
})
export const snapshotChunkReceiptSchema = z.object({
  schema_version: z.number().int().positive(), manifest_id: uuid,
  digest: z.string().regex(/^[0-9a-f]{64}$/), plaintext_size: z.number().int().nonnegative(),
  ciphertext_size: z.number().int().nonnegative(), deduplicated: z.boolean(),
  warnings: z.array(z.string()),
})
export const projectVersionSchema = z.object({
  schema_version: z.number().int().positive(), id: uuid, project_id: uuid, revision,
  parent_version_id: uuid.nullable(), merge_base_version_id: uuid.nullable(),
  snapshot_manifest_id: uuid, created_by_run_id: uuid.nullable(), created_at: timestamp,
})
export const syncChangeSchema = z.object({
  schema_version: z.number().int().positive(), operation_id: uuid, device_id: uuid,
  entity_type: z.string(), entity_id: uuid, base_revision: z.number().int().nonnegative(),
  operation: z.enum(['upsert', 'delete']), payload: jsonValue.nullable(), client_timestamp: timestamp,
})
export const runCheckpointSchema = z.object({
  schema_version: z.number().int().positive(), id: uuid, run_id: uuid,
  sequence: z.number().int().nonnegative(), safe_to_resume: z.boolean(),
  executor_state: jsonValue, created_at: timestamp,
})

export const contractSchemaRegistry = {
  ApprovalRequest: approvalRequestSchema,
  AuthSessionRecord: authSessionRecordSchema,
  Capability: capabilitySchema,
  CapabilityCatalog: capabilityCatalogSchema,
  CapabilityDescriptor: capabilityDescriptorSchema,
  DesktopSession: desktopSessionSchema,
  DesktopStreamTicket: desktopStreamTicketSchema,
  ExecutorPool: executorPoolSchema,
  ExecutorRecord: executorRecordSchema,
  ExecutorRegistration: executorRegistrationSchema,
  ExecutorTarget: executorTargetSchema,
  ListRunsResponse: listRunsResponseSchema,
  MessageRecord: messageRecordSchema,
  MessageRole: messageRoleSchema,
  OperationsSnapshot: operationsSnapshotSchema,
  PasskeyChallenge: passkeyChallengeSchema,
  PasskeyRecord: passkeyRecordSchema,
  ProjectPrivacy: projectPrivacySchema,
  ProjectRecord: projectRecordSchema,
  ProjectVersion: projectVersionSchema,
  PushConfiguration: pushConfigurationSchema,
  PushSubscriptionRecord: pushSubscriptionRecordSchema,
  QuotaLimitsRecord: quotaLimitsRecordSchema,
  QuotaScopeType: quotaScopeTypeSchema,
  QuotaStatus: quotaStatusSchema,
  QuotaUsageRecord: quotaUsageRecordSchema,
  ReauthenticationGrant: reauthenticationGrantSchema,
  RunArtifact: runArtifactSchema,
  RunCheckpoint: runCheckpointSchema,
  RunEvent: runEventSchema,
  RunInputRequest: runInputRequestSchema,
  RunRecord: runRecordSchema,
  RunSpec: runSpecSchema,
  RunState: runStateSchema,
  ScheduleRecord: scheduleRecordSchema,
  SnapshotChunk: snapshotChunkSchema,
  SnapshotChunkReceipt: snapshotChunkReceiptSchema,
  SnapshotFile: snapshotFileSchema,
  SnapshotManifest: snapshotManifestSchema,
  SnapshotUploadSession: snapshotUploadSessionSchema,
  SupportGrantRecord: supportGrantRecordSchema,
  SyncChange: syncChangeSchema,
  TaskDefinition: taskDefinitionSchema,
  TerminalSessionTicket: terminalSessionTicketSchema,
  ThreadRecord: threadRecordSchema,
  ThreadMessageRun: threadMessageRunSchema,
  TotpRecoveryCodes: totpRecoveryCodesSchema,
  TotpSetup: totpSetupSchema,
  TotpStatus: totpStatusSchema,
  VersionResponse: versionResponseSchema,
} as const
