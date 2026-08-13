import {
  messageMetadataForDaemon,
  useChatStore,
  type ChatMessage,
} from '../stores/chatStore'
import {
  crewDefinitionForDaemon,
  crewFromDaemonDefinition,
  useCrewStore,
  type Crew,
} from '../stores/crewStore'
import {
  mcpMetadataForDaemon,
  mcpServerFromDaemonMetadata,
  providerProfileFromDaemonMetadata,
  providerProfileMetadataForDaemon,
  secretMetadataForProviderProfile,
  useConfigStore,
  type LlmProfile,
  type McpServerConfig,
} from '../stores/configStore'
import { useMemoryStore } from '../stores/memoryStore'
import { useProjectStore } from '../stores/projectStore'
import { useSkillStore } from '../stores/skillStore'
import { useTaskStore } from '../stores/taskStore'
import {
  normalizeTask as normalizeWorkTask,
  useWorkTasksStore,
  workTaskMetadataForDaemon,
  type WorkTask,
} from '../stores/workTasksStore'
import { safeInvoke } from '../utils/safeInvoke'
import { hydrateStoredMessage, serializeChatMessageForStorage } from '../utils/chatMessages'
import {
  type LocalDaemonEntity,
  type LocalDaemonRuntimeClient,
} from './localDaemonClient'
import { createLocalDaemonRuntimeClient, mcpDeviceBindingForDaemon } from './localDaemonExecution'

type LegacyTask = {
  id: string
  title: string
  prompt: string
  status: string
  thread_id: string | null
  created_at: string
  updated_at: string
  error: string | null
}

type LegacyMemory = {
  id: string
  scope: string
  scope_ref: string | null
  category: string
  key: string
  content: string
  source_run_id: string | null
  confidence: number
}

type LegacyProfile = {
  id: string
  key: string
  value: string
  source: string
  confidence: number
}

type LegacySkill = {
  id: string
  name: string
  description: string
  prompt_template: string
  trigger_pattern: string | null
  run_mode: string
  auto_generated: boolean
  parent_skill_id: string | null
  source_task_ids: string | null
}

type LegacyProject = {
  id: string
  title: string
  instructions?: string
  threadIds?: string[]
  thread_ids?: string[]
  createdAt?: string
  created_at?: string
  updatedAt?: string
  updated_at?: string
}

type LegacyThread = {
  id: string
  title: string
  createdAt?: string
  created_at?: string
  updatedAt?: string
  updated_at?: string
  providerSettingsJson?: string | null
  provider_settings_json?: string | null
  permissionConfigJson?: string | null
  permission_config_json?: string | null
  runner?: string | null
  crewId?: string | null
  crew_id?: string | null
}

type LegacyMessage = {
  id: string
  role: string
  content: string
  timestamp: number
}

type LegacyWorkTask = Partial<WorkTask> & Record<string, unknown>

function stringValue(value: unknown, fallback = ''): string {
  return typeof value === 'string' ? value : fallback
}

function nullableString(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value : null
}

function numberValue(value: unknown, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback
}

function stringArray(value: unknown): string[] {
  return Array.isArray(value)
    ? Array.from(new Set(value.filter((entry): entry is string => (
        typeof entry === 'string' && entry.trim().length > 0
      ))))
    : []
}

function objectValue(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : undefined
}

function parseObject(value: string | null | undefined): Record<string, unknown> | undefined {
  if (!value?.trim()) return undefined
  try {
    return objectValue(JSON.parse(value))
  } catch {
    return undefined
  }
}

function chatRole(value: unknown): ChatMessage['role'] {
  return value === 'user' || value === 'assistant' || value === 'system' ? value : 'system'
}

function taskStatusForDaemon(status: string): string {
  if (status === 'created' || status === 'planned') return 'pending'
  if (status === 'cancelled') return 'canceled'
  return status
}

function taskStatusForDesktop(status: unknown): string {
  if (status === 'pending') return 'created'
  if (status === 'running') return 'planned'
  if (status === 'canceled') return 'cancelled'
  return typeof status === 'string' ? status : 'created'
}

