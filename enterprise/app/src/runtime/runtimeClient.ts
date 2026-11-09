import { verifySecondFactorSchema, authenticationAssuranceSchema, type AuthenticationAssurance, instanceContentAccessSchema, type InstanceContentAccess } from './enterpriseContracts'
import {snapshotManifestSchema,snapshotUploadSessionSchema,snapshotChunkReceiptSchema} from './contractArtifactRegistry'
import type {SnapshotUploadRequest,SnapshotManifest,SnapshotUploadSession} from './snapshotUpload'
import {managedExecutionResourceSchema,type ManagedExecutionResource} from './managedExecutionResources'
import {
  assertProtocolCompatible,
  approvalRequestSchema,
  authSessionRecordSchema,
  capabilityCatalogSchema,
  desktopSessionSchema,
  desktopStreamTicketSchema,
  executorRecordSchema,
  invitationSecretSchema,
  messageRecordSchema,
  terminalSessionTicketSchema,
  threadRecordSchema,
  threadMessageRunSchema,
  threadMessageRunWithThreadSchema,
  threadScheduleSchema,
  listRunsResponseSchema,
  operationsSnapshotSchema,
  pullSyncChangesResponseSchema,
  passkeyChallengeSchema,
  passkeyRecordSchema,
  projectMergeReviewSchema,
  projectRecordSchema,
  projectVersionSchema,
  providerProfileSchema,
  quotaStatusSchema,
  reauthenticationGrantSchema,
  runArtifactSchema,
  pushConfigurationSchema,
  pushSubscriptionRecordSchema,
  runEventSchema,
  runRecordSchema,
  serverMcpBindingRecordSchema,
  serverSyncChangeSchema,
  syncedEntityPageSchema,
  pushSyncChangesResponseSchema,
  runInputRequestSchema,
  scheduleRecordSchema,
  supportGrantRecordSchema,
  teamRecordSchema,
  teamMemberRecordSchema,
  taskDefinitionSchema,
  totpRecoveryCodesSchema,
  totpSetupSchema,
  totpStatusSchema,
  versionResponseSchema,
  type CapabilityCatalog,
  type ApprovalRequest,
  type ApplyProjectMergeRequest,
  type AuthSessionRecord,
  type CreateRunRequest,
  type CreateProjectRequest,
  type CreateThreadMessageRequest,
  type CreateThreadRunRequest,
  type CreateThreadScheduleRequest,
  type DesktopSession,
  type DesktopStreamTicket,
  type ExecutorTarget,
  type ExecutorRecord,
  type PersonalDeviceRemoteControlMode,
  type InvitationSecret,
  type RunEvent,
  type RunArtifact,
  type PushConfiguration,
  type PushSubscriptionRecord,
  type ProjectRecord,
  type ProjectMergeReview,
  type ProjectVersion,
  type ProviderProfile,
  type QuotaScopeType,
  type QuotaStatus,
  type PasskeyRecord,
  type OperationsSnapshot,
  type PullSyncChangesResponse,
  type MessageRecord,
  type RunRecord,
  type ServerMcpBindingRecord,
  type SetServerMcpBindingRequest,
  type PushSyncChangesResponse,
  type RunInputRequest,
  type ScheduleRecord,
  type ServerSyncChange,
  type SyncedEntityPage,
  type SetQuotaLimitsRequest,
  type SupportGrantRecord,
  type TeamRecord,
  type TeamMemberRecord,
  type TeamRole,
  type SyncChange,
  type TaskDefinition,
  type TotpRecoveryCodes,
  type TotpSetup,
  type TotpStatus,
  type VersionResponse,
  type ReauthenticationGrant,
  type TerminalSessionTicket,
  type ThreadRecord,
  type ThreadMessageRun,
  type ThreadMessageRunWithThread,
  type ThreadSchedule,
  type UpdateProjectRequest,
  type UpdateThreadRequest,
} from './contracts'
import { createPasskey, webauthnAvailableForOrigin } from './webauthn'

export type RuntimeKind = 'local' | 'remote'
export type Unsubscribe = () => void

export interface RuntimeClient {
  readonly kind: RuntimeKind
  version(): Promise<VersionResponse>
  capabilities(): Promise<CapabilityCatalog>
  createRun(request: CreateRunRequest): Promise<RunRecord>
  listRuns(limit?: number): Promise<RunRecord[]>
  getRun(runId: string): Promise<RunRecord>
  cancelRun(runId: string): Promise<RunRecord>
  subscribeRunEvents(
    runId: string,
    afterSequence: number,
    onEvent: (event: RunEvent) => void,
    onError?: (error: Error) => void,
    onConnected?: () => void,
  ): Unsubscribe
}

export interface RemoteRuntimeOptions {
  baseUrl: string
  accessToken: () => string | Promise<string>
  fetch?: typeof globalThis.fetch
  reconnectDelayMs?: number
  requestTimeoutMs?: number
  transferTimeoutMs?: number
}

export class RuntimeRequestTimeoutError extends Error {
  readonly timeoutMs: number

  constructor(path: string, timeoutMs: number) {
    super(`Remote runtime request timed out after ${timeoutMs} ms: ${path}`)
    this.name = 'RuntimeRequestTimeoutError'
    this.timeoutMs = timeoutMs
  }
}

