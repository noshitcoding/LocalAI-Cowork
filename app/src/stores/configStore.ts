import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import type { TerminalPersistenceMode } from './terminalStore'
import {
  deleteCredential,
  llmApiKeyLocator,
  mcpCredentialOwner,
  replaceCredentialMap,
  setCredential,
} from '../security/credentialVault'
import {
  sanitizeMcpServerForPersistence,
  sanitizeProfilesForPersistence,
} from '../security/credentialPersistence'
import { normalizeProviderModels, resolveProviderModelFromCatalog } from '../utils/providerModels'
import { safeInvokeVoid } from '../utils/safeInvoke'

export type OllamaConfig = {
  baseUrl: string
  model: string
  timeoutMs: number
  contextWindow: number
  temperature: number
}

export type BackendKind = 'codex' | 'openai-compatible'
export type ApiProfilePreset = 'ollama' | 'openrouter' | 'openai' | 'custom'
export type ApiProfileAuthMode = 'none' | 'bearer'

/** @deprecated Persisted provider values are accepted only by the v2 migration. */
export type LegacyLlmProviderKind = 'ollama' | 'openai-compatible' | 'openrouter'
/** @deprecated Use ApiProfilePreset for profile templates and BackendKind for routing. */
export type LlmProviderKind = LegacyLlmProviderKind | ApiProfilePreset

export type LlmProfile = {
  id: string
  name: string
  /** @deprecated Persisted legacy values are normalized to openai-compatible on load. */
  provider: LegacyLlmProviderKind
  preset?: ApiProfilePreset
  authMode?: ApiProfileAuthMode
  baseUrl: string
  model: string
  apiKey: string
  hasApiKey?: boolean
  timeoutMs: number
  verifyTlsCertificates: boolean
  contextWindow: number | null
  temperature: number | null
}

export type DefaultLlmProfileIds = {
  api?: string
  /** @deprecated compatibility aliases retained for one migration release */
  ollama: string
  'openai-compatible': string
  openrouter: string
}

export type McpServerConfig = {
  id?: string
  name: string
  command: string
  args: string
  env: Record<string, string>
}

export type StartView = 'last' | 'work' | 'settings'

export type AppPreferences = {
  autoApproveSafeTools: boolean
  autoPilotAllTools: boolean
  readOnlyFsMode: boolean
  commandWhitelist: string
  commandBlacklist: string
  maxToolCallsPerLoop: number
  fallbackToHumanOnRepeatedFailure: boolean
  confirmOnCloseWithRunningTasks: boolean
  telemetryEnabled: boolean
  notificationsEnabled: boolean
  soundsEnabled: boolean
  launchAtStartup: boolean
  showTimestamps: boolean
  defaultStartView: StartView
  focusMode: boolean
  compactMode: boolean
  verboseMode: boolean
  limitThinkingWindow: boolean
  superVerboseAuditLogging: boolean
  fontScale: number
  shortcutOverlayEnabled: boolean
  syncThemeWithSystem: boolean
  chatRetentionDays: number
  autoBackupDb: boolean
  dbBackupIntervalHours: number
  workspaceDefaultPath: string
  mcpAutoReconnect: boolean
  mcpVerboseLogging: boolean
  mcpEnvEditorEnabled: boolean
  mcpAllowManualImport: boolean
  ollamaStreamAutosave: boolean
  dbCleanupOnStart: boolean
  taskBatchMultiSelectEnabled: boolean
  terminalPersistenceMode: TerminalPersistenceMode
}