async function seedMissingEntities(
  client: LocalDaemonRuntimeClient,
  existing: Map<string, Set<string>>,
): Promise<void> {
  const [tasks, memories, profile, skills, projects, threads, workTasks] = await Promise.all([
    safeInvoke<LegacyTask[]>('db_list_tasks', undefined, []),
    safeInvoke<LegacyMemory[]>('memory_search', {
      scope: null,
      scopeRef: null,
      category: null,
      keyword: null,
      limit: 10_000,
    }, []),
    safeInvoke<LegacyProfile[]>('user_profile_list', undefined, []),
    safeInvoke<LegacySkill[]>('skill_list', { limit: 10_000 }, []),
    safeInvoke<LegacyProject[]>('project_list', undefined, []),
    safeInvoke<LegacyThread[]>('db_list_threads', undefined, []),
    safeInvoke<LegacyWorkTask[]>('work_task_list', undefined, []),
  ])
  for (const raw of workTasks) {
    const task = normalizeWorkTask(raw)
    if (!task || existing.get('task')?.has(task.id)) continue
    await client.upsertEntity({
      entity_type: 'task',
      id: task.id,
      payload: workTaskMetadataForDaemon(task),
    })
  }
  for (const crew of useCrewStore.getState().crews) {
    if (existing.get('crew')?.has(crew.id)) continue
    await client.upsertEntity({
      entity_type: 'crew',
      id: crew.id,
      payload: crewDefinitionForDaemon(crew),
    })
  }
  const config = useConfigStore.getState()
  for (const profile of config.llmProfiles) {
    if (!existing.get('provider_profile')?.has(profile.id)) {
      await client.upsertEntity({
        entity_type: 'provider_profile',
        id: profile.id,
        payload: providerProfileMetadataForDaemon(profile),
      })
    }
    const secretId = `provider:${profile.id}`
    if (!existing.get('secret_metadata')?.has(secretId)) {
      await client.upsertEntity({
        entity_type: 'secret_metadata',
        id: secretId,
        payload: secretMetadataForProviderProfile(profile),
      })
    }
  }
  for (const server of config.mcpServers) {
    const id = server.id?.trim() || server.name
    if (!existing.get('mcp_metadata')?.has(id)) {
      await client.upsertEntity({
        entity_type: 'mcp_metadata',
        id,
        payload: mcpMetadataForDaemon(server),
      })
    }
    const deviceBinding = mcpDeviceBindingForDaemon(server)
    await client.upsertMcpBinding(deviceBinding.id, deviceBinding.binding)
  }

  for (const thread of threads) {
    if (!existing.get('thread')?.has(thread.id)) {
      await client.upsertEntity({
        entity_type: 'thread',
        id: thread.id,
        payload: {
          title: thread.title,
          provider_settings: parseObject(
            thread.providerSettingsJson ?? thread.provider_settings_json,
          ),
          runner: thread.runner === 'crew' ? 'crew' : 'model',
          crew_id: thread.runner === 'crew' ? thread.crewId ?? thread.crew_id ?? null : null,
          created_at: thread.createdAt ?? thread.created_at,
          updated_at: thread.updatedAt ?? thread.updated_at,
          migrated_from_desktop: true,
        },
      })
    }

    const messages = await safeInvoke<LegacyMessage[]>(
      'db_list_messages',
      { threadId: thread.id },
      [],
    )
    for (const record of messages) {
      if (existing.get('message')?.has(record.id)) continue
      const message = hydrateStoredMessage(record)
      await client.upsertEntity({
        entity_type: 'message',
        id: record.id,
        payload: messageMetadataForDaemon(thread.id, message),
      })
    }
  }

  for (const project of projects) {
    if (existing.get('project')?.has(project.id)) continue
    await client.upsertEntity({
      entity_type: 'project',
      id: project.id,
      payload: {
        title: project.title,
        instructions: project.instructions ?? '',
        thread_ids: project.threadIds ?? project.thread_ids ?? [],
        project_kind: 'private',
        files_location: 'personal_device',
        created_at: project.createdAt ?? project.created_at,
        updated_at: project.updatedAt ?? project.updated_at,
        migrated_from_desktop: true,
      },
    })
  }

  for (const task of tasks) {
    if (existing.get('task')?.has(task.id)) continue
    await client.upsertEntity({
      entity_type: 'task',
      id: task.id,
      payload: {
        task_kind: 'plan',
        title: task.title,
        description: task.prompt,
        status: taskStatusForDaemon(task.status),
        note: task.error,
        thread_id: task.thread_id,
        migrated_from_desktop: true,
      },
    })
  }
  for (const memory of memories) {
    if (existing.get('memory')?.has(memory.id)) continue
    await client.upsertEntity({
      entity_type: 'memory',
      id: memory.id,
      payload: {
        scope: memory.scope,
        scope_ref: memory.scope_ref,
        category: memory.category,
        key: memory.key,
        content: memory.content,
        target: memory.scope === 'user' ? 'user' : 'memory',
        source_run_id: memory.source_run_id,
        confidence: memory.confidence,
        migrated_from_desktop: true,
      },
    })
  }
  for (const entry of profile) {
    const id = `profile:${entry.id || entry.key}`
    if (existing.get('memory')?.has(id)) continue
    await client.upsertEntity({
      entity_type: 'memory',
      id,
      payload: {
        scope: 'user',
        scope_ref: null,
        category: 'profile',
        key: entry.key,
        content: entry.value,
        target: 'user',
        source: entry.source,
        confidence: entry.confidence,
        migrated_from_desktop: true,
      },
    })
  }
  for (const skill of skills) {
    if (existing.get('skill')?.has(skill.id)) continue
    await client.upsertEntity({
      entity_type: 'skill',
      id: skill.id,
      payload: {
        name: skill.name,
        description: skill.description,
        prompt_template: skill.prompt_template,
        trigger_pattern: skill.trigger_pattern,
        run_mode: skill.run_mode,
        auto_generated: skill.auto_generated,
        parent_skill_id: skill.parent_skill_id,
        source_task_ids: skill.source_task_ids,
        migrated_from_desktop: true,
      },
    })
  }
}

