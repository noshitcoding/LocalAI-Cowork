import { z } from 'zod'
import { directoryListSchema,directorySettingsReceiptSchema,containmentPlanSchema,containmentReceiptSchema,type SetDirectory,type ContainmentPlan } from './directoryContracts'
import { remoteTokenProvider, useRemoteRuntimeStore } from '../stores/remoteRuntimeStore'
import { normalizeServerUrl } from './runtimeClient'
import { createInstanceSchema, provisioningReleaseSchema, rolloutSchema, createRolloutSchema, controlRolloutSchema, type CreateRollout, type InstanceRollout, type RolloutControl, enterprisePolicySchema, instanceSchema, instanceBackupSchema, instanceReleaseSchema, instanceRollbackSchema, organisationListSchema, type EnterprisePolicy, type InstanceAction, type ManagedInstance } from './enterpriseContracts'
export * from './enterpriseContracts'
import { organisationSsoSettingsSchema, updateOrganisationSsoSchema, type UpdateOrganisationSso } from './enterpriseContracts'
import { executionCapacitySchema, executionBudgetSchema, setExecutionBudgetSchema, managementActionRequestSchema, type SetExecutionBudget } from './enterpriseContracts'
import { prepareImportSchema, cutoverImportSchema, abortImportSchema, preparedImportReceiptSchema, cutoverImportReceiptSchema, abortImportReceiptSchema, instanceImportSchema, instanceRouteSchema, type PrepareImport, type CutoverImport, type AbortImport } from './instanceImportContracts'

const id = z.uuid()
import { instanceMemberSchema, instanceMemberCandidateSchema, instanceMemberRequestSchema, type InstanceMemberRole } from './enterpriseContracts'
import { createOrganisationInvitationSchema, organisationInvitationSchema, organisationInvitationSecretSchema, organisationInvitationAcceptedSchema, organisationInvitationTokenSchema, type CreateOrganisationInvitation } from './enterpriseContracts'

export function instanceActions(state: ManagedInstance['state']): InstanceAction[] {
  switch (state) {
    case 'pending': return ['provision', 'delete']
    case 'ready': return ['stop', 'suspend', 'backup', 'upgrade', 'delete']
    case 'stopped': return ['start', 'suspend', 'backup', 'restore', 'upgrade', 'rollback', 'delete']
    case 'suspended': return ['resume', 'delete']
    case 'failed': return ['provision', 'restore', 'delete']
    default: return []
  }
}