type ConfigState = {
  ollama: OllamaConfig
  llmProfiles: LlmProfile[]
  defaultLlmProfileIds: DefaultLlmProfileIds
  llmProfileModels: Record<string, string[]>
  preferences: AppPreferences
  mcpServer: McpServerConfig
  mcpServers: McpServerConfig[]
  activeMcpServerName: string
  availableModels: string[]
  setOllama: (patch: Partial<OllamaConfig>) => void
  addLlmProfile: (preset?: ApiProfilePreset) => string
  updateLlmProfile: (id: string, patch: Partial<Omit<LlmProfile, 'apiKey'>>) => void
  setLlmProfileApiKey: (id: string, apiKey: string) => Promise<void>
  deleteLlmProfile: (id: string) => Promise<void>
  setDefaultLlmProfile: (provider: LlmProviderKind, id: string) => void
  setLlmProfileModels: (id: string, models: string[]) => void
  setPreference: <K extends keyof AppPreferences>(key: K, value: AppPreferences[K]) => void
  setPreferences: (patch: Partial<AppPreferences>) => void
  setMcpServer: (patch: Partial<Omit<McpServerConfig, 'env'>>) => void
  setMcpServerEnv: (env: Record<string, string>) => Promise<void>
  setActiveMcpServer: (name: string) => void
  upsertMcpServer: (server: McpServerConfig) => Promise<void>
  importMcpServers: (servers: McpServerConfig[]) => Promise<void>
  deleteMcpServer: (name: string) => Promise<void>
  setAvailableModels: (models: string[]) => void
}

const DEFAULT_OLLAMA: OllamaConfig = {
  baseUrl: 'http://localhost:11434',
  model: 'llama3.1:8b',
  timeoutMs: 600000,
  contextWindow: 128000,
  temperature: 0.1,
}

const DEFAULT_OPENAI_COMPATIBLE_PROFILE = {
  apiKey: '',
  baseUrl: 'https://api.openai.com/v1',
  model: 'gpt-4.1-mini',
  timeoutMs: 600000,
}

const DEFAULT_LLM_PROFILE_IDS: DefaultLlmProfileIds = {
  api: 'default-ollama',
  ollama: 'default-ollama',
  'openai-compatible': 'default-openai-compatible',
  openrouter: 'default-openrouter',
}

function createBaseLlmProfile(preset: ApiProfilePreset): LlmProfile {
  return preset === 'ollama'
    ? {
        id: DEFAULT_LLM_PROFILE_IDS.ollama,
        name: 'Lokales Ollama',
        provider: 'openai-compatible',
        preset,
        authMode: 'none',
        baseUrl: `${DEFAULT_OLLAMA.baseUrl}/v1`,
        model: DEFAULT_OLLAMA.model,
        apiKey: '',
        timeoutMs: DEFAULT_OLLAMA.timeoutMs,
        verifyTlsCertificates: true,
        contextWindow: DEFAULT_OLLAMA.contextWindow,
        temperature: DEFAULT_OLLAMA.temperature,
      }
    : preset === 'openai'
      ? {
          id: DEFAULT_LLM_PROFILE_IDS['openai-compatible'],
          name: 'OpenAI',
          provider: 'openai-compatible',
          preset,
          authMode: 'bearer',
          baseUrl: DEFAULT_OPENAI_COMPATIBLE_PROFILE.baseUrl,
          model: DEFAULT_OPENAI_COMPATIBLE_PROFILE.model,
          apiKey: DEFAULT_OPENAI_COMPATIBLE_PROFILE.apiKey,
          timeoutMs: DEFAULT_OPENAI_COMPATIBLE_PROFILE.timeoutMs,
          verifyTlsCertificates: true,
          contextWindow: 128000,
          temperature: null,
        }
      : preset === 'openrouter'
        ? {
          id: DEFAULT_LLM_PROFILE_IDS.openrouter,
          name: 'OpenRouter',
          provider: 'openai-compatible',
          preset,
          authMode: 'bearer',
          baseUrl: 'https://openrouter.ai/api/v1',
          model: '',
          apiKey: '',
          timeoutMs: DEFAULT_OLLAMA.timeoutMs,
          verifyTlsCertificates: true,
          contextWindow: 128000,
          temperature: null,
        }
        : {
            id: 'default-custom-api',
            name: 'Eigene API',
            provider: 'openai-compatible',
            preset: 'custom',
            authMode: 'bearer',
            baseUrl: 'http://localhost:8000/v1',
            model: '',
            apiKey: '',
            timeoutMs: DEFAULT_OLLAMA.timeoutMs,
            verifyTlsCertificates: true,
            contextWindow: 128000,
            temperature: null,
          }
}

type PersistedLlmProfile = Partial<Omit<LlmProfile, 'provider' | 'preset'>> & {
  id?: string
  provider?: LegacyLlmProviderKind | 'openai-compatible'
  preset?: ApiProfilePreset
}