async function applyTaskEntity(entity: LocalDaemonEntity): Promise<void> {
  if (entity.payload.task_kind === 'work') {
    if (entity.tombstone) {
      await safeInvoke('work_task_delete', { id: entity.id }, undefined)
      return
    }
    const localRows = await safeInvoke<LegacyWorkTask[]>('work_task_list', undefined, [])
    const local = localRows
      .map((row) => normalizeWorkTask(row, false))
      .find((task) => task?.id === entity.id)
    const payload = entity.payload
    const runner = payload.runner === 'crew' ? 'crew' : 'model'
    await safeInvoke('work_task_upsert', {
      request: {
        id: entity.id,
        title: stringValue(payload.title),
        prompt: stringValue(payload.description),
        expectedOutput: stringValue(payload.expected_output),
        workDir: local?.workDir ?? '',
        threadId: nullableString(payload.thread_id),
        runner,
        crewId: runner === 'crew' ? nullableString(payload.crew_id) : null,
        model: runner === 'model' ? stringValue(payload.model) : '',
        backendSelection: runner === 'model' ? objectValue(payload.backend_selection) : undefined,
        scheduleExpr: stringValue(payload.schedule_expression),
        scheduleEnabled: payload.schedule_enabled === true,
        status: local?.status ?? 'idle',
        output: local?.output ?? null,
        error: local?.error ?? null,
        lastRunAt: local?.lastRunAt ? new Date(local.lastRunAt).toISOString() : null,
        createdAt: stringValue(payload.created_at, entity.created_at),
        updatedAt: entity.updated_at,
      },
    }, undefined)
    return
  }
  if (entity.tombstone) return
  const payload = entity.payload
  await safeInvoke('db_save_task', {
    id: entity.id,
    title: stringValue(payload.title, 'Untitled task'),
    prompt: stringValue(payload.description),
    status: taskStatusForDesktop(payload.status),
    threadId: nullableString(payload.thread_id),
    createdAt: entity.created_at,
  })
  const status = taskStatusForDesktop(payload.status)
  if (status !== 'created') {
    await safeInvoke('db_update_task_status', { id: entity.id, status })
  }
}

