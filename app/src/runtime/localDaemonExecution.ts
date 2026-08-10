import { crewProviderLocator, getCredential, llmApiKeyLocator } from '../security/credentialVault'
import type { ChatProviderState } from '../utils/chatProvider'
import { LocalDaemonRuntimeClient } from './localDaemonClient'
import type { RunEvent, RunRecord, RunState } from './contracts'
import { tauriLocalDaemonBridge } from './tauriLocalDaemonBridge'

const ENTITY_UUID_STORAGE_KEY = 'open-cowork-local-runtime-entity-uuids-v1'
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i

export type DurableLocalRunInput = {
  clientThreadId: string
  clientProjectId: string
  clientTaskId?: string | null
  assistantMessageId: string
  userMessageId?: string | null
  prompt: string
  systemPrompt?: string
  history?: Array<{ role: 'system' | 'user' | 'assistant'; content: string }>
  workspacePath?: string | null
  projectRevision?: number
  taskRevision?: number
  toolPolicy?: 'autonomous' | 'confirm_mutations' | 'read_only'
  provider: ChatProviderState
  mcpServers?: Array<{
    id?: string
    name: string
    command: string
    args: string
    env: Record<string, string>
  }>
  source: 'chat' | 'task'
}

export type DurableCrewRunInput = {
  clientThreadId: string
  clientProjectId: string
  clientTaskId?: string | null
  assistantMessageId: string
  crewLiveMessageId: string
  crewLiveTitle: string
  prompt: string
  workspacePath?: string | null
  projectRevision?: number
  taskRevision?: number
  crewId: string
  crewRequest: Record<string, unknown>
  source: 'chat' | 'task'
}

export type DurableCrewScheduleInput = Omit<
  DurableCrewRunInput,
  'assistantMessageId' | 'crewLiveMessageId' | 'source'
> & {
  scheduleClientId: string
  expression: string
  timezone: string
  enabled: boolean
}

export type DurableCodexRunInput = {
  clientThreadId: string
  clientProjectId: string
  clientTaskId?: string | null
  assistantMessageId: string
  userMessageId?: string | null
  prompt: string
  systemPrompt?: string
  history?: Array<{ role: 'system' | 'user' | 'assistant'; content: string }>
  workspacePath: string
  projectRevision?: number
  taskRevision?: number
  toolPolicy?: 'autonomous' | 'confirm_mutations' | 'read_only'
  profileId: string
  model?: string
  reasoningEffort?: string
  timeoutMs?: number
  source: 'chat' | 'task'
}

export type DurableCodexScheduleInput = Omit<
  DurableCodexRunInput,
  'assistantMessageId' | 'userMessageId' | 'source'
> & {
  scheduleClientId: string
  expression: string
  timezone: string
  enabled: boolean
}

export type DurableLocalScheduleInput = Omit<
  DurableLocalRunInput,
  'assistantMessageId' | 'userMessageId' | 'source'
> & {
  scheduleClientId: string
  expression: string
  timezone: string
  enabled: boolean
}

export type DurableLocalRunWatcher = {
  runId: string
  done: Promise<RunRecord>
  unsubscribe: () => void
}

export type DurableLocalRunCallbacks = {
  onEvent?: (event: RunEvent) => void
  onState?: (state: RunState, run: RunRecord) => void
  onError?: (error: Error) => void
}