export class ManagementClient {
  private baseUrl: string
  private token = remoteTokenProvider()
  private userId = useRemoteRuntimeStore.getState().userId
  constructor(baseUrl = useRemoteRuntimeStore.getState().serverUrl) { this.baseUrl = normalizeServerUrl(baseUrl) }
  async request<T>(path: string, schema: z.ZodType<T>, body?: unknown, method = body === undefined ? 'GET' : 'POST', signal?: AbortSignal): Promise<T> {
    if (!/^\/[a-z0-9/-]+$/i.test(path)) throw new Error('Invalid management resource path')
    const token = await this.token()
    if (useRemoteRuntimeStore.getState().serverUrl !== this.baseUrl || useRemoteRuntimeStore.getState().userId !== this.userId) throw new Error('Server account changed')
    const response = await fetch(`${this.baseUrl}/management/v1${path}`, {
      method, signal, credentials: 'same-origin', redirect: 'error',
      headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    })
    const data: unknown = await response.json().catch(() => null)
    if (useRemoteRuntimeStore.getState().serverUrl !== this.baseUrl || useRemoteRuntimeStore.getState().userId !== this.userId) throw new Error('Server account changed')
    if (!response.ok) {
      const error = z.object({ message: z.string() }).safeParse(data)
      throw new Error(error.success ? error.data.message : `Management request failed (${response.status})`)
    }
    return schema.parse(data)
  }
  organisations(signal?: AbortSignal) { return this.request('/organisations', organisationListSchema, undefined, 'GET', signal) }
  directory(org:string,signal?:AbortSignal){return this.request(`/organisations/${id.parse(org)}/directory`,directoryListSchema,undefined,'GET',signal)}
  setDirectory(org:string,connector:string,body:SetDirectory){return this.request(`/organisations/${id.parse(org)}/directory/${id.parse(connector)}`,directorySettingsReceiptSchema,body,'PUT')}
  prepareDirectoryContainment(org:string,connector:string){return this.request(`/organisations/${id.parse(org)}/directory/${id.parse(connector)}/containment`,containmentPlanSchema,{})}
  confirmDirectoryContainment(org:string,connector:string,plan:ContainmentPlan){return this.request(`/organisations/${id.parse(org)}/directory/${id.parse(connector)}/containment/${id.parse(plan.plan_id)}`,containmentReceiptSchema,{expected_revision:plan.expected_revision,inventory_sha256:plan.inventory_sha256,acknowledgement:plan.acknowledgement})}
  sso(org:string,signal?:AbortSignal) { return this.request(`/organisations/${id.parse(org)}/sso`,organisationSsoSettingsSchema,undefined,'GET',signal) }
  updateSso(org:string,body:UpdateOrganisationSso) { return this.request(`/organisations/${id.parse(org)}/sso`,organisationSsoSettingsSchema,updateOrganisationSsoSchema.parse(body),'PUT') }
  instances(org: string, signal?: AbortSignal) { return this.request(`/organisations/${id.parse(org)}/instances`, z.array(instanceSchema), undefined, 'GET', signal) }
  prepareInstanceImport(org: string, body: PrepareImport) { return this.request(`/organisations/${id.parse(org)}/instance-imports`, preparedImportReceiptSchema, prepareImportSchema.parse(body)) }
  instanceImport(org: string, importId: string, signal?: AbortSignal) { return this.request(`/organisations/${id.parse(org)}/instance-imports/${id.parse(importId)}`, instanceImportSchema, undefined, 'GET', signal) }
  instanceRoute(org: string, source: string, signal?: AbortSignal) { return this.request(`/organisations/${id.parse(org)}/instance-routes/${id.parse(source)}`, instanceRouteSchema, undefined, 'GET', signal) }
  cutoverInstanceImport(org: string, importId: string, body: CutoverImport) { return this.request(`/organisations/${id.parse(org)}/instance-imports/${id.parse(importId)}/cutover`, cutoverImportReceiptSchema, cutoverImportSchema.parse(body)) }
  abortInstanceImport(org: string, importId: string, body: AbortImport) { return this.request(`/organisations/${id.parse(org)}/instance-imports/${id.parse(importId)}/abort`, abortImportReceiptSchema, abortImportSchema.parse(body)) }
  instanceMembers(instance: ManagedInstance, signal?: AbortSignal) { return this.request(`/organisations/${id.parse(instance.organisation_id)}/instances/${id.parse(instance.id)}/members`, z.array(instanceMemberSchema), undefined, 'GET', signal) }
  instanceMemberCandidates(instance: ManagedInstance, signal?: AbortSignal) { return this.request(`/organisations/${id.parse(instance.organisation_id)}/instances/${id.parse(instance.id)}/members/candidates`, z.array(instanceMemberCandidateSchema), undefined, 'GET', signal) }
  setInstanceMember(instance: ManagedInstance, userId: string, role: InstanceMemberRole) { return this.request(`/organisations/${id.parse(instance.organisation_id)}/instances/${id.parse(instance.id)}/members`, instanceMemberRequestSchema, instanceMemberRequestSchema.parse({ user_id: userId, role })) }
  removeInstanceMember(instance: ManagedInstance, userId: string) { return this.request(`/organisations/${id.parse(instance.organisation_id)}/instances/${id.parse(instance.id)}/members/${id.parse(userId)}`, z.object({ user_id: id, removed: z.boolean() }), undefined, 'DELETE') }
  backups(instance: ManagedInstance, signal?: AbortSignal) { return this.request(`/organisations/${id.parse(instance.organisation_id)}/instances/${id.parse(instance.id)}/backups`, z.array(instanceBackupSchema), undefined, 'GET', signal) }
  releases(instance: ManagedInstance, signal?: AbortSignal) { return this.request(`/organisations/${id.parse(instance.organisation_id)}/instances/${id.parse(instance.id)}/releases`, z.array(instanceReleaseSchema), undefined, 'GET', signal) }
  rollbacks(instance: ManagedInstance, signal?: AbortSignal) { return this.request(`/organisations/${id.parse(instance.organisation_id)}/instances/${id.parse(instance.id)}/rollbacks`, z.array(instanceRollbackSchema), undefined, 'GET', signal) }
  rollouts(org: string, signal?: AbortSignal) { return this.request(`/organisations/${id.parse(org)}/rollouts`, z.array(rolloutSchema), undefined, 'GET', signal) }
  createRollout(org: string, request: CreateRollout) { return this.request(`/organisations/${id.parse(org)}/rollouts`, z.object({ id, state: z.string() }), createRolloutSchema.parse(request)) }
  controlRollout(rollout: InstanceRollout, action: RolloutControl) { return this.request(`/organisations/${id.parse(rollout.organisation_id)}/rollouts/${id.parse(rollout.id)}/control`, z.object({ id, state: z.string() }), controlRolloutSchema.parse({ action, expected_revision: rollout.revision })) }
  createOrganisation(name: string) { return this.request('/organisations', z.object({ id }), { name }) }
  executionCapacity(org:string,signal?:AbortSignal) {return this.request(`/organisations/${id.parse(org)}/execution-capacity`,executionCapacitySchema,undefined,'GET',signal)}
  setExecutionBudget(org:string,body:SetExecutionBudget) {return this.request(`/organisations/${id.parse(org)}/execution-capacity`,executionBudgetSchema,setExecutionBudgetSchema.parse(body),'PUT')}
  recoverExecutions(instance:ManagedInstance,reservations:string[]) {
    return this.request(`/organisations/${id.parse(instance.organisation_id)}/instances/${id.parse(instance.id)}/operations`,z.object({id,state:z.string()}),
      managementActionRequestSchema.parse({action:'stop',expected_revision:instance.revision,idempotency_key:crypto.randomUUID(),reconcile_reservation_ids:reservations,acknowledge_execution_stop:true}))
  }
  invitations(org: string, signal?: AbortSignal) { return this.request(`/organisations/${id.parse(org)}/invitations`, z.array(organisationInvitationSchema), undefined, 'GET', signal) }
  createInvitation(org: string, body: CreateOrganisationInvitation) { return this.request(`/organisations/${id.parse(org)}/invitations`, organisationInvitationSecretSchema, createOrganisationInvitationSchema.parse(body)) }
  revokeInvitation(org: string, invitation: string) { return this.request(`/organisations/${id.parse(org)}/invitations/${id.parse(invitation)}`, z.object({ id, state: z.literal('revoked') }), undefined, 'DELETE') }
  acceptInvitation(token: string) { return this.request('/invitations/accept', organisationInvitationAcceptedSchema, organisationInvitationTokenSchema.parse({ token })) }
  launch(instance: ManagedInstance) { return this.request(`/organisations/${id.parse(instance.organisation_id)}/instances/${id.parse(instance.id)}/launch`, z.object({ url: z.url(), email: z.email(), ticket: z.string() }), {}) }
  provisioningReleases(org:string,signal?:AbortSignal) { return this.request(`/organisations/${id.parse(org)}/provisioning-releases`,z.array(provisioningReleaseSchema),undefined,'GET',signal) }
  createInstance(org: string, name: string, backend: 'compose' | 'kubernetes', release?:string) { return this.request(`/organisations/${id.parse(org)}/instances`, z.object({ id, state: z.string() }), createInstanceSchema.parse({ name, backend, ...(release !== undefined ? {release} : {}) })) }
  policy(org: string, policy: EnterprisePolicy) { return this.request(`/organisations/${id.parse(org)}/policy`, enterprisePolicySchema, enterprisePolicySchema.parse(policy), 'PUT') }
  action(instance: ManagedInstance, action: InstanceAction, extra: { backup_id?: string; release?: string; qualification_only?: boolean; rollback_operation_id?: string; confirmation_name?: string; purge_backups?: boolean; acknowledge_execution_enrollment?:boolean } = {}) {
    return this.request(`/organisations/${instance.organisation_id}/instances/${instance.id}/operations`, z.object({ id, state: z.string() }), {
      action, expected_revision: instance.revision, idempotency_key: crypto.randomUUID(), backup_id: extra.backup_id ?? null, release: extra.release ?? null,
      ...(action === 'delete' ? { confirmation_name: extra.confirmation_name, purge_backups: extra.purge_backups ?? false } : {}),
      ...(action === 'rollback' ? { rollback_operation_id: extra.rollback_operation_id } : {}),
      ...(action === 'upgrade' ? { qualification_only: extra.qualification_only ?? false } : {}),
      ...(action === 'enroll_execution' ? { acknowledge_execution_enrollment:extra.acknowledge_execution_enrollment ?? false } : {}),
    })
  }
}