async function applyMemoryEntity(entity: LocalDaemonEntity): Promise<void> {
  if (entity.tombstone) {
    await safeInvoke('memory_delete', { id: entity.id }, undefined)
    return
  }
  const payload = entity.payload
  if (payload.scope === 'user' && payload.category === 'profile') {
    await safeInvoke('user_profile_upsert', {
      id: entity.id.replace(/^profile:/, ''),
      key: stringValue(payload.key),
      value: stringValue(payload.content),
      source: stringValue(payload.source, 'daemon'),
      confidence: numberValue(payload.confidence, 1),
    })
    return
  }
  await safeInvoke('memory_upsert', {
    id: entity.id,
    scope: stringValue(payload.scope, 'agent'),
    scopeRef: nullableString(payload.scope_ref),
    category: stringValue(payload.category, 'context'),
    key: stringValue(payload.key, entity.id),
    content: stringValue(payload.content),
    sourceRunId: nullableString(payload.source_run_id),
    confidence: numberValue(payload.confidence, 1),
  })
}

async function applySkillEntity(entity: LocalDaemonEntity): Promise<void> {
  if (entity.tombstone) {
    await safeInvoke('skill_delete', { id: entity.id }, undefined)
    return
  }
  const payload = entity.payload
  await safeInvoke('skill_upsert', {
    id: entity.id,
    name: stringValue(payload.name),
    description: stringValue(payload.description),
    promptTemplate: stringValue(payload.prompt_template),
    triggerPattern: nullableString(payload.trigger_pattern),
    runMode: stringValue(payload.run_mode, 'execute'),
    autoGenerated: payload.auto_generated === true,
    parentSkillId: nullableString(payload.parent_skill_id),
    sourceTaskIds: nullableString(payload.source_task_ids),
  })
}

async function applyProjectEntity(entity: LocalDaemonEntity): Promise<void> {
  if (entity.tombstone) {
    await safeInvoke('project_delete', {
      projectId: entity.id,
      deleteThreads: false,
    }, undefined)
    return
  }

  const payload = entity.payload
  const desiredThreadIds = stringArray(payload.thread_ids)
  await safeInvoke('project_upsert', {
    request: {
      id: entity.id,
      title: stringValue(payload.title, 'Untitled project'),
      instructions: stringValue(payload.instructions),
      createdAt: stringValue(payload.created_at, entity.created_at),
      updatedAt: stringValue(payload.updated_at, entity.updated_at),
    },
  }, undefined)

  const localProjects = await safeInvoke<LegacyProject[]>('project_list', undefined, [])
  const localThreadIds = stringArray(
    localProjects.find((project) => project.id === entity.id)?.threadIds
      ?? localProjects.find((project) => project.id === entity.id)?.thread_ids,
  )
  for (const threadId of localThreadIds) {
    if (!desiredThreadIds.includes(threadId)) {
      await safeInvoke('project_detach_thread', { projectId: entity.id, threadId }, undefined)
    }
  }
  for (const threadId of desiredThreadIds) {
    if (!localThreadIds.includes(threadId)) {
      await safeInvoke('project_attach_thread', { projectId: entity.id, threadId }, undefined)
    }
  }
}

async function applyThreadEntity(
  entity: LocalDaemonEntity,
  localThreadIds: Set<string>,
): Promise<void> {
  if (entity.tombstone) {
    await safeInvoke('db_delete_thread', { id: entity.id }, undefined)
    localThreadIds.delete(entity.id)
    return
  }

  const payload = entity.payload
  const title = stringValue(payload.title, 'Untitled chat')
  const runner = payload.runner === 'crew' ? 'crew' : 'model'
  const crewId = runner === 'crew' ? nullableString(payload.crew_id) : null
  const providerSettings = objectValue(payload.provider_settings)
  if (!localThreadIds.has(entity.id)) {
    await safeInvoke('db_save_thread', {
      id: entity.id,
      title,
      createdAt: stringValue(payload.created_at, entity.created_at),
      providerSettingsJson: providerSettings ? JSON.stringify(providerSettings) : null,
      permissionConfigJson: null,
      runner,
      crewId,
    }, undefined)
    localThreadIds.add(entity.id)
  } else {
    await safeInvoke('db_update_thread_title', { id: entity.id, title }, undefined)
    await safeInvoke('db_update_thread_provider_settings', {
      id: entity.id,
      providerSettingsJson: providerSettings ? JSON.stringify(providerSettings) : null,
    }, undefined)
    await safeInvoke('db_update_thread_runner', {
      id: entity.id,
      runner,
      crewId,
    }, undefined)
  }
}