function randomUuid(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID()
  }
  const bytes = new Uint8Array(16)
  if (typeof crypto !== 'undefined' && typeof crypto.getRandomValues === 'function') {
    crypto.getRandomValues(bytes)
  } else {
    for (let index = 0; index < bytes.length; index += 1) {
      bytes[index] = Math.floor(Math.random() * 256)
    }
  }
  bytes[6] = (bytes[6] & 0x0f) | 0x40
  bytes[8] = (bytes[8] & 0x3f) | 0x80
  const hex = Array.from(bytes, (value) => value.toString(16).padStart(2, '0')).join('')
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`
}

function parseCommandArguments(value: string): string[] {
  const matches = value.match(/"[^"]*"|'[^']*'|\S+/g) ?? []
  return matches.map((part) => part.replace(/^["']|["']$/g, ''))
}

export function mcpDeviceBindingForDaemon(server: NonNullable<DurableLocalRunInput['mcpServers']>[number]) {
  const name = server.name.trim()
  const command = server.command.trim()
  return {
    id: server.id?.trim() || name,
    binding: {
      name,
      command,
      args: parseCommandArguments(server.args),
      env: { ...server.env },
    },
  }
}

function configuredMcpBindings(input: Pick<DurableLocalRunInput, 'mcpServers'>) {
  return (input.mcpServers ?? [])
    .filter((server) => server.name.trim() && server.command.trim())
    .map(mcpDeviceBindingForDaemon)
}

function objectValue(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {}
}

async function hydrateCrewRequestCredentials(
  crewId: string,
  source: Record<string, unknown>,
): Promise<Record<string, unknown>> {
  const request = JSON.parse(JSON.stringify(source)) as Record<string, unknown>
  const providerConfigs = objectValue(request.providerConfigs)
  for (const [property, legacyProvider] of [
    ['openAICompatible', 'openai_compatible'],
    ['openRouter', 'openrouter'],
  ] as const) {
    const config = objectValue(providerConfigs[property])
    if (Object.keys(config).length === 0) continue
    const profileId = typeof config.profileId === 'string' ? config.profileId.trim() : ''
    const stored = profileId
      ? await getCredential(llmApiKeyLocator(profileId))
      : await getCredential(crewProviderLocator(crewId, legacyProvider))
    config.apiKey = stored ?? (typeof config.apiKey === 'string' ? config.apiKey : '')
    providerConfigs[property] = config
  }
  request.providerConfigs = providerConfigs
  return request
}

async function mirrorCrewProviderDeviceBindings(
  request: Record<string, unknown>,
  client: LocalDaemonRuntimeClient,
): Promise<string[]> {
  const providerConfigs = objectValue(request.providerConfigs)
  const profileIds = new Set<string>()
  for (const property of ['openAICompatible', 'openRouter'] as const) {
    const config = objectValue(providerConfigs[property])
    const profileId = typeof config.profileId === 'string' ? config.profileId.trim() : ''
    if (!profileId) continue
    profileIds.add(profileId)
    const baseUrl = typeof config.baseUrl === 'string' ? config.baseUrl.trim() : ''
    if (!baseUrl) continue
    const apiKey = typeof config.apiKey === 'string' && config.apiKey ? config.apiKey : null
    await client.upsertProviderBinding(profileId, baseUrl, apiKey)
  }
  return Array.from(profileIds)
}

function crewAdapterModelConfig(request: Record<string, unknown>) {
  const ollama = objectValue(request.config)
  const providers = objectValue(request.providerConfigs)
  const openAi = objectValue(providers.openAICompatible)
  const openRouter = objectValue(providers.openRouter)
  const selected = [ollama, openAi, openRouter].find((entry) => (
    typeof entry.baseUrl === 'string'
    && entry.baseUrl.trim()
    && typeof entry.model === 'string'
    && entry.model.trim()
  )) ?? ollama
  const timeout = typeof selected.timeoutMs === 'number' && Number.isFinite(selected.timeoutMs)
    ? Math.max(1_000, Math.min(24 * 60 * 60 * 1_000, Math.trunc(selected.timeoutMs)))
    : 600_000
  return {
    baseUrl: typeof selected.baseUrl === 'string' && selected.baseUrl.trim()
      ? selected.baseUrl.trim()
      : 'http://127.0.0.1:11434',
    model: typeof selected.model === 'string' && selected.model.trim()
      ? selected.model.trim()
      : 'crew-adapter',
    timeout,
    verifyTlsCertificates: selected.verifyTlsCertificates !== false,
  }
}

function readEntityUuids(): Record<string, string> {
  if (typeof window === 'undefined') return {}
  try {
    const parsed = JSON.parse(window.localStorage.getItem(ENTITY_UUID_STORAGE_KEY) ?? '{}')
    if (!parsed || typeof parsed !== 'object') return {}
    const entries = Object.entries(parsed).filter((entry): entry is [string, string] => (
      typeof entry[1] === 'string' && UUID_PATTERN.test(entry[1])
    ))
    return Object.fromEntries(entries)
  } catch {
    return {}
  }
}

export function localRuntimeEntityUuid(kind: 'thread' | 'project' | 'task', clientId: string): string {
  if (UUID_PATTERN.test(clientId)) return clientId
  const key = `${kind}:${clientId}`
  const current = readEntityUuids()
  if (current[key]) return current[key]
  const created = randomUuid()
  if (typeof window !== 'undefined') {
    window.localStorage.setItem(ENTITY_UUID_STORAGE_KEY, JSON.stringify({ ...current, [key]: created }))
  }
  return created
}

export function createLocalDaemonRuntimeClient(): LocalDaemonRuntimeClient {
  return new LocalDaemonRuntimeClient({
    bridge: tauriLocalDaemonBridge,
    capabilities: { schema_version: 2, server_linux: [], executors: [] },
  })
}

function modelCapability(provider: ChatProviderState): string {
  try {
    const endpoint = new URL(provider.endpoint)
    if (
      endpoint.port === '11434'
      || provider.preset === 'ollama'
      || /ollama/i.test(endpoint.hostname)
    ) return 'model.ollama'
    if (endpoint.port === '8000' || /vllm/i.test(endpoint.hostname)) return 'model.vllm'
  } catch {
    // The daemon performs authoritative URL validation and returns a useful error.
  }
  return 'model.external'
}

async function bindProviderForDaemon(
  provider: ChatProviderState,
  client: LocalDaemonRuntimeClient,
): Promise<string | null> {
  if (provider.profileId) {
    await client.upsertProviderBindingFromCredentials(provider.profileId, provider.endpoint)
    return null
  }
  return provider.apiKey || null
}

export async function createDurableLocalRun(
  input: DurableLocalRunInput,
  client = createLocalDaemonRuntimeClient(),
): Promise<{ client: LocalDaemonRuntimeClient; run: RunRecord }> {
  if (input.provider.provider !== 'openai-compatible') {
    throw new Error('The persistent local daemon currently requires an OpenAI-compatible model profile')
  }
  const health = await client.health()
  const threadId = localRuntimeEntityUuid('thread', input.clientThreadId)
  const projectId = localRuntimeEntityUuid('project', input.clientProjectId)
  const taskId = input.clientTaskId
    ? localRuntimeEntityUuid('task', input.clientTaskId)
    : null
  const workspacePath = input.workspacePath?.trim() || null
  if (workspacePath) await client.bindProjectWorkspace(projectId, workspacePath)

  const apiKey = await bindProviderForDaemon(input.provider, client)
  const requiredCapabilities = [modelCapability(input.provider)]
  if (workspacePath) requiredCapabilities.push('files', 'shell')
  const mcpBindings = configuredMcpBindings(input)
  await Promise.all(mcpBindings.map(({ id, binding }) => client.upsertMcpBinding(id, binding)))
  const mcpServers = mcpBindings.map(({ binding }) => binding)
  if (mcpServers.length > 0) requiredCapabilities.push('tool.mcp.invoke')

  const run = await client.createConfiguredRun({
    thread_id: threadId,
    project_id: projectId,
    project_revision: Math.max(1, Math.trunc(input.projectRevision ?? 1)),
    project_privacy: 'private_local',
    task: taskId ? { id: taskId, revision: Math.max(1, Math.trunc(input.taskRevision ?? 1)) } : null,
    executor_target: { kind: 'personal_device', device_id: health.device_id },
    required_capabilities: requiredCapabilities,
    input: {
      prompt: input.prompt,
      system_prompt: input.systemPrompt,
      messages: input.history ?? [],
      tool_policy: input.toolPolicy ?? 'autonomous',
      client_thread_id: input.clientThreadId,
      client_project_id: input.clientProjectId,
      client_task_id: input.clientTaskId ?? null,
      client_provider_profile_id: input.provider.profileId ?? null,
      resolve_current_provider_binding: input.provider.profileId ? true : false,
      client_assistant_message_id: input.assistantMessageId,
      client_user_message_id: input.userMessageId ?? null,
      source: input.source,
    },
    model_profile_id: null,
    snapshot_id: null,
    idempotency_key: `desktop:${input.source}:${input.assistantMessageId}`,
  }, {
    base_url: input.provider.endpoint,
    api_key: apiKey,
    model: input.provider.model,
    timeout_ms: input.provider.timeoutMs,
    max_steps: 64,
    verify_tls_certificates: input.provider.verifyTlsCertificates,
    mcp_servers: mcpServers,
  })
  return { client, run }
}

export async function createDurableCrewRun(
  input: DurableCrewRunInput,
  client = createLocalDaemonRuntimeClient(),
): Promise<{ client: LocalDaemonRuntimeClient; run: RunRecord }> {
  const health = await client.health()
  const threadId = localRuntimeEntityUuid('thread', input.clientThreadId)
  const projectId = localRuntimeEntityUuid('project', input.clientProjectId)
  const taskId = input.clientTaskId
    ? localRuntimeEntityUuid('task', input.clientTaskId)
    : null
  const workspacePath = input.workspacePath?.trim() || null
  if (workspacePath) await client.bindProjectWorkspace(projectId, workspacePath)

  const crewRequest = await hydrateCrewRequestCredentials(input.crewId, input.crewRequest)
  const adapter = crewAdapterModelConfig(crewRequest)
  const streamId = typeof crewRequest.streamId === 'string' && crewRequest.streamId.trim()
    ? crewRequest.streamId.trim()
    : `crew-${input.assistantMessageId}`
  crewRequest.streamId = streamId
  const requiredCapabilities = ['crew.python']
  if (workspacePath) requiredCapabilities.push('files', 'shell')

  const run = await client.createConfiguredRun({
    thread_id: threadId,
    project_id: projectId,
    project_revision: Math.max(1, Math.trunc(input.projectRevision ?? 1)),
    project_privacy: 'private_local',
    task: taskId ? { id: taskId, revision: Math.max(1, Math.trunc(input.taskRevision ?? 1)) } : null,
    executor_target: { kind: 'personal_device', device_id: health.device_id },
    required_capabilities: requiredCapabilities,
    input: {
      prompt: input.prompt,
      tool_policy: 'autonomous',
      client_thread_id: input.clientThreadId,
      client_project_id: input.clientProjectId,
      client_task_id: input.clientTaskId ?? null,
      client_assistant_message_id: input.assistantMessageId,
      client_crew_live_message_id: input.crewLiveMessageId,
      crew_live_title: input.crewLiveTitle,
      crew_stream_id: streamId,
      crew_id: input.crewId,
      source: `crew_${input.source}`,
    },
    model_profile_id: null,
    snapshot_id: null,
    idempotency_key: `desktop:crew:${input.source}:${input.assistantMessageId}`,
  }, {
    base_url: adapter.baseUrl,
    api_key: null,
    model: adapter.model,
    timeout_ms: adapter.timeout,
    max_steps: 1,
    verify_tls_certificates: adapter.verifyTlsCertificates,
    crew_request: crewRequest,
  })
  return { client, run }
}

export async function createDurableCodexRun(
  input: DurableCodexRunInput,
  client = createLocalDaemonRuntimeClient(),
): Promise<{ client: LocalDaemonRuntimeClient; run: RunRecord }> {
  if (!input.profileId.trim()) throw new Error('A ready Codex account is required')
  if (!input.workspacePath.trim()) throw new Error('Persistent Codex runs require a workspace folder')
  const health = await client.health()
  const threadId = localRuntimeEntityUuid('thread', input.clientThreadId)
  const projectId = localRuntimeEntityUuid('project', input.clientProjectId)
  const taskId = input.clientTaskId ? localRuntimeEntityUuid('task', input.clientTaskId) : null
  const workspacePath = input.workspacePath.trim()
  await client.bindProjectWorkspace(projectId, workspacePath)
  const history = (input.history ?? [])
    .filter((message) => message.content.trim())
    .map((message) => `${message.role === 'assistant' ? 'Assistant' : message.role === 'system' ? 'System' : 'User'}:\n${message.content}`)
    .join('\n\n')
  const prompt = [
    input.systemPrompt?.trim() ? `System instructions:\n${input.systemPrompt.trim()}` : '',
    history ? `Sanitized OpenCowork conversation history:\n\n${history}` : '',
    `Current user request:\n${input.prompt}`,
  ].filter(Boolean).join('\n\n')
  const timeoutMs = Math.max(1_000, Math.min(24 * 60 * 60 * 1_000, Math.trunc(input.timeoutMs ?? 600_000)))

  const run = await client.createConfiguredRun({
    thread_id: threadId,
    project_id: projectId,
    project_revision: Math.max(1, Math.trunc(input.projectRevision ?? 1)),
    project_privacy: 'private_local',
    task: taskId ? { id: taskId, revision: Math.max(1, Math.trunc(input.taskRevision ?? 1)) } : null,
    executor_target: { kind: 'personal_device', device_id: health.device_id },
    required_capabilities: ['model.codex', 'files', 'shell'],
    input: {
      prompt: input.prompt,
      tool_policy: input.toolPolicy ?? 'autonomous',
      client_thread_id: input.clientThreadId,
      client_project_id: input.clientProjectId,
      client_task_id: input.clientTaskId ?? null,
      client_assistant_message_id: input.assistantMessageId,
      client_user_message_id: input.userMessageId ?? null,
      codex_profile_id: input.profileId,
      source: input.source,
    },
    model_profile_id: null,
    snapshot_id: null,
    idempotency_key: `desktop:codex:${input.source}:${input.assistantMessageId}`,
  }, {
    base_url: 'https://chatgpt.com/backend-api',
    api_key: null,
    model: input.model?.trim() || 'codex',
    timeout_ms: timeoutMs,
    max_steps: 1,
    verify_tls_certificates: true,
    codex_request: {
      profile_id: input.profileId.trim(),
      prompt,
      cwd: workspacePath,
      model: input.model?.trim() || null,
      reasoning_effort: input.reasoningEffort?.trim() || null,
      tool_policy: input.toolPolicy ?? 'autonomous',
    },
  })
  return { client, run }
}

export async function upsertDurableCodexSchedule(
  input: DurableCodexScheduleInput,
  client = createLocalDaemonRuntimeClient(),
) {
  if (!input.profileId.trim()) throw new Error('A ready Codex account is required')
  if (!input.workspacePath.trim()) throw new Error('Persistent Codex schedules require a workspace folder')
  const health = await client.health()
  const threadId = localRuntimeEntityUuid('thread', input.clientThreadId)
  const projectId = localRuntimeEntityUuid('project', input.clientProjectId)
  const taskId = input.clientTaskId ? localRuntimeEntityUuid('task', input.clientTaskId) : null
  const scheduleId = localRuntimeEntityUuid('task', `schedule:${input.scheduleClientId}`)
  const workspacePath = input.workspacePath.trim()
  await client.bindProjectWorkspace(projectId, workspacePath)
  const history = (input.history ?? [])
    .filter((message) => message.content.trim())
    .map((message) => `${message.role === 'assistant' ? 'Assistant' : message.role === 'system' ? 'System' : 'User'}:\n${message.content}`)
    .join('\n\n')
  const prompt = [
    input.systemPrompt?.trim() ? `System instructions:\n${input.systemPrompt.trim()}` : '',
    history ? `Sanitized OpenCowork conversation history:\n\n${history}` : '',
    `Current scheduled request:\n${input.prompt}`,
  ].filter(Boolean).join('\n\n')
  const timeoutMs = Math.max(1_000, Math.min(24 * 60 * 60 * 1_000, Math.trunc(input.timeoutMs ?? 600_000)))
  return client.upsertSchedule({
    id: scheduleId,
    expression: input.expression,
    timezone: input.timezone,
    enabled: input.enabled,
    run_request: {
      thread_id: threadId,
      project_id: projectId,
      project_revision: Math.max(1, Math.trunc(input.projectRevision ?? 1)),
      project_privacy: 'private_local',
      task: taskId ? { id: taskId, revision: Math.max(1, Math.trunc(input.taskRevision ?? 1)) } : null,
      executor_target: { kind: 'personal_device', device_id: health.device_id },
      required_capabilities: ['model.codex', 'files', 'shell'],
      input: {
        prompt: input.prompt,
        tool_policy: input.toolPolicy ?? 'autonomous',
        client_thread_id: input.clientThreadId,
        client_project_id: input.clientProjectId,
        client_task_id: input.clientTaskId ?? null,
        resolve_current_versions: true,
        client_assistant_message_id: 'assigned-at-trigger',
        client_user_message_id: 'assigned-at-trigger',
        codex_profile_id: input.profileId,
        source: 'task',
      },
      model_profile_id: null,
      snapshot_id: null,
      idempotency_key: `schedule-template:${scheduleId}`,
    },
    model_config: {
      base_url: 'https://chatgpt.com/backend-api',
      api_key: null,
      model: input.model?.trim() || 'codex',
      timeout_ms: timeoutMs,
      max_steps: 1,
      verify_tls_certificates: true,
      codex_request: {
        profile_id: input.profileId.trim(),
        prompt,
        cwd: workspacePath,
        model: input.model?.trim() || null,
        reasoning_effort: input.reasoningEffort?.trim() || null,
        tool_policy: input.toolPolicy ?? 'autonomous',
      },
    },
  })
}

export async function upsertDurableCrewSchedule(
  input: DurableCrewScheduleInput,
  client = createLocalDaemonRuntimeClient(),
) {
  const health = await client.health()
  const threadId = localRuntimeEntityUuid('thread', input.clientThreadId)
  const projectId = localRuntimeEntityUuid('project', input.clientProjectId)
  const taskId = input.clientTaskId ? localRuntimeEntityUuid('task', input.clientTaskId) : null
  const scheduleId = localRuntimeEntityUuid('task', `schedule:${input.scheduleClientId}`)
  const workspacePath = input.workspacePath?.trim() || null
  if (workspacePath) await client.bindProjectWorkspace(projectId, workspacePath)
  const crewRequest = await hydrateCrewRequestCredentials(input.crewId, input.crewRequest)
  const crewProviderProfileIds = await mirrorCrewProviderDeviceBindings(crewRequest, client)
  const adapter = crewAdapterModelConfig(crewRequest)
  const requiredCapabilities = ['crew.python']
  if (workspacePath) requiredCapabilities.push('files', 'shell')

  return client.upsertSchedule({
    id: scheduleId,
    expression: input.expression,
    timezone: input.timezone,
    enabled: input.enabled,
    run_request: {
      thread_id: threadId,
      project_id: projectId,
      project_revision: Math.max(1, Math.trunc(input.projectRevision ?? 1)),
      project_privacy: 'private_local',
      task: taskId ? { id: taskId, revision: Math.max(1, Math.trunc(input.taskRevision ?? 1)) } : null,
      executor_target: { kind: 'personal_device', device_id: health.device_id },
      required_capabilities: requiredCapabilities,
      input: {
        prompt: input.prompt,
        tool_policy: 'autonomous',
        client_thread_id: input.clientThreadId,
        client_project_id: input.clientProjectId,
        client_task_id: input.clientTaskId ?? null,
        resolve_current_versions: true,
        resolve_current_crew_provider_bindings: crewProviderProfileIds.length > 0,
        client_crew_provider_profile_ids: crewProviderProfileIds,
        client_assistant_message_id: 'assigned-at-trigger',
        client_user_message_id: 'assigned-at-trigger',
        client_crew_live_message_id: 'assigned-at-trigger',
        crew_live_title: input.crewLiveTitle,
        crew_stream_id: 'assigned-at-trigger',
        crew_id: input.crewId,
        source: 'crew_task',
      },
      model_profile_id: null,
      snapshot_id: null,
      idempotency_key: `schedule-template:${scheduleId}`,
    },
    model_config: {
      base_url: adapter.baseUrl,
      api_key: null,
      model: adapter.model,
      timeout_ms: adapter.timeout,
      max_steps: 1,
      verify_tls_certificates: adapter.verifyTlsCertificates,
      crew_request: crewRequest,
    },
  })
}

export async function upsertDurableLocalSchedule(
  input: DurableLocalScheduleInput,
  client = createLocalDaemonRuntimeClient(),
) {
  if (input.provider.provider !== 'openai-compatible') {
    throw new Error('Persistent local schedules currently require an OpenAI-compatible model profile')
  }
  const health = await client.health()
  const threadId = localRuntimeEntityUuid('thread', input.clientThreadId)
  const projectId = localRuntimeEntityUuid('project', input.clientProjectId)
  const taskId = input.clientTaskId ? localRuntimeEntityUuid('task', input.clientTaskId) : null
  const scheduleId = localRuntimeEntityUuid('task', `schedule:${input.scheduleClientId}`)
  const workspacePath = input.workspacePath?.trim() || null
  if (workspacePath) await client.bindProjectWorkspace(projectId, workspacePath)
  const apiKey = await bindProviderForDaemon(input.provider, client)
  const requiredCapabilities = [modelCapability(input.provider)]
  if (workspacePath) requiredCapabilities.push('files', 'shell')
  const mcpBindings = configuredMcpBindings(input)
  await Promise.all(mcpBindings.map(({ id, binding }) => client.upsertMcpBinding(id, binding)))
  const mcpServers = mcpBindings.map(({ binding }) => binding)
  if (mcpServers.length > 0) requiredCapabilities.push('tool.mcp.invoke')
  return client.upsertSchedule({
    id: scheduleId,
    expression: input.expression,
    timezone: input.timezone,
    enabled: input.enabled,
    run_request: {
      thread_id: threadId,
      project_id: projectId,
      project_revision: Math.max(1, Math.trunc(input.projectRevision ?? 1)),
      project_privacy: 'private_local',
      task: taskId ? { id: taskId, revision: Math.max(1, Math.trunc(input.taskRevision ?? 1)) } : null,
      executor_target: { kind: 'personal_device', device_id: health.device_id },
      required_capabilities: requiredCapabilities,
      input: {
        prompt: input.prompt,
        system_prompt: input.systemPrompt,
        messages: input.history ?? [],
        tool_policy: input.toolPolicy ?? 'autonomous',
        client_thread_id: input.clientThreadId,
        client_project_id: input.clientProjectId,
        client_task_id: input.clientTaskId ?? null,
        client_provider_profile_id: input.provider.profileId ?? null,
        client_mcp_server_ids: mcpBindings.map(({ id }) => id),
        resolve_current_versions: true,
        resolve_current_provider_binding: input.provider.profileId ? true : false,
        resolve_current_mcp_bindings: mcpBindings.length > 0,
        client_assistant_message_id: 'assigned-at-trigger',
        client_user_message_id: 'assigned-at-trigger',
        source: 'task',
      },
      model_profile_id: null,
      snapshot_id: null,
      idempotency_key: `schedule-template:${scheduleId}`,
    },
    model_config: {
      base_url: input.provider.endpoint,
      api_key: apiKey,
      model: input.provider.model,
      timeout_ms: input.provider.timeoutMs,
      max_steps: 64,
      verify_tls_certificates: input.provider.verifyTlsCertificates,
      mcp_servers: mcpServers,
    },
  })
}

export async function deleteDurableLocalSchedule(
  scheduleClientId: string,
  client = createLocalDaemonRuntimeClient(),
): Promise<boolean> {
  return client.deleteSchedule(localRuntimeEntityUuid('task', `schedule:${scheduleClientId}`))
}

export function watchDurableLocalRun(
  client: LocalDaemonRuntimeClient,
  runId: string,
  callbacks: DurableLocalRunCallbacks = {},
): DurableLocalRunWatcher {
  let unsubscribe = () => {}
  let settled = false
  let resolveDone!: (run: RunRecord) => void
  let rejectDone!: (error: Error) => void
  const done = new Promise<RunRecord>((resolve, reject) => {
    resolveDone = resolve
    rejectDone = reject
  })
  const finish = async () => {
    if (settled) return
    try {
      const run = await client.getRun(runId)
      if (!['completed', 'failed', 'canceled', 'expired', 'interrupted'].includes(run.state)) return
      settled = true
      unsubscribe()
      callbacks.onState?.(run.state, run)
      resolveDone(run)
    } catch (cause) {
      const error = cause instanceof Error ? cause : new Error(String(cause))
      callbacks.onError?.(error)
      rejectDone(error)
    }
  }
  unsubscribe = client.subscribeRunEvents(runId, 0, (event) => {
    callbacks.onEvent?.(event)
    if (event.kind === 'completed' || event.kind === 'failed') void finish()
    if (event.kind === 'state_changed' && event.payload && typeof event.payload === 'object') {
      const state = (event.payload as Record<string, unknown>).to
      if (state === 'canceled' || state === 'expired' || state === 'interrupted') void finish()
    }
  }, (error) => callbacks.onError?.(error))
  void finish()
  return { runId, done, unsubscribe: () => unsubscribe() }
}