function inferProfilePreset(profile: PersistedLlmProfile): ApiProfilePreset {
  if (profile.preset === 'ollama' || profile.preset === 'openrouter' || profile.preset === 'openai' || profile.preset === 'custom') {
    return profile.preset
  }
  if (profile.provider === 'ollama') return 'ollama'
  if (profile.provider === 'openrouter') return 'openrouter'
  const baseUrl = profile.baseUrl?.trim().toLowerCase() ?? ''
  return baseUrl.includes('api.openai.com') ? 'openai' : 'custom'
}

function normalizeApiBaseUrl(baseUrl: string, preset: ApiProfilePreset): string {
  const trimmed = baseUrl.trim().replace(/\/$/, '')
  if (preset === 'ollama' && trimmed && !trimmed.toLowerCase().endsWith('/v1')) return `${trimmed}/v1`
  return trimmed
}

export function normalizeLlmProfile(profile: PersistedLlmProfile): LlmProfile {
  const preset = inferProfilePreset(profile)
  const baseProfile = createBaseLlmProfile(preset)
  const rawTimeout = Number(profile.timeoutMs ?? baseProfile.timeoutMs)
  const rawContextWindow = profile.contextWindow ?? baseProfile.contextWindow
  const rawTemperature = profile.temperature ?? baseProfile.temperature
  const normalizedModel = profile.model?.trim()

  return {
    ...baseProfile,
    ...profile,
    id: profile.id?.trim() || baseProfile.id,
    provider: 'openai-compatible',
    preset,
    authMode: profile.authMode ?? (preset === 'ollama' ? 'none' : 'bearer'),
    name: profile.name?.trim() || baseProfile.name,
    baseUrl: normalizeApiBaseUrl(profile.baseUrl?.trim() || baseProfile.baseUrl, preset),
    model: normalizedModel ?? baseProfile.model,
    apiKey: profile.apiKey?.trim() ?? baseProfile.apiKey,
    hasApiKey: profile.hasApiKey ?? Boolean(profile.apiKey?.trim()),
    timeoutMs: Math.max(1000, Number.isFinite(rawTimeout) ? rawTimeout : baseProfile.timeoutMs),
    verifyTlsCertificates: profile.verifyTlsCertificates ?? baseProfile.verifyTlsCertificates,
    contextWindow: Math.max(
      512,
      Number.isFinite(Number(rawContextWindow)) ? Number(rawContextWindow) : DEFAULT_OLLAMA.contextWindow,
    ),
    temperature: preset === 'ollama'
      ? (Number.isFinite(Number(rawTemperature)) ? Number(rawTemperature) : DEFAULT_OLLAMA.temperature)
      : null,
  }
}

function createDefaultLlmProfile(preset: ApiProfilePreset, overrides: Partial<LlmProfile> = {}): LlmProfile {
  return normalizeLlmProfile({
    ...createBaseLlmProfile(preset),
    ...overrides,
    provider: 'openai-compatible',
    preset,
  })
}

function buildDefaultLlmProfiles(
  legacyOllama: Partial<OllamaConfig> | undefined,
): LlmProfile[] {
  return [
    createDefaultLlmProfile('ollama', {
      baseUrl: legacyOllama?.baseUrl,
      model: legacyOllama?.model,
      timeoutMs: legacyOllama?.timeoutMs,
      contextWindow: legacyOllama?.contextWindow,
      temperature: legacyOllama?.temperature,
    }),
    createDefaultLlmProfile('openai'),
    createDefaultLlmProfile('openrouter'),
  ]
}

function ensureLlmProfiles(
  legacyOllama: Partial<OllamaConfig> | undefined,
  profiles: PersistedLlmProfile[] | undefined,
): LlmProfile[] {
  const fallbackProfiles = buildDefaultLlmProfiles(legacyOllama)
  const byId = new Map<string, LlmProfile>(fallbackProfiles.map((profile) => [profile.id, profile]))

  ;(profiles ?? []).forEach((profile) => {
    if (!profile?.id) {
      return
    }
    byId.set(profile.id, normalizeLlmProfile(profile))
  })

  return Array.from(byId.values())
}