async function applyMessageEntity(
  entity: LocalDaemonEntity,
  localMessageIds: Set<string>,
  liveThreadIds: Set<string>,
): Promise<void> {
  if (entity.tombstone) {
    await safeInvoke('db_delete_messages', { ids: [entity.id] }, undefined)
    localMessageIds.delete(entity.id)
    return
  }

  const payload = entity.payload
  const threadId = stringValue(payload.thread_id)
  if (!threadId || !liveThreadIds.has(threadId)) return
  const message: ChatMessage = {
    id: entity.id,
    role: chatRole(payload.role),
    content: stringValue(payload.content),
    timestamp: numberValue(payload.timestamp, Date.parse(entity.created_at)),
    visibleInChat: typeof payload.visible_in_chat === 'boolean' ? payload.visible_in_chat : undefined,
    durableRunId: nullableString(payload.durable_run_id) ?? undefined,
    durableRunState: nullableString(payload.durable_run_state) ?? undefined,
    durableRequestId: nullableString(payload.durable_request_id) ?? undefined,
    durableRequestKind: payload.durable_request_kind === 'approval' || payload.durable_request_kind === 'input'
      ? payload.durable_request_kind
      : undefined,
    streaming: false,
  }
  const content = serializeChatMessageForStorage(message)
  if (localMessageIds.has(entity.id)) {
    await safeInvoke('db_update_message_content', { id: entity.id, content }, undefined)
  } else {
    await safeInvoke('db_save_message', {
      id: entity.id,
      threadId,
      role: message.role,
      content,
      timestamp: message.timestamp,
    }, undefined)
    localMessageIds.add(entity.id)
  }
}

function applyCrewEntities(entities: LocalDaemonEntity[]): void {
  const currentState = useCrewStore.getState()
  const currentById = new Map(currentState.crews.map((crew) => [crew.id, crew]))
  const tombstones = new Set(entities
    .filter((entity) => entity.tombstone)
    .map((entity) => entity.id))
  const daemonCrewIds = new Set(entities.map((entity) => entity.id))
  const nextCrews: Crew[] = currentState.crews.filter((crew) => (
    !daemonCrewIds.has(crew.id) && !tombstones.has(crew.id)
  ))
  for (const entity of entities) {
    if (entity.tombstone) continue
    const crew = crewFromDaemonDefinition(entity.payload, currentById.get(entity.id))
    if (crew) nextCrews.push(crew)
  }
  useCrewStore.setState({
    crews: nextCrews,
    activeCrewId: currentState.activeCrewId && nextCrews.some((crew) => crew.id === currentState.activeCrewId)
      ? currentState.activeCrewId
      : nextCrews[0]?.id ?? null,
  })
}

function applyProviderProfileEntities(entities: LocalDaemonEntity[]): void {
  const state = useConfigStore.getState()
  const currentById = new Map(state.llmProfiles.map((profile) => [profile.id, profile]))
  const tombstones = new Set(entities.filter((entity) => entity.tombstone).map((entity) => entity.id))
  const daemonIds = new Set(entities.map((entity) => entity.id))
  const nextProfiles: LlmProfile[] = state.llmProfiles.filter((profile) => (
    !daemonIds.has(profile.id) && !tombstones.has(profile.id)
  ))
  for (const entity of entities) {
    if (entity.tombstone) continue
    const profile = providerProfileFromDaemonMetadata(
      entity.id,
      entity.payload,
      currentById.get(entity.id),
    )
    if (profile) nextProfiles.push(profile)
  }
  const localOllama = nextProfiles.find((profile) => profile.id === state.defaultLlmProfileIds.ollama)
  useConfigStore.setState({
    llmProfiles: nextProfiles,
    ollama: localOllama
      ? {
          ...state.ollama,
          model: localOllama.model,
          timeoutMs: localOllama.timeoutMs,
          contextWindow: localOllama.contextWindow ?? state.ollama.contextWindow,
          temperature: localOllama.temperature ?? state.ollama.temperature,
        }
      : state.ollama,
  })
}

