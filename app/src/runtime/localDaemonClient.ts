import {
  capabilityCatalogSchema,
  listRunsResponseSchema,
  runEventSchema,
  runRecordSchema,
  SCHEMA_VERSION,
  type CapabilityCatalog,
  type CreateRunRequest,
  type RunEvent,
  type RunRecord,
  type VersionResponse,
} from './contracts'
import type { RuntimeClient, Unsubscribe } from './runtimeClient'

export interface LocalDaemonBridge {
  call(method: string, params?: unknown): Promise<unknown>
}

export interface LocalDaemonRuntimeOptions {
  bridge: LocalDaemonBridge
  capabilities: CapabilityCatalog
  pollIntervalMs?: number
}

export type LocalDaemonHealth = {
  status: string
  schema_version: number
  device_id: string
  daemon_version: string
}

export type LocalDaemonModelConfig = {
  base_url: string
  api_key?: string | null
  model: string
  timeout_ms: number
  max_steps?: number
  verify_tls_certificates: boolean
  mcp_servers?: Array<{
    name: string
    command: string
    args: string[]
    env: Record<string, string>
  }>
  crew_request?: Record<string, unknown> | null
  codex_request?: Record<string, unknown> | null
}

export type LocalDaemonSchedule = {
  id: string
  expression: string
  timezone: string
  enabled: boolean
  next_run_at: string | null
  last_triggered_at: string | null
  last_error: string | null
  created_at: string
  updated_at: string
}

export type LocalDaemonProviderBinding = {
  profile_id: string
  bound: boolean
  base_url?: string
  has_api_key?: boolean
  updated_at?: string
}

export type LocalDaemonMcpBinding = {
  server_id: string
  bound: boolean
  name?: string
  executable_hint?: string
  argument_count?: number
  environment_keys?: string[]
  updated_at?: string
}

export type LocalDaemonEntity = {
  entity_type: string
  id: string
  revision: number
  etag: string
  payload: Record<string, unknown>
  tombstone: boolean
  created_at: string
  updated_at: string
}

export type LocalDaemonSyncChange = {
  cursor: number
  entity_type: string
  entity_id: string
  revision: number
  operation: 'upsert' | 'delete'
  entity: LocalDaemonEntity
  created_at: string
}

/**
 * RuntimeClient adapter for the per-user Rust daemon.
 *
 * The Tauri shell owns the platform-specific named-pipe/Unix-socket bridge and
 * injects it here. Browser builds never receive such a bridge and therefore
 * cannot accidentally access local files or model endpoints.
 */
export class LocalDaemonRuntimeClient implements RuntimeClient {
  readonly kind = 'local' as const
  readonly #bridge: LocalDaemonBridge
  readonly #capabilities: CapabilityCatalog
  readonly #pollIntervalMs: number

  constructor(options: LocalDaemonRuntimeOptions) {
    this.#bridge = options.bridge
    this.#capabilities = capabilityCatalogSchema.parse(options.capabilities)
    this.#pollIntervalMs = options.pollIntervalMs ?? 750
  }

  async version(): Promise<VersionResponse> {
    const health = await this.health()
    return {
      api_version: 'v1',
      schema_version: health.schema_version ?? SCHEMA_VERSION,
      minimum_compatible_schema_version: SCHEMA_VERSION,
      build_version: health.daemon_version,
    }
  }

  async health(): Promise<LocalDaemonHealth> {
    const health = await this.#bridge.call('health') as Partial<LocalDaemonHealth>
    if (
      health.status !== 'ok'
      || typeof health.schema_version !== 'number'
      || typeof health.device_id !== 'string'
      || typeof health.daemon_version !== 'string'
    ) {
      throw new Error('The local daemon returned an invalid health response')
    }
    return health as LocalDaemonHealth
  }

  async capabilities(): Promise<CapabilityCatalog> {
    return this.#capabilities
  }