export class RemoteRuntimeClient implements RuntimeClient {
  get enterpriseSource(): string { return this.#baseUrl }
  readonly kind = 'remote' as const
  readonly #baseUrl: string
  readonly #accessToken: RemoteRuntimeOptions['accessToken']
  readonly #fetch: typeof globalThis.fetch
  readonly #reconnectDelayMs: number
  readonly #requestTimeoutMs: number
  readonly #transferTimeoutMs: number

  constructor(options: RemoteRuntimeOptions) {
    this.#baseUrl = normalizeServerUrl(options.baseUrl)
    this.#accessToken = options.accessToken
    this.#fetch = options.fetch ?? globalThis.fetch.bind(globalThis)
    this.#reconnectDelayMs = options.reconnectDelayMs ?? 1_000
    this.#requestTimeoutMs = Math.max(1, options.requestTimeoutMs ?? 30_000)
    this.#transferTimeoutMs = Math.max(this.#requestTimeoutMs, options.transferTimeoutMs ?? 5 * 60_000)
  }

  async version(): Promise<VersionResponse> {
    const version = versionResponseSchema.parse(await this.#request('/api/v1/version'))
    assertProtocolCompatible(version)
    return version
  }

  async capabilities(): Promise<CapabilityCatalog> {
    return capabilityCatalogSchema.parse(await this.#request('/api/v1/capabilities'))
  }

  async beginSnapshotUpload(request:SnapshotUploadRequest,signal?:AbortSignal):Promise<SnapshotUploadSession>{
    return snapshotUploadSessionSchema.parse(await this.#request('/api/v1/snapshots',{method:'POST',body:JSON.stringify(request),signal}))
  }
  async snapshotUploadStatus(manifest:string):Promise<SnapshotUploadSession>{
    return snapshotUploadSessionSchema.parse(await this.#request(`/api/v1/snapshots/${encodeURIComponent(manifest)}/upload`))
  }
  async listManagedExecutionResources():Promise<ManagedExecutionResource[]>{
    const rows=await this.#request('/api/v1/managed-resources')
    if(!Array.isArray(rows))throw Error('Invalid managed resource catalog')
    return rows.map(row=>managedExecutionResourceSchema.parse(row))
  }
  async uploadSnapshotChunk(manifest:string,digest:string,bytes:Blob,signal?:AbortSignal){
    const response=await this.#authorizedFetch(`/api/v1/snapshots/${encodeURIComponent(manifest)}/chunks/${encodeURIComponent(digest)}`,{method:'PUT',headers:{'content-type':'application/octet-stream'},body:bytes,signal},this.#transferTimeoutMs)
    if(!response.ok)throw await RuntimeHttpError.fromResponse(response)
    const receipt=snapshotChunkReceiptSchema.parse(await response.json())
    if(receipt.manifest_id!==manifest||receipt.digest!==digest||receipt.plaintext_size!==bytes.size)throw Error('Snapshot chunk receipt differs from the original upload')
    return receipt
  }
  async commitSnapshot(manifest:string,signal?:AbortSignal):Promise<SnapshotManifest>{
    return snapshotManifestSchema.parse(await this.#request(`/api/v1/snapshots/${encodeURIComponent(manifest)}/commit`,{method:'POST',signal}))
  }

  async setPersonalDeviceRemoteControl(
    executor: ExecutorRecord,
    mode: PersonalDeviceRemoteControlMode,
  ): Promise<ExecutorRecord> {
    if (executor.registration.kind !== 'personal_device') {
      throw new Error('Remote-control modes can only be changed for personal devices')
    }
    return executorRecordSchema.parse(await this.#request('/api/v1/executors', {
      method: 'POST',
      body: JSON.stringify({
        ...executor.registration,
        owner_user_id: null,
        personal_device_remote_control: mode,
      }),
    }))
  }

  async createRun(request: CreateRunRequest): Promise<RunRecord> {
    return runRecordSchema.parse(
      await this.#request('/api/v1/runs', {
        method: 'POST',
        body: JSON.stringify(request),
      }),
    )
  }

  async enterprise(path: string, method: 'GET' | 'POST' = 'GET', body?: unknown,expectedUser?:string): Promise<unknown> {
    if (!/^\/(?:context|device-grants|transfer-grants|transfers|workflows|operations)(?:\/[0-9a-f-]{36}(?:\/(?:stop|continue|reconcile|revoke|reject|reassign|steps\/[0-9a-f-]{36}))?)?$/.test(path)) {
      throw new Error('Invalid enterprise operation path')
    }
    const serialized = body === undefined ? undefined : JSON.stringify(body)
    if (serialized && new TextEncoder().encode(serialized).byteLength > 1024 * 1024) throw new Error('Enterprise request exceeds its bound')
    if(method==='POST'&&!expectedUser)throw new Error('Original enterprise actor required')
    return this.#request('/api/v1/enterprise' + path, { method, body: serialized, cache: 'no-store',headers:expectedUser?{'x-cowork-enterprise-user':expectedUser}:undefined })
  }
  async listExecutors(): Promise<ExecutorRecord[]> {
    return executorRecordSchema.array().parse(await this.#request('/api/v1/executors'))
  }

  async listRuns(limit = 100): Promise<RunRecord[]> {
    const response = listRunsResponseSchema.parse(
      await this.#request(`/api/v1/runs?limit=${Math.max(1, Math.min(200, limit))}`),
    )
    return response.items
  }

  async listProjects(): Promise<ProjectRecord[]> {
    return projectRecordSchema.array().parse(await this.#request('/api/v1/projects'))
  }

  async createProject(request: CreateProjectRequest): Promise<ProjectRecord> {
    return projectRecordSchema.parse(await this.#request('/api/v1/projects', {
      method: 'POST', body: JSON.stringify(request),
    }))
  }

  async listProjectVersions(projectId: string): Promise<ProjectVersion[]> {
    return projectVersionSchema.array().parse(await this.#request(
      `/api/v1/projects/${encodeURIComponent(projectId)}/versions`,
    ))
  }

  async applyProjectVersion(
    projectId: string,
    versionId: string,
    expectedProjectRevision: number,
    expectedCurrentVersionId: string | null,
  ): Promise<ProjectVersion> {
    return projectVersionSchema.parse(await this.#request(
      `/api/v1/projects/${encodeURIComponent(projectId)}/versions/${encodeURIComponent(versionId)}/apply`,
      {
        method: 'POST',
        body: JSON.stringify({
          expected_project_revision: expectedProjectRevision,
          expected_current_version_id: expectedCurrentVersionId,
        }),
      },
    ))
  }