function applyMcpMetadataEntities(entities: LocalDaemonEntity[]): void {
  const state = useConfigStore.getState()
  const currentById = new Map(state.mcpServers.map((server) => [server.id?.trim() || server.name, server]))
  const tombstones = new Set(entities.filter((entity) => entity.tombstone).map((entity) => entity.id))
  const daemonIds = new Set(entities.map((entity) => entity.id))
  const nextServers: McpServerConfig[] = state.mcpServers.filter((server) => {
    const id = server.id?.trim() || server.name
    return !daemonIds.has(id) && !tombstones.has(id)
  })
  for (const entity of entities) {
    if (entity.tombstone) continue
    const server = mcpServerFromDaemonMetadata(entity.id, entity.payload, currentById.get(entity.id))
    if (server) nextServers.push(server)
  }
  if (nextServers.length === 0) return
  const activeMcpServerName = nextServers.some((server) => server.name === state.activeMcpServerName)
    ? state.activeMcpServerName
    : nextServers[0].name
  useConfigStore.setState({
    mcpServers: nextServers,
    activeMcpServerName,
    mcpServer: nextServers.find((server) => server.name === activeMcpServerName) ?? nextServers[0],
  })
}

let reconciliation: Promise<void> | null = null
const entityWriteQueues = new Map<string, Promise<unknown>>()
let crewMirroringInstalled = false
let applyingDurableCrewState = false
let configMirroringInstalled = false
let applyingDurableConfigState = false

export async function mirrorProviderDeviceBinding(
  profile: LlmProfile,
  client = createLocalDaemonRuntimeClient(),
): Promise<void> {
  const baseUrl = profile.baseUrl.trim()
  if (!baseUrl) return
  await client.upsertProviderBindingFromCredentials(profile.id, baseUrl)
}

export async function mirrorMcpDeviceBinding(
  server: McpServerConfig,
  client = createLocalDaemonRuntimeClient(),
): Promise<void> {
  if (!server.name.trim() || !server.command.trim()) return
  const deviceBinding = mcpDeviceBindingForDaemon(server)
  await client.upsertMcpBinding(deviceBinding.id, deviceBinding.binding)
}

function enqueueEntityWrite<T>(
  entityType: string,
  id: string,
  operation: () => Promise<T>,
): Promise<T> {
  const key = `${entityType}:${id}`
  const previous = entityWriteQueues.get(key) ?? Promise.resolve()
  const queued = previous.catch(() => undefined).then(operation)
  entityWriteQueues.set(key, queued)
  void queued.finally(() => {
    if (entityWriteQueues.get(key) === queued) entityWriteQueues.delete(key)
  }).catch(() => undefined)
  return queued
}

function installDurableCrewMirroring(): void {
  if (crewMirroringInstalled) return
  crewMirroringInstalled = true
  let previous = new Map(useCrewStore.getState().crews.map((crew) => [
    crew.id,
    JSON.stringify(crewDefinitionForDaemon(crew)),
  ]))
  useCrewStore.subscribe((state) => {
    const next = new Map(state.crews.map((crew) => [
      crew.id,
      JSON.stringify(crewDefinitionForDaemon(crew)),
    ]))
    if (applyingDurableCrewState) {
      previous = next
      return
    }
    for (const crew of state.crews) {
      if (previous.get(crew.id) === next.get(crew.id)) continue
      void mirrorDurableLocalEntity('crew', crew.id, crewDefinitionForDaemon(crew))
        .catch((error) => console.warn('[crewStore] Daemon crew mirror failed', error))
    }
    for (const id of previous.keys()) {
      if (next.has(id)) continue
      void tombstoneDurableLocalEntity('crew', id)
        .catch((error) => console.warn('[crewStore] Daemon crew tombstone failed', error))
    }
    previous = next
  })
}

