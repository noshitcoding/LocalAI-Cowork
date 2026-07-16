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

export type OllamaConfig = {
  baseUrl: string
  model: string
  timeoutMs: number
  contextWindow: number
  temperature: number
}

export type LlmProviderKind = 'ollama' | 'openai-compatible' | 'openrouter'

export type LlmProfile = {
  id: string
  name: string
  provider: LlmProviderKind
  baseUrl: string
  model: string
  apiKey: string
  timeoutMs: number
  verifyTlsCertificates: boolean
  contextWindow: number | null
  temperature: number | null
}

export type DefaultLlmProfileIds = Record<LlmProviderKind, string>

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
  addLlmProfile: (provider: LlmProviderKind) => string
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
  ollama: 'default-ollama',
  'openai-compatible': 'default-openai-compatible',
  openrouter: 'default-openrouter',
}

function createBaseLlmProfile(provider: LlmProviderKind): LlmProfile {
  return provider === 'ollama'
    ? {
        id: DEFAULT_LLM_PROFILE_IDS.ollama,
        name: 'Lokales Ollama',
        provider,
        baseUrl: DEFAULT_OLLAMA.baseUrl,
        model: DEFAULT_OLLAMA.model,
        apiKey: '',
        timeoutMs: DEFAULT_OLLAMA.timeoutMs,
        verifyTlsCertificates: true,
        contextWindow: DEFAULT_OLLAMA.contextWindow,
        temperature: DEFAULT_OLLAMA.temperature,
      }
    : provider === 'openai-compatible'
      ? {
          id: DEFAULT_LLM_PROFILE_IDS['openai-compatible'],
          name: 'OpenAI-compatible',
          provider,
          baseUrl: DEFAULT_OPENAI_COMPATIBLE_PROFILE.baseUrl,
          model: DEFAULT_OPENAI_COMPATIBLE_PROFILE.model,
          apiKey: DEFAULT_OPENAI_COMPATIBLE_PROFILE.apiKey,
          timeoutMs: DEFAULT_OPENAI_COMPATIBLE_PROFILE.timeoutMs,
          verifyTlsCertificates: true,
          contextWindow: null,
          temperature: null,
        }
      : {
          id: DEFAULT_LLM_PROFILE_IDS.openrouter,
          name: 'OpenRouter',
          provider,
          baseUrl: 'https://openrouter.ai/api/v1',
          model: '',
          apiKey: '',
          timeoutMs: DEFAULT_OLLAMA.timeoutMs,
          verifyTlsCertificates: true,
          contextWindow: null,
          temperature: null,
        }
}

function normalizeLlmProfile(profile: Partial<LlmProfile> & Pick<LlmProfile, 'provider'>): LlmProfile {
  const baseProfile = createBaseLlmProfile(profile.provider)
  const rawTimeout = Number(profile.timeoutMs ?? baseProfile.timeoutMs)
  const rawContextWindow = profile.contextWindow ?? baseProfile.contextWindow
  const rawTemperature = profile.temperature ?? baseProfile.temperature
  const normalizedModel = profile.model?.trim()

  return {
    ...baseProfile,
    ...profile,
    name: profile.name?.trim() || baseProfile.name,
    baseUrl: profile.baseUrl?.trim() || baseProfile.baseUrl,
    model: normalizedModel ?? baseProfile.model,
    apiKey: profile.apiKey?.trim() ?? baseProfile.apiKey,
    timeoutMs: Math.max(1000, Number.isFinite(rawTimeout) ? rawTimeout : baseProfile.timeoutMs),
    verifyTlsCertificates: profile.verifyTlsCertificates ?? baseProfile.verifyTlsCertificates,
    contextWindow: profile.provider === 'ollama'
      ? Math.max(512, Number.isFinite(Number(rawContextWindow)) ? Number(rawContextWindow) : DEFAULT_OLLAMA.contextWindow)
      : null,
    temperature: profile.provider === 'ollama'
      ? (Number.isFinite(Number(rawTemperature)) ? Number(rawTemperature) : DEFAULT_OLLAMA.temperature)
      : null,
  }
}