  async reviewProjectMerge(
    projectId: string,
    baseVersionId: string,
    currentVersionId: string,
    resultVersionId: string,
  ): Promise<ProjectMergeReview> {
    const query = new URLSearchParams({
      base_version_id: baseVersionId,
      current_version_id: currentVersionId,
      result_version_id: resultVersionId,
    })
    return projectMergeReviewSchema.parse(await this.#request(
      `/api/v1/projects/${encodeURIComponent(projectId)}/merge-review?${query.toString()}`,
    ))
  }

  async applyProjectMerge(
    projectId: string,
    request: ApplyProjectMergeRequest,
  ): Promise<ProjectVersion> {
    return projectVersionSchema.parse(await this.#request(
      `/api/v1/projects/${encodeURIComponent(projectId)}/merge-apply`,
      { method: 'POST', body: JSON.stringify(request) },
    ))
  }

  async listTeams(): Promise<TeamRecord[]> {
    return teamRecordSchema.array().parse(await this.#request('/api/v1/teams'))
  }

  async createTeam(name: string): Promise<TeamRecord> {
    return teamRecordSchema.parse(await this.#request('/api/v1/teams', {
      method: 'POST', body: JSON.stringify({ name }),
    }))
  }

  async createInvitation(email: string): Promise<InvitationSecret> {
    return invitationSecretSchema.parse(await this.#request('/api/v1/auth/invitations', {
      method: 'POST', body: JSON.stringify({ email, expires_at: null }),
    }))
  }

  async listTeamMembers(teamId: string): Promise<TeamMemberRecord[]> {
    return teamMemberRecordSchema.array().parse(await this.#request(
      `/api/v1/teams/${encodeURIComponent(teamId)}/members`,
    ))
  }

  async setTeamMember(teamId: string, userId: string, role: Exclude<TeamRole, 'owner'>): Promise<void> {
    await this.#request(`/api/v1/teams/${encodeURIComponent(teamId)}/members`, {
      method: 'POST', body: JSON.stringify({ user_id: userId, role }),
    }, false)
  }

  async removeTeamMember(teamId: string, userId: string): Promise<void> {
    await this.#request(
      `/api/v1/teams/${encodeURIComponent(teamId)}/members/${encodeURIComponent(userId)}`,
      { method: 'DELETE' },
      false,
    )
  }

  async listProviderProfiles(): Promise<ProviderProfile[]> {
    return providerProfileSchema.array().parse(
      await this.#request('/api/v1/provider-profiles'),
    )
  }

  async createProviderProfile(request: {
    team_id: string | null
    name: string
    provider_kind: string
    model_defaults: unknown
    api_key: string | null
  }): Promise<ProviderProfile> {
    return providerProfileSchema.parse(await this.#request('/api/v1/provider-profiles', {
      method: 'POST', body: JSON.stringify(request),
    }))
  }

  async updateProviderProfile(profileId: string, request: {
    expected_revision: number
    name: string
    provider_kind: string
    model_defaults: unknown
    api_key?: string | null
  }): Promise<ProviderProfile> {
    return providerProfileSchema.parse(await this.#request(
      `/api/v1/provider-profiles/${encodeURIComponent(profileId)}`,
      { method: 'PUT', body: JSON.stringify(request) },
    ))
  }

  async setProviderProfileSecret(
    profileId: string,
    expectedRevision: number,
    apiKey: string | null,
  ): Promise<ProviderProfile> {
    return providerProfileSchema.parse(await this.#request(
      `/api/v1/provider-profiles/${encodeURIComponent(profileId)}/secret`,
      {
        method: 'PUT',
        body: JSON.stringify({ expected_revision: expectedRevision, api_key: apiKey }),
      },
    ))
  }

  async deleteProviderProfile(profileId: string, expectedRevision: number): Promise<void> {
    await this.#request(
      `/api/v1/provider-profiles/${encodeURIComponent(profileId)}?expected_revision=${Math.max(1, Math.trunc(expectedRevision))}`,
      { method: 'DELETE' },
      false,
    )
  }

  async listServerMcpBindings(projectId: string): Promise<ServerMcpBindingRecord[]> {
    return serverMcpBindingRecordSchema.array().parse(await this.#request(
      `/api/v1/projects/${encodeURIComponent(projectId)}/mcp-bindings`,
    ))
  }

  async setServerMcpBinding(
    projectId: string,
    mcpEntityId: string,
    request: SetServerMcpBindingRequest,
  ): Promise<ServerMcpBindingRecord> {
    return serverMcpBindingRecordSchema.parse(await this.#request(
      `/api/v1/projects/${encodeURIComponent(projectId)}/mcp-bindings/${encodeURIComponent(mcpEntityId)}`,
      { method: 'PUT', body: JSON.stringify(request) },
    ))
  }

  async deleteServerMcpBinding(
    projectId: string,
    mcpEntityId: string,
    expectedRevision: number,
  ): Promise<void> {
    await this.#request(
      `/api/v1/projects/${encodeURIComponent(projectId)}/mcp-bindings/${encodeURIComponent(mcpEntityId)}?expected_revision=${Math.max(1, Math.trunc(expectedRevision))}`,
      { method: 'DELETE' },
      false,
    )
  }

  async updateProject(projectId: string, request: UpdateProjectRequest): Promise<ProjectRecord> {
    return projectRecordSchema.parse(await this.#request(
      `/api/v1/projects/${encodeURIComponent(projectId)}`,
      { method: 'PUT', body: JSON.stringify(request) },
    ))
  }

  async deleteProject(projectId: string, expectedRevision: number): Promise<void> {
    await this.#request(
      `/api/v1/projects/${encodeURIComponent(projectId)}?expected_revision=${Math.max(0, Math.trunc(expectedRevision))}`,
      { method: 'DELETE' },
      false,
    )
  }

  async createThread(projectId: string, title: string): Promise<ThreadRecord> {
    return threadRecordSchema.parse(await this.#request('/api/v1/threads', {
      method: 'POST',
      body: JSON.stringify({
        project_id: projectId,
        title,
        forked_from_thread_id: null,
        forked_from_message_id: null,
      }),
    }))
  }

  async createThreadRun(request: CreateThreadRunRequest): Promise<ThreadMessageRunWithThread> {
    return threadMessageRunWithThreadSchema.parse(await this.#request('/api/v1/thread-runs', {
      method: 'POST',
      body: JSON.stringify(request),
    }))
  }

  async updateThread(threadId: string, request: UpdateThreadRequest): Promise<ThreadRecord> {
    return threadRecordSchema.parse(await this.#request(
      `/api/v1/threads/${encodeURIComponent(threadId)}`,
      { method: 'PUT', body: JSON.stringify(request) },
    ))
  }

  async deleteThread(threadId: string, expectedRevision: number): Promise<void> {
    await this.#request(
      `/api/v1/threads/${encodeURIComponent(threadId)}?expected_revision=${Math.max(0, Math.trunc(expectedRevision))}`,
      { method: 'DELETE' },
      false,
    )
  }

  async createThreadMessage(
    threadId: string,
    request: CreateThreadMessageRequest,
  ): Promise<ThreadMessageRun> {
    return threadMessageRunSchema.parse(await this.#request(
      `/api/v1/threads/${encodeURIComponent(threadId)}/messages`,
      { method: 'POST', body: JSON.stringify(request) },
    ))
  }

  async listThreadMessages(threadId: string, limit = 100): Promise<MessageRecord[]> {
    return messageRecordSchema.array().parse(await this.#request(
      `/api/v1/threads/${encodeURIComponent(threadId)}/messages?limit=${Math.max(1, Math.min(1_000, limit))}`,
    ))
  }

  async listProjectThreads(projectId: string): Promise<ThreadRecord[]> {
    return threadRecordSchema.array().parse(await this.#request(
      `/api/v1/projects/${encodeURIComponent(projectId)}/threads`,
    ))
  }

  async pushSyncChanges(changes: SyncChange[]): Promise<PushSyncChangesResponse> {
    return pushSyncChangesResponseSchema.parse(await this.#request('/api/v1/sync/changes', {
      method: 'POST', body: JSON.stringify({ changes }),
    }))
  }

  async pullSyncChanges(after = 0, limit = 200): Promise<PullSyncChangesResponse> {
    return pullSyncChangesResponseSchema.parse(await this.#request(
      `/api/v1/sync/changes?after=${Math.max(0, Math.trunc(after))}&limit=${Math.max(1, Math.min(1_000, Math.trunc(limit)))}`,
    ))
  }

  subscribeSyncEvents(
    afterCursor: number,
    onEvent: (event: ServerSyncChange) => void,
    onError?: (error: Error) => void,
  ): Unsubscribe {
    const controller = new AbortController()
    void this.#consumeSyncEvents(afterCursor, controller.signal, onEvent, onError)
    return () => controller.abort()
  }

  async listSyncedEntities(
    entityType: string,
    after?: string | null,
    limit = 200,
  ): Promise<SyncedEntityPage> {
    const query = new URLSearchParams({ limit: String(Math.max(1, Math.min(1_000, limit))) })
    if (after) query.set('after', after)
    return syncedEntityPageSchema.parse(await this.#request(
      `/api/v1/sync/entities/${encodeURIComponent(entityType)}?${query.toString()}`,
    ))
  }

  async getContentAccess(): Promise<InstanceContentAccess> {
    return instanceContentAccessSchema.parse(await this.#request('/api/v1/content-access'))
  }

  async listSupportGrants(): Promise<SupportGrantRecord[]> {
    return supportGrantRecordSchema.array().parse(await this.#request('/api/v1/support-grants'))
  }

  async createSupportGrant(request: {
    support_user_id: string
    project_id: string | null
    thread_id: string | null
    reason: string
    expires_at: string
  }): Promise<SupportGrantRecord> {
    return supportGrantRecordSchema.parse(await this.#request('/api/v1/support-grants', {
      method: 'POST', body: JSON.stringify(request),
    }))
  }

  async revokeSupportGrant(grantId: string): Promise<void> {
    await this.#request(`/api/v1/support-grants/${encodeURIComponent(grantId)}`, {
      method: 'DELETE',
    }, false)
  }

  async getQuota(scopeType: QuotaScopeType, scopeId: string): Promise<QuotaStatus> {
    return quotaStatusSchema.parse(await this.#request(
      `/api/v1/quotas/${scopeType}/${encodeURIComponent(scopeId)}`,
    ))
  }

  async setQuota(
    scopeType: QuotaScopeType,
    scopeId: string,
    request: SetQuotaLimitsRequest,
  ): Promise<QuotaStatus> {
    return quotaStatusSchema.parse(await this.#request(
      `/api/v1/quotas/${scopeType}/${encodeURIComponent(scopeId)}`,
      { method: 'PUT', body: JSON.stringify(request) },
    ))
  }

  async operationsMetrics(): Promise<OperationsSnapshot> {
    return operationsSnapshotSchema.parse(await this.#request('/api/v1/operations/metrics'))
  }

  async downloadSupportBundle(): Promise<Blob> {
    const response = await this.#authorizedFetch('/api/v1/operations/support-bundle')
    if (!response.ok) throw await RuntimeHttpError.fromResponse(response)
    return response.blob()
  }

  async listTasks(projectId: string): Promise<TaskDefinition[]> {
    return taskDefinitionSchema.array().parse(await this.#request(
      `/api/v1/tasks?project_id=${encodeURIComponent(projectId)}`,
    ))
  }

  async createTask(request: {
    project_id: string
    name: string
    instructions: string
    required_capabilities: string[]
    default_target: ExecutorTarget | null
    config: unknown
    release: boolean
  }): Promise<TaskDefinition> {
    return taskDefinitionSchema.parse(await this.#request('/api/v1/tasks', {
      method: 'POST', body: JSON.stringify(request),
    }))
  }

  async createTaskVersion(taskId: string, request: {
    base_revision: number
    name: string
    instructions: string
    required_capabilities: string[]
    default_target: ExecutorTarget | null
    config: unknown
    release: boolean
  }): Promise<TaskDefinition> {
    return taskDefinitionSchema.parse(await this.#request(
      `/api/v1/tasks/${encodeURIComponent(taskId)}/versions`,
      { method: 'POST', body: JSON.stringify(request) },
    ))
  }

  async releaseTaskVersion(taskId: string, revision: number): Promise<TaskDefinition> {
    return taskDefinitionSchema.parse(await this.#request(
      `/api/v1/tasks/${encodeURIComponent(taskId)}/release`,
      { method: 'POST', body: JSON.stringify({ revision }) },
    ))
  }

  async deleteTask(taskId: string, expectedRevision: number): Promise<void> {
    await this.#request(
      `/api/v1/tasks/${encodeURIComponent(taskId)}?expected_revision=${Math.max(1, Math.trunc(expectedRevision))}`,
      { method: 'DELETE' },
      false,
    )
  }

  async listSchedules(projectId: string): Promise<ScheduleRecord[]> {
    return scheduleRecordSchema.array().parse(await this.#request(
      `/api/v1/schedules?project_id=${encodeURIComponent(projectId)}`,
    ))
  }

  async listAuthSessions(): Promise<AuthSessionRecord[]> {
    return authSessionRecordSchema.array().parse(await this.#request('/api/v1/auth/sessions'))
  }

  async revokeAuthSession(sessionId: string): Promise<void> {
    await this.#request(`/api/v1/auth/sessions/${encodeURIComponent(sessionId)}`, {
      method: 'DELETE',
    }, false)
  }

  async createSchedule(request: {
    task_id: string
    project_id: string
    thread_id: string
    cron: string
    timezone: string
    executor_target: ExecutorTarget
    input: unknown
    model_profile_id: string | null
    enabled: boolean
  }): Promise<ScheduleRecord> {
    return scheduleRecordSchema.parse(await this.#request('/api/v1/schedules', {
      method: 'POST',
      body: JSON.stringify(request),
    }))
  }

  async createThreadSchedule(request: CreateThreadScheduleRequest): Promise<ThreadSchedule> {
    return threadScheduleSchema.parse(await this.#request('/api/v1/thread-schedules', {
      method: 'POST', body: JSON.stringify(request),
    }))
  }

  async updateSchedule(
    scheduleId: string,
    request: {
      expected_revision: number
      cron: string
      timezone: string
      executor_target: ExecutorTarget
      input: unknown
      model_profile_id: string | null
      enabled: boolean
    },
  ): Promise<ScheduleRecord> {
    return scheduleRecordSchema.parse(await this.#request(
      `/api/v1/schedules/${encodeURIComponent(scheduleId)}`,
      { method: 'PUT', body: JSON.stringify(request) },
    ))
  }

  async deleteSchedule(scheduleId: string): Promise<void> {
    await this.#request(
      `/api/v1/schedules/${encodeURIComponent(scheduleId)}`,
      { method: 'DELETE' },
      false,
    )
  }

  async authenticationAssurance(organisation?:string,stepUp=false): Promise<AuthenticationAssurance> {
    const query=new URLSearchParams();if(organisation)query.set('organisation',organisation);if(stepUp)query.set('step_up','true')
    return authenticationAssuranceSchema.parse(await this.#request(`/api/v1/auth/assurance${query.size?'?'+query:''}`))
  }
  async verifySecondFactor(code: string,organisation?:string,stepUp=false): Promise<AuthenticationAssurance> {
    const query=new URLSearchParams();if(organisation)query.set('organisation',organisation);if(stepUp)query.set('step_up','true')
    return authenticationAssuranceSchema.parse(await this.#request(`/api/v1/auth/assurance/verify${query.size?'?'+query:''}`, { method:'POST',body:JSON.stringify(verifySecondFactorSchema.parse({ code })) }))
  }

  async totpStatus(): Promise<TotpStatus> {
    return totpStatusSchema.parse(await this.#request('/api/v1/auth/totp'))
  }

  async setupTotp(): Promise<TotpSetup> {
    return totpSetupSchema.parse(await this.#request('/api/v1/auth/totp/setup', { method: 'POST' }))
  }

  async enableTotp(code: string): Promise<TotpRecoveryCodes> {
    return totpRecoveryCodesSchema.parse(await this.#request('/api/v1/auth/totp/enable', {
      method: 'POST', body: JSON.stringify({ code }),
    }))
  }

  async regenerateRecoveryCodes(code: string): Promise<TotpRecoveryCodes> {
    return totpRecoveryCodesSchema.parse(await this.#request('/api/v1/auth/totp/recovery-codes', {
      method: 'POST', body: JSON.stringify({ code }),
    }))
  }

  async disableTotp(password: string, secondFactor: string): Promise<void> {
    await this.#request('/api/v1/auth/totp/disable', {
      method: 'POST', body: JSON.stringify({ password, second_factor: secondFactor }),
    }, false)
  }

  async listPasskeys(): Promise<PasskeyRecord[]> {
    return passkeyRecordSchema.array().parse(await this.#request('/api/v1/auth/passkeys'))
  }

  passkeysAvailableInContext(): boolean {
    return webauthnAvailableForOrigin(this.#baseUrl)
  }

  async registerPasskey(label: string): Promise<PasskeyRecord> {
    const challenge = passkeyChallengeSchema.parse(await this.#request(
      '/api/v1/auth/passkeys/register/start',
      { method: 'POST' },
    ))
    const credential = await createPasskey(challenge.public_key, this.#baseUrl)
    return passkeyRecordSchema.parse(await this.#request(
      '/api/v1/auth/passkeys/register/finish',
      {
        method: 'POST',
        body: JSON.stringify({
          challenge_id: challenge.challenge_id,
          label,
          credential,
        }),
      },
    ))
  }

  async removePasskey(passkeyId: string, password: string, secondFactor?: string): Promise<void> {
    await this.#request(`/api/v1/auth/passkeys/${encodeURIComponent(passkeyId)}`, {
      method: 'DELETE',
      body: JSON.stringify({ password, second_factor: secondFactor?.trim() || null }),
    }, false)
  }

  async getRun(runId: string): Promise<RunRecord> {
    return runRecordSchema.parse(await this.#request(`/api/v1/runs/${encodeURIComponent(runId)}`))
  }

  async cancelRun(runId: string): Promise<RunRecord> {
    return runRecordSchema.parse(
      await this.#request(`/api/v1/runs/${encodeURIComponent(runId)}/cancel`, {
        method: 'POST',
      }),
    )
  }

  async listArtifacts(runId: string): Promise<RunArtifact[]> {
    return runArtifactSchema.array().parse(
      await this.#request(`/api/v1/runs/${encodeURIComponent(runId)}/artifacts`),
    )
  }

  async downloadArtifact(runId: string, artifactId: string): Promise<Blob> {
    const response = await this.#authorizedFetch(
      `/api/v1/runs/${encodeURIComponent(runId)}/artifacts/${encodeURIComponent(artifactId)}`,
      {},
      this.#transferTimeoutMs,
    )
    if (!response.ok) throw await RuntimeHttpError.fromResponse(response)
    return response.blob()
  }

  async uploadAttachment(runId: string, file: File): Promise<void> {
    if (file.size === 0) throw new Error('Attachment must not be empty')
    if (file.size > 64 * 1024 * 1024) throw new Error('Attachment exceeds the 64 MiB limit')
    const query = new URLSearchParams({ name: file.name })
    const response = await this.#authorizedFetch(
      `/api/v1/runs/${encodeURIComponent(runId)}/attachments?${query.toString()}`,
      {
        method: 'POST',
        headers: { 'content-type': file.type || 'application/octet-stream' },
        body: file,
      },
      this.#transferTimeoutMs,
    )
    if (!response.ok) throw await RuntimeHttpError.fromResponse(response)
  }

  async pushConfiguration(): Promise<PushConfiguration> {
    return pushConfigurationSchema.parse(await this.#request('/api/v1/push/config'))
  }

  async registerFcmPush(deviceId: string, token: string): Promise<PushSubscriptionRecord> {
    return pushSubscriptionRecordSchema.parse(await this.#request('/api/v1/push/subscriptions', {
      method: 'POST',
      body: JSON.stringify({ device_id: deviceId, provider: 'fcm', token }),
    }))
  }

  async registerWebPush(
    deviceId: string,
    subscription: { endpoint: string; p256dh: string; auth: string },
  ): Promise<PushSubscriptionRecord> {
    return pushSubscriptionRecordSchema.parse(await this.#request('/api/v1/push/subscriptions', {
      method: 'POST',
      body: JSON.stringify({ device_id: deviceId, provider: 'web_push', ...subscription }),
    }))
  }

  async listPushSubscriptions(): Promise<PushSubscriptionRecord[]> {
    return pushSubscriptionRecordSchema.array().parse(
      await this.#request('/api/v1/push/subscriptions'),
    )
  }

  async removePushSubscription(subscriptionId: string): Promise<void> {
    await this.#request(
      `/api/v1/push/subscriptions/${encodeURIComponent(subscriptionId)}`,
      { method: 'DELETE' },
      false,
    )
  }

  async listDesktopSessions(runId: string): Promise<DesktopSession[]> {
    return desktopSessionSchema.array().parse(
      await this.#request(`/api/v1/runs/${encodeURIComponent(runId)}/desktop-sessions`),
    )
  }

  async startDesktopSession(
    runId: string,
    dimensions: { width?: number; height?: number } = {},
  ): Promise<DesktopSession> {
    return desktopSessionSchema.parse(
      await this.#request(`/api/v1/runs/${encodeURIComponent(runId)}/desktop-sessions`, {
        method: 'POST',
        body: JSON.stringify(dimensions),
      }),
    )
  }

  async stopDesktopSession(runId: string, sessionId: string): Promise<void> {
    await this.#request(
      `/api/v1/runs/${encodeURIComponent(runId)}/desktop-sessions/${encodeURIComponent(sessionId)}`,
      { method: 'DELETE' },
      false,
    )
  }

  async reauthenticateDesktopControl(password: string): Promise<ReauthenticationGrant> {
    return reauthenticationGrantSchema.parse(
      await this.#request('/api/v1/auth/reauthenticate', {
        method: 'POST',
        body: JSON.stringify({ password, purpose: 'desktop_control' }),
      }),
    )
  }

  async createDesktopStreamTicket(
    runId: string,
    sessionId: string,
    control = false,
    reauthenticationToken?: string,
  ): Promise<DesktopStreamTicket> {
    return desktopStreamTicketSchema.parse(
      await this.#request(
        `/api/v1/runs/${encodeURIComponent(runId)}/desktop-sessions/${encodeURIComponent(sessionId)}/tickets`,
        {
          method: 'POST',
          body: JSON.stringify({
            control,
            reauthentication_token: reauthenticationToken ?? null,
          }),
        },
      ),
    )
  }

  desktopStreamUrl(sessionId: string, ticket: string): string {
    const url = new URL(
      `${this.#baseUrl}/api/v1/desktop-sessions/${encodeURIComponent(sessionId)}/stream`,
    )
    url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
    url.searchParams.set('ticket', ticket)
    return url.toString()
  }

  async createTerminalSession(
    runId: string,
    dimensions: { columns: number; rows: number },
  ): Promise<TerminalSessionTicket> {
    return terminalSessionTicketSchema.parse(
      await this.#request(`/api/v1/runs/${encodeURIComponent(runId)}/terminal-sessions`, {
        method: 'POST',
        body: JSON.stringify(dimensions),
      }),
    )
  }

  terminalStreamUrl(sessionId: string, ticket: string): string {
    const url = new URL(
      `${this.#baseUrl}/api/v1/terminal-sessions/${encodeURIComponent(sessionId)}/stream`,
    )
    url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
    url.searchParams.set('ticket', ticket)
    return url.toString()
  }

  async listApprovals(runId: string): Promise<ApprovalRequest[]> {
    return approvalRequestSchema.array().parse(
      await this.#request(`/api/v1/runs/${encodeURIComponent(runId)}/approvals`),
    )
  }

  async resolveApproval(
    runId: string,
    approvalId: string,
    expectedRevision: number,
    decision: 'approved' | 'rejected',
  ): Promise<ApprovalRequest> {
    return approvalRequestSchema.parse(await this.#request(
      `/api/v1/runs/${encodeURIComponent(runId)}/approvals/${encodeURIComponent(approvalId)}/resolve`,
      {
        method: 'POST',
        body: JSON.stringify({ expected_revision: expectedRevision, decision }),
      },
    ))
  }

  async listInputRequests(runId: string): Promise<RunInputRequest[]> {
    return runInputRequestSchema.array().parse(
      await this.#request(`/api/v1/runs/${encodeURIComponent(runId)}/input-requests`),
    )
  }

  async submitInputResponse(
    runId: string,
    inputId: string,
    expectedRevision: number,
    response: unknown,
  ): Promise<RunInputRequest> {
    return runInputRequestSchema.parse(await this.#request(
      `/api/v1/runs/${encodeURIComponent(runId)}/input-requests/${encodeURIComponent(inputId)}/respond`,
      {
        method: 'POST',
        body: JSON.stringify({ expected_revision: expectedRevision, response }),
      },
    ))
  }

  subscribeRunEvents(
    runId: string,
    afterSequence: number,
    onEvent: (event: RunEvent) => void,
    onError?: (error: Error) => void,
    onConnected?: () => void,
  ): Unsubscribe {
    const controller = new AbortController()
    void this.#consumeEvents(runId, afterSequence, controller.signal, onEvent, onError, onConnected)
    return () => controller.abort()
  }

  async #request(path: string, init: RequestInit = {}, expectJson = true): Promise<unknown> {
    const response = await this.#authorizedFetch(path, init)
    if (!response.ok) throw await RuntimeHttpError.fromResponse(response)
    if (!expectJson || response.status === 204) return undefined
    return response.json() as Promise<unknown>
  }

  async #authorizedFetch(
    path: string,
    init: RequestInit = {},
    timeoutMs = this.#requestTimeoutMs,
  ): Promise<Response> {
    const token = await this.#accessToken()
    const headers = new Headers(init.headers)
    headers.set('authorization', `Bearer ${token}`)
    if (init.body !== undefined && !headers.has('content-type')) {
      headers.set('content-type', 'application/json')
    }
    const controller = new AbortController()
    const callerSignal = init.signal
    let timeout: ReturnType<typeof setTimeout> | undefined
    let removeCallerAbortListener = () => {}
    const cancellation = new Promise<never>((_resolve, reject) => {
      const abortFromCaller = () => {
        controller.abort(callerSignal?.reason)
        reject(callerSignal?.reason instanceof Error
          ? callerSignal.reason
          : new DOMException('The remote runtime request was aborted.', 'AbortError'))
      }
      if (callerSignal?.aborted) {
        abortFromCaller()
        return
      }
      if (callerSignal) {
        callerSignal.addEventListener('abort', abortFromCaller, { once: true })
        removeCallerAbortListener = () => callerSignal.removeEventListener('abort', abortFromCaller)
      }
      timeout = setTimeout(() => {
        const error = new RuntimeRequestTimeoutError(path, timeoutMs)
        controller.abort(error)
        reject(error)
      }, timeoutMs)
    })
    const request = Promise.resolve().then(() => this.#fetch(
      `${this.#baseUrl}${path}`,
      { ...init, headers, signal: controller.signal, redirect: 'error' },
    ))
    try {
      return await Promise.race([request, cancellation])
    } finally {
      if (timeout) clearTimeout(timeout)
      removeCallerAbortListener()
    }
  }

  async #consumeEvents(
    runId: string,
    initialSequence: number,
    signal: AbortSignal,
    onEvent: (event: RunEvent) => void,
    onError?: (error: Error) => void,
    onConnected?: () => void,
  ): Promise<void> {
    let sequence = initialSequence
    while (!signal.aborted) {
      try {
        const token = await this.#accessToken()
        const response = await this.#fetch(
          `${this.#baseUrl}/api/v1/runs/${encodeURIComponent(runId)}/events`,
          {
            headers: {
              authorization: `Bearer ${token}`,
              accept: 'text/event-stream',
              'last-event-id': String(sequence),
            },
            signal,
          },
        )
        if (!response.ok) throw await RuntimeHttpError.fromResponse(response)
        if (!response.body) throw new Error('The server returned an empty event stream')
        if (signal.aborted) return
        onConnected?.()
        for await (const frame of parseSse(response.body, signal)) {
          if (frame.data === '' || frame.data === 'keep-alive') continue
          const event = runEventSchema.parse(JSON.parse(frame.data))
          if (event.sequence <= sequence) continue
          sequence = event.sequence
          onEvent(event)
        }
      } catch (cause) {
        if (signal.aborted) return
        const error = cause instanceof Error ? cause : new Error(String(cause))
        onError?.(error)
      }
      await abortableDelay(this.#reconnectDelayMs, signal)
    }
  }

  async #consumeSyncEvents(
    initialCursor: number,
    signal: AbortSignal,
    onEvent: (event: ServerSyncChange) => void,
    onError?: (error: Error) => void,
  ): Promise<void> {
    let cursor = Math.max(0, Math.trunc(initialCursor))
    while (!signal.aborted) {
      try {
        const token = await this.#accessToken()
        const response = await this.#fetch(`${this.#baseUrl}/api/v1/sync/events`, {
          headers: {
            authorization: `Bearer ${token}`,
            accept: 'text/event-stream',
            'last-event-id': String(cursor),
          },
          signal,
        })
        if (!response.ok) throw await RuntimeHttpError.fromResponse(response)
        if (!response.body) throw new Error('The server returned an empty sync event stream')
        for await (const frame of parseSse(response.body, signal)) {
          if (frame.data === '' || frame.data === 'keep-alive') continue
          const event = serverSyncChangeSchema.parse(JSON.parse(frame.data))
          if (event.cursor <= cursor) continue
          cursor = event.cursor
          onEvent(event)
        }
      } catch (cause) {
        if (signal.aborted) return
        onError?.(cause instanceof Error ? cause : new Error(String(cause)))
      }
      await abortableDelay(this.#reconnectDelayMs, signal)
    }
  }
}