function installDurableConfigMirroring(): void {
  if (configMirroringInstalled) return
  configMirroringInstalled = true
  const snapshot = () => {
    const state = useConfigStore.getState()
    return {
      providers: new Map(state.llmProfiles.map((profile) => [
        profile.id,
        JSON.stringify(providerProfileMetadataForDaemon(profile)),
      ])),
      bindings: new Map(state.llmProfiles.map((profile) => [
        profile.id,
        JSON.stringify({ base_url: profile.baseUrl, has_api_key: profile.hasApiKey === true }),
      ])),
      secrets: new Map(state.llmProfiles.map((profile) => [
        `provider:${profile.id}`,
        JSON.stringify(secretMetadataForProviderProfile(profile)),
      ])),
      mcp: new Map(state.mcpServers.map((server) => {
        const id = server.id?.trim() || server.name
        return [id, JSON.stringify(mcpMetadataForDaemon(server))]
      })),
      mcpBindings: new Map(state.mcpServers.map((server) => {
        const deviceBinding = mcpDeviceBindingForDaemon(server)
        return [deviceBinding.id, JSON.stringify(deviceBinding.binding)]
      })),
    }
  }
  let previous = snapshot()
  useConfigStore.subscribe((state) => {
    const next = snapshot()
    if (applyingDurableConfigState) {
      previous = next
      return
    }
    for (const profile of state.llmProfiles) {
      if (previous.providers.get(profile.id) !== next.providers.get(profile.id)) {
        void mirrorDurableLocalEntity('provider_profile', profile.id, providerProfileMetadataForDaemon(profile))
          .catch((error) => console.warn('[configStore] Daemon provider profile mirror failed', error))
      }
      if (previous.bindings.get(profile.id) !== next.bindings.get(profile.id)) {
        void mirrorProviderDeviceBinding(profile)
          .catch((error) => console.warn('[configStore] Daemon provider device binding failed', error))
      }
      const secretId = `provider:${profile.id}`
      if (previous.secrets.get(secretId) !== next.secrets.get(secretId)) {
        void mirrorDurableLocalEntity('secret_metadata', secretId, secretMetadataForProviderProfile(profile))
          .catch((error) => console.warn('[configStore] Daemon secret metadata mirror failed', error))
      }
    }
    for (const server of state.mcpServers) {
      const id = server.id?.trim() || server.name
      if (previous.mcp.get(id) !== next.mcp.get(id)) {
        void mirrorDurableLocalEntity('mcp_metadata', id, mcpMetadataForDaemon(server))
          .catch((error) => console.warn('[configStore] Daemon MCP metadata mirror failed', error))
      }
      if (previous.mcpBindings.get(id) !== next.mcpBindings.get(id)) {
        void mirrorMcpDeviceBinding(server)
          .catch((error) => console.warn('[configStore] Daemon MCP device binding failed', error))
      }
    }
    for (const id of previous.providers.keys()) {
      if (next.providers.has(id)) continue
      void tombstoneDurableLocalEntity('provider_profile', id).catch(() => undefined)
      void tombstoneDurableLocalEntity('secret_metadata', `provider:${id}`).catch(() => undefined)
      void createLocalDaemonRuntimeClient().deleteProviderBinding(id).catch(() => undefined)
    }
    for (const id of previous.mcp.keys()) {
      if (next.mcp.has(id)) continue
      void tombstoneDurableLocalEntity('mcp_metadata', id).catch(() => undefined)
      void createLocalDaemonRuntimeClient().deleteMcpBinding(id).catch(() => undefined)
    }
    previous = next
  })
}

export function mirrorDurableLocalEntity(
  entityType: string,
  id: string,
  payload: Record<string, unknown>,
): Promise<LocalDaemonEntity> {
  return enqueueEntityWrite(entityType, id, async () => {
    const client = createLocalDaemonRuntimeClient()
    await client.health()
    const current = (await client.listEntities(entityType, true))
      .find((entity) => entity.id === id)
    return client.upsertEntity({
      entity_type: entityType,
      id,
      payload,
      expected_revision: current?.revision ?? 0,
    })
  })
}

export function tombstoneDurableLocalEntity(
  entityType: string,
  id: string,
): Promise<LocalDaemonEntity | null> {
  return enqueueEntityWrite(entityType, id, async () => {
    const client = createLocalDaemonRuntimeClient()
    await client.health()
    const current = (await client.listEntities(entityType, true))
      .find((entity) => entity.id === id)
    if (!current || current.tombstone) return current ?? null
    return client.deleteEntity(entityType, id, current.revision)
  })
}