  async createRun(request: CreateRunRequest): Promise<RunRecord> {
    return runRecordSchema.parse(await this.#bridge.call('runs.create', request))
  }

  async createConfiguredRun(
    request: CreateRunRequest,
    modelConfig: LocalDaemonModelConfig,
  ): Promise<RunRecord> {
    return runRecordSchema.parse(await this.#bridge.call('runs.create', {
      ...request,
      model_config: {
        ...modelConfig,
        max_steps: modelConfig.max_steps ?? 64,
      },
    }))
  }

  async bindProjectWorkspace(projectId: string, workspacePath: string): Promise<void> {
    await this.#bridge.call('projects.bind_workspace', {
      project_id: projectId,
      workspace_path: workspacePath,
    })
  }

  async upsertProviderBinding(
    profileId: string,
    baseUrl: string,
    apiKey: string | null,
  ): Promise<LocalDaemonProviderBinding> {
    return await this.#bridge.call('provider_bindings.upsert', {
      profile_id: profileId,
      base_url: baseUrl,
      api_key: apiKey,
    }) as LocalDaemonProviderBinding
  }

  async getProviderBinding(profileId: string): Promise<LocalDaemonProviderBinding> {
    return await this.#bridge.call('provider_bindings.get', {
      profile_id: profileId,
    }) as LocalDaemonProviderBinding
  }

  async deleteProviderBinding(profileId: string): Promise<boolean> {
    const result = await this.#bridge.call('provider_bindings.delete', {
      profile_id: profileId,
    }) as { deleted?: boolean }
    return result.deleted === true
  }

  async upsertMcpBinding(
    serverId: string,
    binding: { name: string; command: string; args: string[]; env: Record<string, string> },
  ): Promise<LocalDaemonMcpBinding> {
    return await this.#bridge.call('mcp_bindings.upsert', {
      server_id: serverId,
      ...binding,
    }) as LocalDaemonMcpBinding
  }

  async getMcpBinding(serverId: string): Promise<LocalDaemonMcpBinding> {
    return await this.#bridge.call('mcp_bindings.get', {
      server_id: serverId,
    }) as LocalDaemonMcpBinding
  }

  async deleteMcpBinding(serverId: string): Promise<boolean> {
    const result = await this.#bridge.call('mcp_bindings.delete', {
      server_id: serverId,
    }) as { deleted?: boolean }
    return result.deleted === true
  }

  async resolveApproval(runId: string, approvalId: string, approved: boolean): Promise<void> {
    await this.#bridge.call('runs.approvals.resolve', {
      run_id: runId,
      approval_id: approvalId,
      decision: approved ? 'approved' : 'rejected',
    })
  }

  async respondToInput(runId: string, inputId: string, response: unknown): Promise<void> {
    await this.#bridge.call('runs.input_requests.respond', {
      run_id: runId,
      input_id: inputId,
      response,
    })
  }

  async upsertSchedule(request: {
    id: string
    expression: string
    timezone: string
    enabled: boolean
    run_request: CreateRunRequest
    model_config: LocalDaemonModelConfig
  }): Promise<LocalDaemonSchedule> {
    return await this.#bridge.call('schedules.upsert', request) as LocalDaemonSchedule
  }

  async listSchedules(): Promise<LocalDaemonSchedule[]> {
    return await this.#bridge.call('schedules.list') as LocalDaemonSchedule[]
  }

  async deleteSchedule(scheduleId: string): Promise<boolean> {
    const result = await this.#bridge.call('schedules.delete', { schedule_id: scheduleId }) as { deleted?: boolean }
    return result.deleted === true
  }

  async runScheduleNow(scheduleId: string): Promise<RunRecord> {
    return runRecordSchema.parse(await this.#bridge.call('schedules.run_now', {
      schedule_id: scheduleId,
    }))
  }

  async listRuns(limit = 100): Promise<RunRecord[]> {
    return listRunsResponseSchema.parse(await this.#bridge.call('runs.list', {
      limit: Math.max(1, Math.min(200, limit)),
    })).items
  }

  async listActiveRuns(): Promise<RunRecord[]> {
    return listRunsResponseSchema.parse(await this.#bridge.call('runs.list_active')).items
  }

  async upsertEntity(input: {
    entity_type: string
    id: string
    payload: Record<string, unknown>
    expected_revision?: number | null
  }): Promise<LocalDaemonEntity> {
    return await this.#bridge.call('entities.upsert', input) as LocalDaemonEntity
  }

  async listEntities(
    entityType: string,
    includeTombstones = false,
  ): Promise<LocalDaemonEntity[]> {
    return await this.#bridge.call('entities.list', {
      entity_type: entityType,
      include_tombstones: includeTombstones,
    }) as LocalDaemonEntity[]
  }

  async deleteEntity(
    entityType: string,
    id: string,
    expectedRevision?: number | null,
  ): Promise<LocalDaemonEntity> {
    return await this.#bridge.call('entities.delete', {
      entity_type: entityType,
      id,
      expected_revision: expectedRevision ?? null,
    }) as LocalDaemonEntity
  }

  async listEntityChanges(after = 0, limit = 200): Promise<{
    changes: LocalDaemonSyncChange[]
    next_cursor: number
  }> {
    return await this.#bridge.call('entities.changes', {
      after: Math.max(0, Math.trunc(after)),
      limit: Math.max(1, Math.min(1000, Math.trunc(limit))),
    }) as { changes: LocalDaemonSyncChange[]; next_cursor: number }
  }

  async getRun(runId: string): Promise<RunRecord> {
    return runRecordSchema.parse(await this.#bridge.call('runs.get', { run_id: runId }))
  }

  async cancelRun(runId: string): Promise<RunRecord> {
    return runRecordSchema.parse(await this.#bridge.call('runs.cancel', { run_id: runId }))
  }

  subscribeRunEvents(
    runId: string,
    afterSequence: number,
    onEvent: (event: RunEvent) => void,
    onError?: (error: Error) => void,
  ): Unsubscribe {
    let stopped = false
    let sequence = afterSequence
    let timeout: number | undefined
    const poll = async () => {
      try {
        const raw = await this.#bridge.call('runs.events', {
          run_id: runId,
          after: sequence,
        })
        const events = runEventSchema.array().parse(raw)
        for (const event of events) {
          if (event.sequence <= sequence) continue
          sequence = event.sequence
          onEvent(event)
        }
        if (events.some((event) => {
          if (event.kind === 'completed' || event.kind === 'failed') return true
          if (event.kind !== 'state_changed' || !event.payload || typeof event.payload !== 'object') return false
          const target = (event.payload as Record<string, unknown>).to
          return target === 'completed' || target === 'failed' || target === 'canceled' || target === 'expired'
        })) {
          stopped = true
        }
      } catch (cause) {
        onError?.(cause instanceof Error ? cause : new Error(String(cause)))
      }
      if (!stopped) timeout = window.setTimeout(poll, this.#pollIntervalMs)
    }
    void poll()
    return () => {
      stopped = true
      if (timeout !== undefined) window.clearTimeout(timeout)
    }
  }
}
