import {organisationMfaPolicySchema,organisationMfaUpdateSchema,organisationMfaReceiptSchema,instanceMfaConfigurationSchema} from './organisationMfa'
import { z } from 'zod'
import { executorTargetSchema } from './contracts'

const id = z.uuid()
const releaseId = z.string().regex(/^[a-f0-9]{64}$/)
export const resourceLimitsSchema = z.object({ cpu_millis: z.number().int().min(100), memory_mib: z.number().int().min(128), storage_gib: z.number().int().positive(), concurrent_runs: z.number().int().positive(), instances: z.number().int().positive() }).strict()
export const enterprisePolicySchema = z.object({
  revision: z.number().int().positive(), extensions: z.enum(['catalog', 'approval_required', 'unrestricted']),
  content_access: z.enum(['support_grant', 'organisation_admin']), minimum_isolation: z.enum(['container', 'virtual_machine']),
  allow_laptop_full_access: z.boolean(), allow_offline_continuation: z.boolean(), allowed_network_origins: z.array(z.string()),
  limits: resourceLimitsSchema, audit_retention_days: z.number().int().min(1).max(3650), content_logging: z.boolean(),
}).strict()
export type EnterprisePolicy = z.infer<typeof enterprisePolicySchema>
export const organisationSchema = z.object({ id, name: z.string(), policy: enterprisePolicySchema, role: z.enum(['admin', 'member', 'auditor']), created_at: z.string() })
export const instanceSchema = z.object({ id, organisation_id: id, owner_user_id: id, name: z.string(), backend: z.enum(['compose', 'kubernetes']), state: z.enum(['pending', 'provisioning', 'ready', 'stopped', 'suspended', 'deleting', 'deleted', 'failed']), policy: enterprisePolicySchema, revision: z.number().int(), created_at: z.string(), current_release: releaseId.nullable().optional(), provisioning_release: releaseId.nullable().optional(), rollout_id: id.nullable().optional(), execution_required:z.boolean().optional(), can_manage: z.boolean().optional(), can_open: z.boolean().optional(), can_open_support: z.boolean().optional(), available_actions: z.array(z.enum(['provision', 'start', 'stop', 'suspend', 'resume', 'backup', 'restore', 'upgrade', 'rollback', 'enroll_execution', 'delete'])).optional() })
export const createInstanceSchema = z.object({ name:z.string().min(1).max(120),backend:z.enum(['compose','kubernetes']),policy:enterprisePolicySchema.nullish(),release:releaseId.nullish() }).strict()
export const provisioningReleaseSchema = z.object({id:releaseId,name:z.string()}).strict()
export type ProvisioningRelease = z.infer<typeof provisioningReleaseSchema>
export const instanceReleaseSchema = z.object({ id: releaseId, name: z.string(), source_release: releaseId, qualified: z.boolean() }).strict()
export const instanceRollbackSchema = z.object({ operation_id: id, backup_id: id, source_release: releaseId, release: releaseId, created_at: z.string() }).strict()
export type InstanceRollback = z.infer<typeof instanceRollbackSchema>
export type InstanceRelease = z.infer<typeof instanceReleaseSchema>
export type Organisation = z.infer<typeof organisationSchema>
export type ManagedInstance = z.infer<typeof instanceSchema>
export const instanceBackupSchema = z.object({ id, organisation_id: id, instance_id: id, operation_id: id, sha256: z.string().regex(/^[0-9a-f]{64}$/), size_bytes: z.number().int().positive(), manifest_version: z.literal(1), created_at: z.string() }).strict()
export type InstanceBackup = z.infer<typeof instanceBackupSchema>
export const operationRecoveryRequestSchema = z.discriminatedUnion('mode', [z.object({ mode: z.literal('stop_if_unproven') }).strict(), z.object({mode:z.literal('continue_enrollment')}).strict(), z.object({ mode: z.literal('continue_deletion'), confirmation_name: z.string().min(1).max(120), purge_backups: z.literal(true) }).strict()])
export const instanceActionSchema = z.enum(['provision', 'start', 'stop', 'suspend', 'resume', 'backup', 'restore', 'upgrade', 'rollback', 'enroll_execution', 'delete'])
export type InstanceAction = z.infer<typeof instanceActionSchema>
export const organisationListSchema = z.object({ schema_version: z.literal(3), platform_admin: z.boolean(), execution_admission_enabled:z.boolean().optional(), release_selection_enabled:z.boolean().optional(), instance_imports_enabled:z.boolean().optional(), items: z.array(organisationSchema) })
export const environmentSchema = z.object({ instance_id: id, name: z.string(), backend: z.enum(['docker', 'kubernetes', 'libvirt', 'kubevirt', 'hyper_v', 'open_shell', 'attached']), isolation: z.enum(['container', 'virtual_machine']), template_id: id, template_revision: z.number().int().positive(), persistent: z.boolean(), shared_workspace: z.boolean(), limits: resourceLimitsSchema }).strict()
export const chatExecutionPolicySchema = z.object({ revision: z.number().int().positive(), environments: z.array(id), device_grants: z.array(id), transfer_grants: z.array(id) }).strict()
export const deviceGrantSchema = z.object({ id, organisation_id: id, instance_id: id, user_id: id, device_id: id, chat_id: id, access: z.enum(['none', 'selected', 'sandbox', 'full']), read_roots: z.array(z.string()), write_roots: z.array(z.string()), capabilities: z.array(z.string()), policy_revision: z.number().int().positive(), locally_confirmed_at: z.string(), revoked_at: z.string().nullable() }).strict()
export const executionDestinationSchema = z.discriminatedUnion('kind', [z.object({ kind: z.literal('environment'), id }), z.object({ kind: z.literal('device'), id, grant_id: id })])
export const transferGrantSchema = z.object({ id, organisation_id: id, instance_id: id, chat_id: id, source: executionDestinationSchema, destination: executionDestinationSchema, relative_paths: z.array(z.string()), writable: z.boolean(), max_bytes: z.number().int().nonnegative() }).strict()
export const executionStepSchema = z.object({ id, destination: executionDestinationSchema, depends_on: z.array(id), state: z.enum(['pending', 'ready', 'running', 'offline_running', 'waiting_input', 'completed', 'interrupted', 'failed', 'canceled']), idempotency_key: z.string(), required_capabilities: z.array(z.string()) }).strict()
export const workflowRunSchema = z.object({ id, organisation_id: id, instance_id: id, chat_id: id, policy: chatExecutionPolicySchema, steps: z.array(executionStepSchema) }).strict()
export const managementActionRequestSchema = z.object({ action: instanceActionSchema, expected_revision: z.number().int(), idempotency_key: id, backup_id: id.nullable().optional(), release: z.string().nullable().optional(), qualification_only: z.boolean().optional(), rollback_operation_id: id.nullable().optional(), confirmation_name: z.string().nullable().optional(), purge_backups: z.boolean().optional(), reconcile_reservation_ids:z.array(id).max(100).optional(), acknowledge_execution_stop:z.boolean().optional(), acknowledge_execution_enrollment:z.boolean().optional() }).strict()
export const createRolloutSchema = z.object({ name: z.string().min(1).max(120), release: releaseId, batch_size: z.number().int().min(1).max(10), targets: z.array(z.object({ instance_id: id, expected_revision: z.number().int() }).strict()).min(1).max(40), idempotency_key: id }).strict()
export const controlRolloutSchema = z.object({ action: z.enum(['pause', 'resume', 'cancel']), expected_revision: z.number().int() }).strict()
export const rolloutTargetSchema = z.object({ organisation_id: id, rollout_id: id, instance_id: id, instance_name: z.string(), ordinal: z.number().int(), source_release: releaseId, initial_state: z.enum(['ready', 'stopped']), expected_revision: z.number().int(), state: z.enum(['waiting', 'active', 'succeeded', 'failed', 'canceled']), next_action: z.enum(['stop', 'upgrade', 'start']).nullable(), operation_id: id.nullable(), error_code: z.string().nullable() })
export const rolloutSchema = z.object({ id, organisation_id: id, actor_user_id: id, name: z.string(), release: releaseId, batch_size: z.number().int(), state: z.enum(['running', 'paused', 'canceling', 'canceled', 'succeeded']), error_code: z.string().nullable(), revision: z.number().int(), created_at: z.string(), updated_at: z.string(), targets: z.array(rolloutTargetSchema) })
export type InstanceRollout = z.infer<typeof rolloutSchema>
export type CreateRollout = z.infer<typeof createRolloutSchema>
export type RolloutControl = z.infer<typeof controlRolloutSchema>['action']
export const instanceLaunchSchema = z.object({ url: z.url(), email: z.email(), ticket: z.string() }).strict()
export const instanceContentAccessSchema = z.object({ managed: z.boolean(), organisation_id: id.nullable(), instance_id: id.nullable(), policy_revision: z.number().int().positive().nullable(), content_access: z.enum(['support_grant', 'organisation_admin']).nullable(), read_only: z.boolean(), instance_viewer: z.boolean().optional(), support_administrator: z.boolean(), automatic_content_access: z.boolean(), administrators: z.array(z.object({ user_id: id, display_name: z.string(), email: z.email() }).strict()) }).strict()
export type InstanceContentAccess = z.infer<typeof instanceContentAccessSchema>
export const instanceMemberRoleSchema = z.enum(['manager', 'member', 'viewer'])
export const instanceMemberSchema = z.object({ user_id: id, email: z.email(), display_name: z.string(), role: z.enum(['owner', 'manager', 'member', 'viewer']), organisation_role: z.enum(['admin', 'member', 'auditor','suspended']) }).strict()
export const instanceMemberCandidateSchema = z.object({ user_id: id, email: z.email(), display_name: z.string() }).strict()
export const instanceMemberRequestSchema = z.object({ user_id: id, role: instanceMemberRoleSchema }).strict()
export type InstanceMember = z.infer<typeof instanceMemberSchema>
export type InstanceMemberRole = z.infer<typeof instanceMemberRoleSchema>
export type InstanceMemberCandidate = z.infer<typeof instanceMemberCandidateSchema>
const organisationRoleSchema = z.enum(['admin', 'member', 'auditor'])
export const ssoGroupMappingSchema = z.object({ group:z.string().min(1).max(256),role:organisationRoleSchema }).strict()
export const organisationSsoProviderSchema = z.object({ id:z.string(),label:z.string(),issuer:z.string(),groups_claim:z.string() }).strict()
export const organisationSsoSettingsSchema = z.object({ organisation_id:id,revision:z.number().int().nonnegative(),enabled:z.boolean(),available:z.boolean(),provider_id:z.string().nullable(),auto_provision:z.boolean(),group_mappings:z.array(ssoGroupMappingSchema).max(100),providers:z.array(organisationSsoProviderSchema) }).strict()
export const updateOrganisationSsoSchema = z.object({ expected_revision:z.number().int().nonnegative(),enabled:z.boolean(),provider_id:z.string().nullable(),auto_provision:z.boolean(),group_mappings:z.array(ssoGroupMappingSchema).max(100) }).strict()
export const organisationSsoPublicSchema = z.object({ enabled:z.boolean(),organisation_id:id,organisation_name:z.string(),provider_label:z.string().nullable() }).strict()
export type OrganisationSsoSettings = z.infer<typeof organisationSsoSettingsSchema>
export type UpdateOrganisationSso = z.infer<typeof updateOrganisationSsoSchema>
export const startOidcAuthorizationSchema = z.object({device_id:id,code_challenge:z.string(),code_challenge_method:z.literal('S256'),client_state:z.string(),redirect_uri:z.string()}).strict()
export const oidcAuthorizationSchema = z.object({schema_version:z.literal(2),authorization_url:z.url(),expires_at:z.string()}).strict()
export const createOrganisationInvitationSchema = z.object({ email: z.email(), role: organisationRoleSchema, expires_at: z.string().datetime({ offset: true }).nullable().optional() }).strict()
export const organisationInvitationSchema = z.object({ id, organisation_id: id, email: z.email(), role: organisationRoleSchema, invited_by: id, created_at: z.string(), expires_at: z.string(), state: z.enum(['pending', 'accepted', 'revoked', 'expired', 'authority_revoked']) }).strict()
export const organisationInvitationSecretSchema = z.object({ id, organisation_id: id, email: z.email(), role: organisationRoleSchema, expires_at: z.string(), token: z.string().min(32).max(256) }).strict()
export const organisationInvitationTokenSchema = z.object({ token: z.string().min(32).max(256) }).strict()
export const organisationInvitationPreviewSchema = z.object({ id, organisation_id: id, organisation_name: z.string(), email: z.email(), role: organisationRoleSchema, expires_at: z.string(), account_exists: z.boolean() }).strict()
export const organisationInvitationAcceptedSchema = z.object({ organisation_id: id, user_id: id, role: organisationRoleSchema, membership_created: z.boolean() }).strict()
export const registerOrganisationInvitationSchema = z.object({ token: z.string().min(32).max(256), display_name: z.string().min(1).max(200), password: z.string().min(12).max(1024), device_id: id }).strict()
export type OrganisationInvitation = z.infer<typeof organisationInvitationSchema>
export type OrganisationInvitationSecret = z.infer<typeof organisationInvitationSecretSchema>
export type OrganisationInvitationPreview = z.infer<typeof organisationInvitationPreviewSchema>
export type CreateOrganisationInvitation = z.infer<typeof createOrganisationInvitationSchema>
export const verifySecondFactorSchema = z.object({ code:z.string().min(1).max(128) }).strict()
export const authenticationAssuranceSchema = z.object({ required:z.boolean(),totp_enabled:z.boolean(),passkey_registered:z.boolean(),session_verified:z.boolean(),enrollment_allowed:z.boolean(),reauthentication_required:z.boolean(),provider_verified:z.boolean().optional(),provider_verification_expires_at:z.string().optional(),oidc_reauthentication_available:z.boolean().optional(),oidc_organisation_id:id.optional(),organisation_id:id.optional(),organisation_policy_revision:z.number().int().positive().optional(),verification_expires_at:z.string().optional() }).strict()
export type AuthenticationAssurance = z.infer<typeof authenticationAssuranceSchema>
export const executionBudgetSchema = z.object({ revision:z.number().int().nonnegative(), organisation_concurrent_runs:z.number().int().positive(), user_concurrent_runs:z.number().int().positive() }).strict()
export const setExecutionBudgetSchema = z.object({ expected_revision:z.number().int().nonnegative(), organisation_concurrent_runs:z.number().int().positive(), user_concurrent_runs:z.number().int().positive() }).strict()
export const executionAdmissionRequestSchema = z.object({ organisation_id:id, instance_id:id, execution_epoch:id, run_id:id, user_id:id, target:executorTargetSchema,mfa_contract:z.literal(1).optional() }).strict()
export const executionAdmissionDecisionSchema = z.object({ admitted:z.boolean(), reservation_id:id.nullable(), reason:z.enum(['platform','organisation','user','instance']).nullable() }).strict()
export const executionReleaseRequestSchema = z.object({ organisation_id:id, instance_id:id, execution_epoch:id, run_id:id, reservation_id:id, completion_id:id, outcome:z.enum(['completed','failed','canceled']), execution_stopped:z.boolean() }).strict()
export const executionReleaseAcknowledgementSchema = z.object({ reservation_id:id, released:z.boolean() }).strict()
export const executionReservationSchema = executionAdmissionRequestSchema.extend({ id, created_at:z.string() }).strict()
export const executionCapacitySchema = z.object({ organisation_id:id, budget:executionBudgetSchema, held_runs:z.number().int().nonnegative(), reservations:z.array(executionReservationSchema) }).strict()
export type ExecutionCapacity = z.infer<typeof executionCapacitySchema>
export type SetExecutionBudget = z.infer<typeof setExecutionBudgetSchema>
export const executionGenerationBindingSchema = z.object({ operation_id:id, organisation_id:id, instance_id:id, source_epoch:id, candidate_epoch:id }).strict()
export const executionRuntimeAttestationSchema = z.object({version:z.literal(1),server_image:z.string().regex(/^[a-zA-Z0-9][a-zA-Z0-9._:/-]*@sha256:[a-f0-9]{64}$/),manifest_sha256:z.string().regex(/^[a-f0-9]{64}$/)}).strict()
export const executionGenerationReceiptSchema = z.object({ binding:executionGenerationBindingSchema, active_epoch:id, runtime:executionRuntimeAttestationSchema.optional() }).strict()
export const executionRecoveryReservationSchema = z.object({reservation_id:id,execution_epoch:id,run_id:id}).strict()
export const executionRecoveryBindingSchema = z.object({generation:executionGenerationBindingSchema,reservations:z.array(executionRecoveryReservationSchema).min(1).max(100)}).strict()
export const executionRecoveryReceiptSchema = z.object({binding:executionRecoveryBindingSchema,stopped:z.boolean(),inventory_sha256:z.string().regex(/^[a-f0-9]{64}$/),verified_generations:z.number().int().positive()}).strict()
export const executionEnrollmentReceiptSchema = z.object({binding:executionGenerationBindingSchema,outcome:z.enum(['enrolled','unchanged']),source_manifest_sha256:z.string().regex(/^[a-f0-9]{64}$/),inventory_sha256:z.string().regex(/^[a-f0-9]{64}$/).nullable(),verified_generations:z.number().int().nonnegative()}).strict()
export const enterpriseContractRegistry = {
  OrganisationMfaPolicy:organisationMfaPolicySchema,UpdateOrganisationMfaPolicy:organisationMfaUpdateSchema,OrganisationMfaReceipt:organisationMfaReceiptSchema,InstanceMfaConfiguration:instanceMfaConfigurationSchema,
  ExecutionEnrollmentReceipt:executionEnrollmentReceiptSchema,
  ExecutionRuntimeAttestation:executionRuntimeAttestationSchema,
  ExecutionRecoveryReservation:executionRecoveryReservationSchema,ExecutionRecoveryBinding:executionRecoveryBindingSchema,ExecutionRecoveryReceipt:executionRecoveryReceiptSchema,
  ExecutionGenerationBinding:executionGenerationBindingSchema, ExecutionGenerationReceipt:executionGenerationReceiptSchema,
  ExecutionReleaseAcknowledgement:executionReleaseAcknowledgementSchema, ExecutionBudget:executionBudgetSchema, SetExecutionBudget:setExecutionBudgetSchema, ExecutionAdmissionRequest:executionAdmissionRequestSchema, ExecutionAdmissionDecision:executionAdmissionDecisionSchema, ExecutionReleaseRequest:executionReleaseRequestSchema, ExecutionReservation:executionReservationSchema, ExecutionCapacity:executionCapacitySchema,
  StartOidcAuthorizationRequest:startOidcAuthorizationSchema, OidcAuthorization:oidcAuthorizationSchema,
  SsoGroupMapping:ssoGroupMappingSchema, OrganisationSsoProvider:organisationSsoProviderSchema, OrganisationSsoSettings:organisationSsoSettingsSchema, UpdateOrganisationSso:updateOrganisationSsoSchema, OrganisationSsoPublic:organisationSsoPublicSchema,
  AuthenticationAssurance: authenticationAssuranceSchema, VerifySecondFactor:verifySecondFactorSchema,
  CreateOrganisationInvitation: createOrganisationInvitationSchema, OrganisationInvitation: organisationInvitationSchema, OrganisationInvitationSecret: organisationInvitationSecretSchema,
  OrganisationInvitationToken: organisationInvitationTokenSchema, OrganisationInvitationPreview: organisationInvitationPreviewSchema, OrganisationInvitationAccepted: organisationInvitationAcceptedSchema,
  RegisterOrganisationInvitation: registerOrganisationInvitationSchema,
  InstanceMemberRecord: instanceMemberSchema, InstanceMemberCandidate: instanceMemberCandidateSchema,
  InstanceMember: instanceMemberRequestSchema,
  InstanceContentAccess: instanceContentAccessSchema,
  Rollout: rolloutSchema, CreateRollout: createRolloutSchema, ControlRollout: controlRolloutSchema,
  OperationRecoveryRequest: operationRecoveryRequestSchema, ResourceLimits: resourceLimitsSchema, PolicyRevision: enterprisePolicySchema, Organization: organisationSchema,
  CreateInstance:createInstanceSchema, ProvisioningRelease:provisioningReleaseSchema,
  Instance: instanceSchema, InstanceBackup: instanceBackupSchema, InstanceRelease: instanceReleaseSchema, InstanceRollback: instanceRollbackSchema, Environment: environmentSchema, ChatExecutionPolicy: chatExecutionPolicySchema,
  DeviceGrant: deviceGrantSchema, TransferGrant: transferGrantSchema, ExecutionStep: executionStepSchema,
  WorkflowRun: workflowRunSchema, OrganisationList: organisationListSchema, ActionRequest: managementActionRequestSchema, InstanceLaunch: instanceLaunchSchema,
}