function createDefaultLlmProfile(provider: LlmProviderKind, overrides: Partial<LlmProfile> = {}): LlmProfile {
  return normalizeLlmProfile({
    ...createBaseLlmProfile(provider),
    ...overrides,
    provider,
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
    createDefaultLlmProfile('openai-compatible'),
    createDefaultLlmProfile('openrouter'),
  ]
}

function ensureLlmProfiles(
  legacyOllama: Partial<OllamaConfig> | undefined,
  profiles: LlmProfile[] | undefined,
): LlmProfile[] {
  const fallbackProfiles = buildDefaultLlmProfiles(legacyOllama)
  const byId = new Map<string, LlmProfile>(fallbackProfiles.map((profile) => [profile.id, profile]))

  ;(profiles ?? []).forEach((profile) => {
    if (!profile?.id || !profile.provider) {
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
    ollama: defaultIds?.ollama ?? DEFAULT_LLM_PROFILE_IDS.ollama,
    'openai-compatible': defaultIds?.['openai-compatible'] ?? DEFAULT_LLM_PROFILE_IDS['openai-compatible'],
    openrouter: defaultIds?.openrouter ?? DEFAULT_LLM_PROFILE_IDS.openrouter,
  }

  const resolveProviderFallback = (provider: LlmProviderKind) => {
    return profiles.find((profile) => profile.provider === provider)?.id ?? createDefaultLlmProfile(provider).id
  }

  if (!profiles.some((profile) => profile.id === nextIds.ollama && profile.provider === 'ollama')) {
    nextIds.ollama = resolveProviderFallback('ollama')
  }
  if (!profiles.some((profile) => profile.id === nextIds['openai-compatible'] && profile.provider === 'openai-compatible')) {
    nextIds['openai-compatible'] = resolveProviderFallback('openai-compatible')
  }
  if (!profiles.some((profile) => profile.id === nextIds.openrouter && profile.provider === 'openrouter')) {
    nextIds.openrouter = resolveProviderFallback('openrouter')
  }

  return nextIds
}

function resolveDefaultLlmProfile(
  profiles: LlmProfile[],
  defaultIds: DefaultLlmProfileIds,
  provider: LlmProviderKind,
): LlmProfile {
  return profiles.find((profile) => profile.id === defaultIds[provider] && profile.provider === provider)
    ?? profiles.find((profile) => profile.provider === provider)
    ?? createDefaultLlmProfile(provider)
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
    baseUrl: activeProfile.baseUrl || currentOllama?.baseUrl || DEFAULT_OLLAMA.baseUrl,
    model: activeProfile.model || currentOllama?.model || DEFAULT_OLLAMA.model,
    timeoutMs: Math.max(DEFAULT_OLLAMA.timeoutMs, activeProfile.timeoutMs || currentOllama?.timeoutMs || DEFAULT_OLLAMA.timeoutMs),
    contextWindow: activeProfile.contextWindow ?? currentOllama?.contextWindow ?? DEFAULT_OLLAMA.contextWindow,
    temperature: activeProfile.temperature ?? currentOllama?.temperature ?? DEFAULT_OLLAMA.temperature,
  }
}

function createLlmProfileId(provider: LlmProviderKind): string {
  return `${provider}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
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
  return command === 'open-cowork-docs-mcp' || name === 'local-docs'
}

function isLegacyscreenshotServer(server: McpServerConfig): boolean {
  const command = server.command.trim().toLowerCase()
  const name = server.name.trim().toLowerCase()
  return command === 'open-cowork-screenshot-mcp' || name === 'screenshot'
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
                  provider: 'ollama',
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
      addLlmProfile: (provider) => {
        const id = createLlmProfileId(provider)
        set((state) => ({
          llmProfiles: [
            ...state.llmProfiles,
            createDefaultLlmProfile(provider, {
              id,
              name: `${provider === 'ollama' ? 'Ollama' : provider === 'openai-compatible' ? 'OpenAI-compatible' : 'OpenRouter'} ${state.llmProfiles.filter((profile) => profile.provider === provider).length + 1}`,
            }),
          ],
          llmProfileModels: {
            ...state.llmProfileModels,
            [id]: [],
          },
        }))
        return id
      },
      updateLlmProfile: (id, patch) =>
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
        }),
      setLlmProfileApiKey: async (id, apiKey) => {
        if (!useConfigStore.getState().llmProfiles.some((profile) => profile.id === id)) return
        await setCredential(llmApiKeyLocator(id), apiKey)
        set((state) => ({
          llmProfiles: state.llmProfiles.map((profile) => (
            profile.id === id ? { ...profile, apiKey } : profile
          )),
        }))
      },
      deleteLlmProfile: async (id) => {
        if (Object.values(useConfigStore.getState().defaultLlmProfileIds).includes(id)) return
        await deleteCredential(llmApiKeyLocator(id))
        set((state) => {
          if (Object.values(state.defaultLlmProfileIds).includes(id)) {
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
      setDefaultLlmProfile: (provider, id) =>
        set((state) => {
          const profile = state.llmProfiles.find((item) => item.id === id && item.provider === provider)
          if (!profile) {
            return state
          }

          const defaultLlmProfileIds = {
            ...state.defaultLlmProfileIds,
            [provider]: id,
          }

          return {
            defaultLlmProfileIds,
            ollama: provider === 'ollama'
              ? syncLegacyOllamaConfig(state.llmProfiles, defaultLlmProfileIds, state.ollama)
              : state.ollama,
            availableModels: provider === 'ollama'
              ? state.llmProfileModels[id] ?? []
              : state.availableModels,
          }
        }),
      setLlmProfileModels: (id, models) =>
        set((state) => ({
          llmProfileModels: {
            ...state.llmProfileModels,
            [id]: models,
          },
          availableModels: id === state.defaultLlmProfileIds.ollama ? models : state.availableModels,
        })),
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
        const llmProfileModels = {
          [defaultLlmProfileIds.ollama]: availableModels,
          ...(state.llmProfileModels ?? {}),
        }
        return {
          ...current,
          ...persistedState,
          ollama: syncLegacyOllamaConfig(llmProfiles, defaultLlmProfileIds, state.ollama),
          llmProfiles,
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