export interface HybridRuntimeOptions {
  local: RuntimeClient
  remote?: RuntimeClient
}

export class HybridRuntimeClient {
  readonly #local: RuntimeClient
  readonly #remote?: RuntimeClient

  constructor(options: HybridRuntimeOptions) {
    this.#local = options.local
    this.#remote = options.remote
  }

  forTarget(target: ExecutorTarget): RuntimeClient {
    if (target.kind === 'personal_device') return this.#local
    if (!this.#remote) {
      throw new Error('This Open Cowork installation is not connected to a server')
    }
    return this.#remote
  }

  get local(): RuntimeClient {
    return this.#local
  }

  get remote(): RuntimeClient | undefined {
    return this.#remote
  }
}

interface SseFrame {
  id?: string
  event?: string
  data: string
}

async function* parseSse(
  stream: ReadableStream<Uint8Array>,
  signal: AbortSignal,
): AsyncGenerator<SseFrame> {
  const reader = stream.getReader()
  const decoder = new TextDecoder()
  let buffer = ''
  try {
    while (!signal.aborted) {
      const { done, value } = await reader.read()
      if (signal.aborted) return
      buffer += decoder.decode(value, { stream: !done })
      let boundary = findSseBoundary(buffer, done)
      while (boundary && !signal.aborted) {
        const raw = buffer.slice(0, boundary.index)
        buffer = buffer.slice(boundary.index + boundary.length)
        const frame = parseSseFrame(raw)
        if (frame) yield frame
        boundary = findSseBoundary(buffer, done)
      }
      if (done) return
    }
  } finally {
    reader.releaseLock()
  }
}