function ensureDefaultLlmProfileIds(
  defaultIds: Partial<DefaultLlmProfileIds> | undefined,
  profiles: LlmProfile[],
): DefaultLlmProfileIds {
  const nextIds: DefaultLlmProfileIds = {
    api: defaultIds?.api ?? defaultIds?.ollama ?? DEFAULT_LLM_PROFILE_IDS.api,
    ollama: defaultIds?.ollama ?? DEFAULT_LLM_PROFILE_IDS.ollama,
    'openai-compatible': defaultIds?.['openai-compatible'] ?? DEFAULT_LLM_PROFILE_IDS['openai-compatible'],
    openrouter: defaultIds?.openrouter ?? DEFAULT_LLM_PROFILE_IDS.openrouter,
  }

  const resolvePresetFallback = (preset: ApiProfilePreset) => {
    return profiles.find((profile) => profile.preset === preset)?.id ?? createDefaultLlmProfile(preset).id
  }

  if (!profiles.some((profile) => profile.id === nextIds.ollama && profile.preset === 'ollama')) {
    nextIds.ollama = resolvePresetFallback('ollama')
  }
  if (!profiles.some((profile) => profile.id === nextIds['openai-compatible'] && profile.preset === 'openai')) {
    nextIds['openai-compatible'] = resolvePresetFallback('openai')
  }
  if (!profiles.some((profile) => profile.id === nextIds.openrouter && profile.preset === 'openrouter')) {
    nextIds.openrouter = resolvePresetFallback('openrouter')
  }
  if (!profiles.some((profile) => profile.id === nextIds.api)) nextIds.api = nextIds.ollama

  return nextIds
}

function resolveDefaultLlmProfile(
  profiles: LlmProfile[],
  defaultIds: DefaultLlmProfileIds,
  preset: ApiProfilePreset,
): LlmProfile {
  const legacyId = preset === 'ollama'
    ? defaultIds.ollama
    : preset === 'openrouter'
      ? defaultIds.openrouter
      : defaultIds['openai-compatible']
  return profiles.find((profile) => profile.id === legacyId && profile.preset === preset)
    ?? profiles.find((profile) => profile.preset === preset)
    ?? createDefaultLlmProfile(preset)
}

function syncLegacyOllamaConfig(
  profiles: LlmProfile[],
  defaultIds: DefaultLlmProfileIds,
  currentOllama: Partial<OllamaConfig> | undefined,
): OllamaConfig {
  const activeProfile = resolveDefaultLlmProfile(profiles, defaultIds, 'ollama')

  return {
    ...DEFAULT_OLLAMA,
    ...(currentOllama ?? {}),
    baseUrl: (activeProfile.baseUrl || currentOllama?.baseUrl || DEFAULT_OLLAMA.baseUrl).replace(/\/v1\/?$/i, ''),
    model: activeProfile.model || currentOllama?.model || DEFAULT_OLLAMA.model,
    timeoutMs: Math.max(DEFAULT_OLLAMA.timeoutMs, activeProfile.timeoutMs || currentOllama?.timeoutMs || DEFAULT_OLLAMA.timeoutMs),
    contextWindow: activeProfile.contextWindow ?? currentOllama?.contextWindow ?? DEFAULT_OLLAMA.contextWindow,
    temperature: activeProfile.temperature ?? currentOllama?.temperature ?? DEFAULT_OLLAMA.temperature,
  }
}