export function reconcileDurableLocalEntities(
  client = createLocalDaemonRuntimeClient(),
): Promise<void> {
  if (reconciliation) return reconciliation
  reconciliation = (async () => {
    await client.health()
    const initial = await Promise.all([
      client.listEntities('project', true),
      client.listEntities('thread', true),
      client.listEntities('message', true),
      client.listEntities('crew', true),
      client.listEntities('provider_profile', true),
      client.listEntities('secret_metadata', true),
      client.listEntities('mcp_metadata', true),
      client.listEntities('task', true),
      client.listEntities('memory', true),
      client.listEntities('skill', true),
    ])
    const existing = new Map<string, Set<string>>([
      ['project', new Set(initial[0].map((entity) => entity.id))],
      ['thread', new Set(initial[1].map((entity) => entity.id))],
      ['message', new Set(initial[2].map((entity) => entity.id))],
      ['crew', new Set(initial[3].map((entity) => entity.id))],
      ['provider_profile', new Set(initial[4].map((entity) => entity.id))],
      ['secret_metadata', new Set(initial[5].map((entity) => entity.id))],
      ['mcp_metadata', new Set(initial[6].map((entity) => entity.id))],
      ['task', new Set(initial[7].map((entity) => entity.id))],
      ['memory', new Set(initial[8].map((entity) => entity.id))],
      ['skill', new Set(initial[9].map((entity) => entity.id))],
    ])
    await seedMissingEntities(client, existing)
    const [
      projects,
      threads,
      messages,
      crews,
      providerProfiles,
      _secretMetadata,
      mcpMetadata,
      tasks,
      memories,
      skills,
      localThreads,
    ] = await Promise.all([
      client.listEntities('project', true),
      client.listEntities('thread', true),
      client.listEntities('message', true),
      client.listEntities('crew', true),
      client.listEntities('provider_profile', true),
      client.listEntities('secret_metadata', true),
      client.listEntities('mcp_metadata', true),
      client.listEntities('task', true),
      client.listEntities('memory', true),
      client.listEntities('skill', true),
      safeInvoke<LegacyThread[]>('db_list_threads', undefined, []),
    ])
    for (const entity of projects) await applyProjectEntity(entity)
    const localThreadIds = new Set(localThreads.map((thread) => thread.id))
    for (const entity of threads) await applyThreadEntity(entity, localThreadIds)
    const liveThreadIds = new Set(threads
      .filter((entity) => !entity.tombstone)
      .map((entity) => entity.id))
    const localMessages = await Promise.all(Array.from(localThreadIds).map((threadId) => (
      safeInvoke<LegacyMessage[]>('db_list_messages', { threadId }, [])
    )))
    const localMessageIds = new Set(localMessages.flat().map((message) => message.id))
    for (const entity of messages) {
      await applyMessageEntity(entity, localMessageIds, liveThreadIds)
    }
    applyingDurableCrewState = true
    try {
      applyCrewEntities(crews)
    } finally {
      applyingDurableCrewState = false
    }
    applyingDurableConfigState = true
    try {
      applyProviderProfileEntities(providerProfiles)
      applyMcpMetadataEntities(mcpMetadata)
    } finally {
      applyingDurableConfigState = false
    }
    await Promise.all(useConfigStore.getState().llmProfiles.map((profile) => (
      mirrorProviderDeviceBinding(profile, client).catch((error) => {
        console.warn('[configStore] Initial daemon provider device binding failed', error)
      })
    )))
    for (const entity of tasks) await applyTaskEntity(entity)
    for (const entity of memories) await applyMemoryEntity(entity)
    for (const entity of skills) await applySkillEntity(entity)
    await Promise.all([
      useProjectStore.getState().loadFromDb(),
      useChatStore.getState().loadFromDb(),
      useTaskStore.getState().loadFromDb(),
      useWorkTasksStore.getState().loadFromDb(),
      useMemoryStore.getState().loadEntries(undefined, undefined, 10_000),
      useMemoryStore.getState().loadProfile(),
      useSkillStore.getState().loadSkills(10_000),
    ])
    installDurableCrewMirroring()
    installDurableConfigMirroring()
  })().finally(() => {
    reconciliation = null
  })
  return reconciliation
}