function sseLineEndingLength(buffer: string, index: number, atEndOfStream: boolean): number {
  if (buffer[index] === '\n') return 1
  if (buffer[index] !== '\r') return 0
  if (buffer[index + 1] === '\n') return 2
  return index + 1 < buffer.length || atEndOfStream ? 1 : 0
}

function findSseBoundary(
  buffer: string,
  atEndOfStream: boolean,
): { index: number; length: number } | null {
  for (let index = 0; index < buffer.length; index += 1) {
    const firstLength = sseLineEndingLength(buffer, index, atEndOfStream)
    if (firstLength === 0) continue
    const secondIndex = index + firstLength
    const secondLength = sseLineEndingLength(buffer, secondIndex, atEndOfStream)
    if (secondLength > 0) {
      return { index, length: firstLength + secondLength }
    }
    index = secondIndex - 1
  }
  return null
}

function parseSseFrame(raw: string): SseFrame | undefined {
  const frame: SseFrame = { data: '' }
  const data: string[] = []
  for (const line of raw.split(/\r\n|\r|\n/)) {
    if (line.startsWith(':')) continue
    const separator = line.indexOf(':')
    const field = separator < 0 ? line : line.slice(0, separator)
    const value = separator < 0 ? '' : line.slice(separator + 1).replace(/^ /, '')
    if (field === 'id') frame.id = value
    if (field === 'event') frame.event = value
    if (field === 'data') data.push(value)
  }
  frame.data = data.join('\n')
  return data.length > 0 ? frame : undefined
}