function createLlmProfileId(preset: ApiProfilePreset): string {
  return `${preset}-api-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}

function persistApiProfileMetadata(profile: LlmProfile | undefined): void {
  if (!profile) return
  const timestamp = new Date().toISOString()
  void safeInvokeVoid('api_profile_upsert', {
    profile: {
      id: profile.id,
      name: profile.name,
      preset: profile.preset ?? 'custom',
      authMode: profile.authMode ?? (profile.preset === 'ollama' ? 'none' : 'bearer'),
      baseUrl: profile.baseUrl,
      model: profile.model,
      timeoutMs: profile.timeoutMs,
      verifyTlsCertificates: profile.verifyTlsCertificates,
      contextWindow: profile.contextWindow,
      temperature: profile.temperature,
      isExample: Object.values(DEFAULT_LLM_PROFILE_IDS).includes(profile.id),
      createdAt: timestamp,
      updatedAt: timestamp,
    },
  })
}

function persistDefaultApiProfile(profileId: string): void {
  void safeInvokeVoid('api_default_profile_write', { profileId })
}

const DEFAULT_PREFERENCES: AppPreferences = {
  autoApproveSafeTools: true,
  autoPilotAllTools: false,
  readOnlyFsMode: false,
  commandWhitelist: 'npm run test\nnpm run build\ncargo check',
  commandBlacklist: 'rm -rf\ndel /f /s /q\nformat c:',
  maxToolCallsPerLoop: 12,
  fallbackToHumanOnRepeatedFailure: true,
  confirmOnCloseWithRunningTasks: true,
  telemetryEnabled: false,
  notificationsEnabled: true,
  soundsEnabled: false,
  launchAtStartup: false,
  showTimestamps: true,
  defaultStartView: 'last',
  focusMode: false,
  compactMode: false,
  verboseMode: false,
  limitThinkingWindow: true,
  superVerboseAuditLogging: false,
  fontScale: 100,
  shortcutOverlayEnabled: true,
  syncThemeWithSystem: false,
  chatRetentionDays: 30,
  autoBackupDb: true,
  dbBackupIntervalHours: 24,
  workspaceDefaultPath: '',
  mcpAutoReconnect: true,
  mcpVerboseLogging: false,
  mcpEnvEditorEnabled: true,
  mcpAllowManualImport: true,
  ollamaStreamAutosave: true,
  dbCleanupOnStart: false,
  taskBatchMultiSelectEnabled: true,
  terminalPersistenceMode: 'runtime',
}

const DEFAULT_MCP: McpServerConfig = {
  id: 'default-duckduckgo-websearch',
  name: 'duckduckgo-websearch',
  command: 'node',
  args: 'scripts/mcp/duckduckgo-websearch-server.mjs',
  env: {
    DDG_MAX_RESULTS: '5',
    DDG_REGION: 'wt-wt',
    DDG_SAFESEARCH: 'moderate',
    DDG_TIMEOUT_MS: '10000',
  },
}

function normalizeServer(server: McpServerConfig): McpServerConfig {
  return {
    id: server.id?.trim() || `mcp-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`,
    name: server.name.trim(),
    command: server.command.trim(),
    args: server.args.trim(),
    env: server.env ?? {},
  }
}

function isLegacyFilesystemServer(server: McpServerConfig): boolean {
  const command = server.command.trim().toLowerCase()
  const args = server.args.trim().toLowerCase()
  const name = server.name.trim().toLowerCase()
  return (
    (command === 'npx' && args.includes('@modelcontextprotocol/server-filesystem')) ||
    name === 'filesystem'
  )
}

function isLegacyLocalDocsServer(server: McpServerConfig): boolean {
  const command = server.command.trim().toLowerCase()
  const name = server.name.trim().toLowerCase()
  return (
    command === 'localai-cowork-docs-mcp'
    || command === 'open-cowork-docs-mcp'
    || name === 'local-docs'
  )
}

function isLegacyscreenshotServer(server: McpServerConfig): boolean {
  const command = server.command.trim().toLowerCase()
  const name = server.name.trim().toLowerCase()
  return (
    command === 'localai-cowork-screenshot-mcp'
    || command === 'open-cowork-screenshot-mcp'
    || name === 'screenshot'
  )
}

function migrateServer(server: McpServerConfig): McpServerConfig {
  if (
    !isLegacyFilesystemServer(server)
    && !isLegacyLocalDocsServer(server)
    && !isLegacyscreenshotServer(server)
  ) {
    return server
  }

  return {
    ...DEFAULT_MCP,
    env: server.env ?? {},
  }
}

function chooseServer(
  servers: McpServerConfig[],
  activeName: string,
): McpServerConfig {
  return servers.find((server) => server.name === activeName) ?? servers[0] ?? DEFAULT_MCP
}

export const useConfigStore = create<ConfigState>()(
  persist(
    (set) => ({
      ollama: DEFAULT_OLLAMA,
      llmProfiles: buildDefaultLlmProfiles(DEFAULT_OLLAMA),
      defaultLlmProfileIds: DEFAULT_LLM_PROFILE_IDS,
      llmProfileModels: {
        [DEFAULT_LLM_PROFILE_IDS.ollama]: [],
        [DEFAULT_LLM_PROFILE_IDS['openai-compatible']]: [],
        [DEFAULT_LLM_PROFILE_IDS.openrouter]: [],
      },
      preferences: DEFAULT_PREFERENCES,
      mcpServer: DEFAULT_MCP,
      mcpServers: [DEFAULT_MCP],
      activeMcpServerName: DEFAULT_MCP.name,
      availableModels: [],
      setOllama: (patch) =>
        set((state) => {
          const nextOllama = { ...state.ollama, ...patch }
          const llmProfiles = state.llmProfiles.map((profile) => (
            profile.id === state.defaultLlmProfileIds.ollama
              ? normalizeLlmProfile({
                  ...profile,
                  provider: 'openai-compatible',
                  preset: 'ollama',
                  authMode: 'none',
                  baseUrl: nextOllama.baseUrl,
                  model: nextOllama.model,
                  timeoutMs: nextOllama.timeoutMs,
                  contextWindow: nextOllama.contextWindow,
                  temperature: nextOllama.temperature,
                })
              : profile
          ))

          return {
            ollama: nextOllama,
            llmProfiles,
          }
        }),
      addLlmProfile: (preset = 'custom') => {
        const id = createLlmProfileId(preset)
        set((state) => ({
          llmProfiles: [
            ...state.llmProfiles,
            createDefaultLlmProfile(preset, {
              id,
              name: `${preset === 'ollama' ? 'Ollama' : preset === 'openrouter' ? 'OpenRouter' : preset === 'openai' ? 'OpenAI' : 'Eigene API'} ${state.llmProfiles.filter((profile) => profile.preset === preset).length + 1}`,
            }),
          ],
          llmProfileModels: {
            ...state.llmProfileModels,
            [id]: [],
          },
        }))
        persistApiProfileMetadata(useConfigStore.getState().llmProfiles.find((profile) => profile.id === id))
        return id
      },
      updateLlmProfile: (id, patch) => {
        set((state) => {
          const profile = state.llmProfiles.find((item) => item.id === id)
          if (!profile) {
            return state
          }

          const llmProfiles = state.llmProfiles.map((item) => (
            item.id === id
              ? normalizeLlmProfile({
                  ...item,
                  ...patch,
                  provider: item.provider,
                })
              : item
          ))

          return {
            llmProfiles,
            ollama: id === state.defaultLlmProfileIds.ollama
              ? syncLegacyOllamaConfig(llmProfiles, state.defaultLlmProfileIds, state.ollama)
              : state.ollama,
          }
        })
        persistApiProfileMetadata(useConfigStore.getState().llmProfiles.find((profile) => profile.id === id))
      },
      setLlmProfileApiKey: async (id, apiKey) => {
        if (!useConfigStore.getState().llmProfiles.some((profile) => profile.id === id)) return
        await setCredential(llmApiKeyLocator(id), apiKey)
        set((state) => ({
          llmProfiles: state.llmProfiles.map((profile) => (
            profile.id === id ? { ...profile, apiKey: '', hasApiKey: Boolean(apiKey) } : profile
          )),
        }))
      },
      deleteLlmProfile: async (id) => {
        if (useConfigStore.getState().defaultLlmProfileIds.api === id) return
        await deleteCredential(llmApiKeyLocator(id))
        await safeInvokeVoid('api_profile_delete', { id })
        set((state) => {
          if (state.defaultLlmProfileIds.api === id) {
            return state
          }

          const nextModels = { ...state.llmProfileModels }
          delete nextModels[id]

          return {
            llmProfiles: state.llmProfiles.filter((profile) => profile.id !== id),
            llmProfileModels: nextModels,
          }
        })
      },
      setDefaultLlmProfile: (provider, id) => {
        set((state) => {
          const profile = state.llmProfiles.find((item) => item.id === id)
          if (!profile) {
            return state
          }

          const defaultLlmProfileIds = {
            ...state.defaultLlmProfileIds,
            api: id,
            ...(provider === 'ollama' ? { ollama: id } : {}),
            ...(provider === 'openrouter' ? { openrouter: id } : {}),
            ...(provider === 'openai' || provider === 'openai-compatible' ? { 'openai-compatible': id } : {}),
          }

          return {
            defaultLlmProfileIds,
            ollama: profile.preset === 'ollama'
              ? syncLegacyOllamaConfig(state.llmProfiles, defaultLlmProfileIds, state.ollama)
              : state.ollama,
            availableModels: profile.preset === 'ollama'
              ? state.llmProfileModels[id] ?? []
              : state.availableModels,
          }
        })
        persistDefaultApiProfile(id)
      },
      setLlmProfileModels: (id, models) => {
        set((state) => {
          const normalizedModels = normalizeProviderModels(models)
          const llmProfiles = state.llmProfiles.map((profile) => {
            if (profile.id !== id || profile.preset === 'ollama') return profile
            const resolvedModel = resolveProviderModelFromCatalog(profile.model, normalizedModels)
            return resolvedModel && resolvedModel !== profile.model
              ? normalizeLlmProfile({ ...profile, model: resolvedModel })
              : profile
          })

          return {
            llmProfiles,
            llmProfileModels: {
              ...state.llmProfileModels,
              [id]: normalizedModels,
            },
            availableModels: id === state.defaultLlmProfileIds.ollama ? normalizedModels : state.availableModels,
          }
        })
        persistApiProfileMetadata(useConfigStore.getState().llmProfiles.find((profile) => profile.id === id))
      },
      setPreference: (key, value) =>
        set((state) => ({
          preferences: {
            ...state.preferences,
            [key]: value,
          },
        })),
      setPreferences: (patch) =>
        set((state) => ({
          preferences: {
            ...state.preferences,
            ...patch,
          },
        })),
      setMcpServer: (patch) =>
        set((state) => {
          const updated = normalizeServer({ ...state.mcpServer, ...patch })
          const servers = (state.mcpServers.length > 0 ? state.mcpServers : [state.mcpServer])
            .map((server) => (server.name === state.mcpServer.name ? updated : server))
          return {
            mcpServer: updated,
            mcpServers: servers,
            activeMcpServerName: updated.name,
          }
        }),
      setMcpServerEnv: async (env) => {
        const state = useConfigStore.getState()
        const active = state.mcpServer
        await replaceCredentialMap(
          'mcp_env',
          mcpCredentialOwner(active),
          active.env ?? {},
          env,
        )
        set((current) => {
          const updated = normalizeServer({ ...current.mcpServer, env })
          const servers = (current.mcpServers.length > 0 ? current.mcpServers : [current.mcpServer])
            .map((server) => (server.id === updated.id ? updated : server))
          return {
            mcpServer: updated,
            mcpServers: servers,
          }
        })
      },
      setActiveMcpServer: (name) =>
        set((state) => {
          const servers = state.mcpServers.length > 0 ? state.mcpServers : [state.mcpServer]
          return {
            activeMcpServerName: name,
            mcpServer: chooseServer(servers, name),
          }
        }),
      upsertMcpServer: async (server) => {
        const normalized = normalizeServer(server)
        const existingServer = useConfigStore.getState().mcpServers.find((item) => (
          item.id === normalized.id || item.name === normalized.name
        ))
        await replaceCredentialMap(
          'mcp_env',
          mcpCredentialOwner(normalized),
          existingServer?.env ?? {},
          normalized.env,
        )
        set((state) => {
          const existing = state.mcpServers.length > 0 ? state.mcpServers : [state.mcpServer]
          const servers = existing.some((item) => item.id === normalized.id || item.name === normalized.name)
            ? existing.map((item) => (item.id === normalized.id || item.name === normalized.name ? normalized : item))
            : [...existing, normalized]
          return {
            mcpServers: servers,
            activeMcpServerName: normalized.name,
            mcpServer: normalized,
          }
        })
      },
      importMcpServers: async (serversToImport) => {
        const normalizedImports = serversToImport.map(normalizeServer)
          .filter((server) => server.name && server.command)
        await Promise.all(normalizedImports.map((server) => replaceCredentialMap(
          'mcp_env',
          mcpCredentialOwner(server),
          {},
          server.env,
        )))
        set((state) => {
          const existing = state.mcpServers.length > 0 ? state.mcpServers : [state.mcpServer]
          const byName = new Map(existing.map((server) => [server.name, server]))
          normalizedImports.forEach((server) => {
            byName.set(server.name, server)
          })
          const servers = Array.from(byName.values())
          const activeMcpServerName = normalizedImports[0]?.name ?? state.activeMcpServerName
          return {
            mcpServers: servers,
            activeMcpServerName,
            mcpServer: chooseServer(servers, activeMcpServerName),
          }
        })
      },
      deleteMcpServer: async (name) => {
        const server = useConfigStore.getState().mcpServers.find((item) => item.name === name)
        if (server) {
          await replaceCredentialMap('mcp_env', mcpCredentialOwner(server), server.env, {})
        }
        set((state) => {
          const servers = (state.mcpServers.length > 0 ? state.mcpServers : [state.mcpServer])
            .filter((server) => server.name !== name)
          const fallback = servers[0] ?? DEFAULT_MCP
          return {
            mcpServers: servers.length > 0 ? servers : [DEFAULT_MCP],
            activeMcpServerName: fallback.name,
            mcpServer: fallback,
          }
        })
      },
      setAvailableModels: (models) =>
        set((state) => ({
          availableModels: models,
          llmProfileModels: {
            ...state.llmProfileModels,
            [state.defaultLlmProfileIds.ollama]: models,
          },
        })),
    }),
    {
      name: 'open-cowork-config',
      partialize: (state) => ({
        ollama: state.ollama,
        llmProfiles: sanitizeProfilesForPersistence(state.llmProfiles),
        defaultLlmProfileIds: state.defaultLlmProfileIds,
        llmProfileModels: state.llmProfileModels,
        preferences: state.preferences,
        mcpServer: sanitizeMcpServerForPersistence(state.mcpServer),
        mcpServers: state.mcpServers.map(sanitizeMcpServerForPersistence),
        activeMcpServerName: state.activeMcpServerName,
        availableModels: state.availableModels,
      }),
      merge: (persisted, current) => {
        const state = persisted as Partial<ConfigState>
        const persistedState = { ...(state as Partial<ConfigState> & {
          openAIComputerUse?: unknown
        }) }
        delete persistedState.openAIComputerUse
        const persistedServers = Array.isArray(state.mcpServers) ? state.mcpServers : []
        const normalizedServers = persistedServers
          .map(normalizeServer)
          .map(migrateServer)
          .filter((server) => server.name && server.command)
        const dedupedByName = Array.from(
          new Map(normalizedServers.map((server) => [server.name, server])).values(),
        )
        const migratedCurrent = state.mcpServer
          ? migrateServer(normalizeServer(state.mcpServer))
          : undefined
        const mcpServers = dedupedByName.length > 0 ? dedupedByName : [migratedCurrent ?? DEFAULT_MCP]
        const activeMcpServerName = state.activeMcpServerName || migratedCurrent?.name || mcpServers[0].name
        const llmProfiles = ensureLlmProfiles(state.ollama, state.llmProfiles)
        const defaultLlmProfileIds = ensureDefaultLlmProfileIds(state.defaultLlmProfileIds, llmProfiles)
        const availableModels = Array.isArray(state.availableModels) ? state.availableModels : []
        const llmProfileModels: Record<string, string[]> = {
          [defaultLlmProfileIds.ollama]: availableModels,
          ...(state.llmProfileModels ?? {}),
        }
        const syncedLlmProfiles = llmProfiles.map((profile) => {
          if (profile.preset === 'ollama') return profile
          const resolvedModel = resolveProviderModelFromCatalog(
            profile.model,
            llmProfileModels[profile.id] ?? [],
          )
          return resolvedModel && resolvedModel !== profile.model
            ? normalizeLlmProfile({ ...profile, model: resolvedModel })
            : profile
        })
        return {
          ...current,
          ...persistedState,
          ollama: syncLegacyOllamaConfig(syncedLlmProfiles, defaultLlmProfileIds, state.ollama),
          llmProfiles: syncedLlmProfiles,
          defaultLlmProfileIds,
          llmProfileModels,
          preferences: {
            ...DEFAULT_PREFERENCES,
            ...(state.preferences ?? {}),
          },
          mcpServers,
          activeMcpServerName,
          mcpServer: chooseServer(mcpServers, activeMcpServerName),
          availableModels,
        }
      },
    }
  )
)
