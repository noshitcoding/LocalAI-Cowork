import {
  assertProtocolCompatible,
  approvalRequestSchema,
  authSessionRecordSchema,
  capabilityCatalogSchema,
  desktopSessionSchema,
  desktopStreamTicketSchema,
  executorRecordSchema,
  messageRecordSchema,
  terminalSessionTicketSchema,
  threadRecordSchema,
  threadMessageRunSchema,
  listRunsResponseSchema,
  operationsSnapshotSchema,
  pullSyncChangesResponseSchema,
  passkeyChallengeSchema,
  passkeyRecordSchema,
  projectRecordSchema,
  providerProfileSchema,
  quotaStatusSchema,
  reauthenticationGrantSchema,
  runArtifactSchema,
  pushConfigurationSchema,
  pushSubscriptionRecordSchema,
  runEventSchema,
  runRecordSchema,
  serverSyncChangeSchema,
  syncedEntityPageSchema,
  pushSyncChangesResponseSchema,
  runInputRequestSchema,
  scheduleRecordSchema,
  supportGrantRecordSchema,
  teamRecordSchema,
  taskDefinitionSchema,
  totpRecoveryCodesSchema,
  totpSetupSchema,
  totpStatusSchema,
  versionResponseSchema,
  type CapabilityCatalog,
  type ApprovalRequest,
  type AuthSessionRecord,
  type CreateRunRequest,
  type CreateThreadMessageRequest,
  type DesktopSession,
  type DesktopStreamTicket,
  type ExecutorTarget,
  type ExecutorRecord,
  type PersonalDeviceRemoteControlMode,
  type RunEvent,
  type RunArtifact,
  type PushConfiguration,
  type PushSubscriptionRecord,
  type ProjectRecord,
  type ProviderProfile,
  type QuotaScopeType,
  type QuotaStatus,
  type PasskeyRecord,
  type OperationsSnapshot,
  type PullSyncChangesResponse,
  type MessageRecord,
  type RunRecord,
  type PushSyncChangesResponse,
  type RunInputRequest,
  type ScheduleRecord,
  type ServerSyncChange,
  type SyncedEntityPage,
  type SetQuotaLimitsRequest,
  type SupportGrantRecord,
  type TeamRecord,
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
  ): Unsubscribe
}

export interface RemoteRuntimeOptions {
  baseUrl: string
  accessToken: () => string | Promise<string>
  fetch?: typeof globalThis.fetch
  reconnectDelayMs?: number
}

export class RemoteRuntimeClient implements RuntimeClient {
  readonly kind = 'remote' as const
  readonly #baseUrl: string
  readonly #accessToken: RemoteRuntimeOptions['accessToken']
  readonly #fetch: typeof globalThis.fetch
  readonly #reconnectDelayMs: number

  constructor(options: RemoteRuntimeOptions) {
    this.#baseUrl = normalizeServerUrl(options.baseUrl)
    this.#accessToken = options.accessToken
    this.#fetch = options.fetch ?? globalThis.fetch.bind(globalThis)
    this.#reconnectDelayMs = options.reconnectDelayMs ?? 1_000
  }

  async version(): Promise<VersionResponse> {
    const version = versionResponseSchema.parse(await this.#request('/api/v1/version'))
    assertProtocolCompatible(version)
    return version
  }

  async capabilities(): Promise<CapabilityCatalog> {
    return capabilityCatalogSchema.parse(await this.#request('/api/v1/capabilities'))
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

  async listRuns(limit = 100): Promise<RunRecord[]> {
    const response = listRunsResponseSchema.parse(
      await this.#request(`/api/v1/runs?limit=${Math.max(1, Math.min(200, limit))}`),
    )
    return response.items
  }

  async listProjects(): Promise<ProjectRecord[]> {
    return projectRecordSchema.array().parse(await this.#request('/api/v1/projects'))
  }

  async listTeams(): Promise<TeamRecord[]> {
    return teamRecordSchema.array().parse(await this.#request('/api/v1/teams'))
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
  ): Unsubscribe {
    const controller = new AbortController()
    void this.#consumeEvents(runId, afterSequence, controller.signal, onEvent, onError)
    return () => controller.abort()
  }

  async #request(path: string, init: RequestInit = {}, expectJson = true): Promise<unknown> {
    const response = await this.#authorizedFetch(path, init)
    if (!response.ok) throw await RuntimeHttpError.fromResponse(response)
    if (!expectJson || response.status === 204) return undefined
    return response.json() as Promise<unknown>
  }

  async #authorizedFetch(path: string, init: RequestInit = {}): Promise<Response> {
    const token = await this.#accessToken()
    const headers = new Headers(init.headers)
    headers.set('authorization', `Bearer ${token}`)
    if (init.body !== undefined && !headers.has('content-type')) {
      headers.set('content-type', 'application/json')
    }
    return this.#fetch(`${this.#baseUrl}${path}`, { ...init, headers })
  }

  async #consumeEvents(
    runId: string,
    initialSequence: number,
    signal: AbortSignal,
    onEvent: (event: RunEvent) => void,
    onError?: (error: Error) => void,
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
      buffer += decoder.decode(value, { stream: !done }).replaceAll('\r\n', '\n')
      let boundary = buffer.indexOf('\n\n')
      while (boundary >= 0) {
        const raw = buffer.slice(0, boundary)
        buffer = buffer.slice(boundary + 2)
        const frame = parseSseFrame(raw)
        if (frame) yield frame
        boundary = buffer.indexOf('\n\n')
      }
      if (done) return
    }
  } finally {
    reader.releaseLock()
  }
}

function parseSseFrame(raw: string): SseFrame | undefined {
  const frame: SseFrame = { data: '' }
  const data: string[] = []
  for (const line of raw.split('\n')) {
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
    (url.hostname === '127.0.0.1' || url.hostname === 'localhost' || url.hostname === '[::1]')
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