function abortableDelay(milliseconds: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    if (signal.aborted) return resolve()
    const timeout = window.setTimeout(resolve, milliseconds)
    signal.addEventListener(
      'abort',
      () => {
        window.clearTimeout(timeout)
        resolve()
      },
      { once: true },
    )
  })
}

export function normalizeServerUrl(value: string): string {
  const url = new URL(value)
  if (url.username || url.password) throw new Error('Server URLs must not contain credentials')
  if (url.protocol !== 'https:' && !isLoopback(url)) {
    throw new Error('Remote Open Cowork servers must use HTTPS')
  }
  url.pathname = url.pathname.replace(/\/$/, '')
  url.search = ''
  url.hash = ''
  return url.toString().replace(/\/$/, '')
}

function isLoopback(url: URL): boolean {
  return (
    url.protocol === 'http:' &&
    (url.hostname === '127.0.0.1' || url.hostname === 'localhost' || url.hostname === '[::1]' || /^[0-9a-f]{32}\.localhost$/.test(url.hostname))
  )
}

export class RuntimeHttpError extends Error {
  readonly status: number
  readonly code?: string

  constructor(status: number, message: string, code?: string) {
    super(message)
    this.name = 'RuntimeHttpError'
    this.status = status
    this.code = code
  }

  static async fromResponse(response: Response): Promise<RuntimeHttpError> {
    const text = await response.text()
    try {
      const body = JSON.parse(text) as { error?: string; message?: string }
      return new RuntimeHttpError(
        response.status,
        body.message ?? response.statusText,
        body.error,
      )
    } catch {
      return new RuntimeHttpError(response.status, text || response.statusText)
    }
  }
}
