import { create } from 'zustand'
import { persist } from 'zustand/middleware'

export type OllamaConfig = {
  baseUrl: string
  model: string
  timeoutMs: number
  contextWindow: number
  temperature: number
}

export type OpenAIComputerUseConfig = {
  apiKey: string
  baseUrl: string
  model: string
  maxSteps: number
  actionDelayMs: number
  launchDelayMs: number
  autoAcknowledgeSafetyChecks: boolean
}

export type McpServerConfig = {
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
}

type ConfigState = {
  ollama: OllamaConfig
  openAIComputerUse: OpenAIComputerUseConfig
  preferences: AppPreferences
  mcpServer: McpServerConfig
  mcpServers: McpServerConfig[]
  activeMcpServerName: string
  availableModels: string[]
  setOllama: (patch: Partial<OllamaConfig>) => void
  setOpenAIComputerUse: (patch: Partial<OpenAIComputerUseConfig>) => void
  setPreference: <K extends keyof AppPreferences>(key: K, value: AppPreferences[K]) => void
  setPreferences: (patch: Partial<AppPreferences>) => void
  setMcpServer: (patch: Partial<McpServerConfig>) => void
  setActiveMcpServer: (name: string) => void
  upsertMcpServer: (server: McpServerConfig) => void
  importMcpServers: (servers: McpServerConfig[]) => void
  deleteMcpServer: (name: string) => void
  setAvailableModels: (models: string[]) => void
}

const DEFAULT_OLLAMA: OllamaConfig = {
  baseUrl: 'http://192.168.178.82:11434',
  model: 'gpt-oss:20b',
  timeoutMs: 600000,
  contextWindow: 128000,
  temperature: 0.1,
}

const DEFAULT_OPENAI_COMPUTER_USE: OpenAIComputerUseConfig = {
  apiKey: '',
  baseUrl: 'https://api.openai.com/v1',
  model: 'computer-use-preview',
  maxSteps: 40,
  actionDelayMs: 900,
  launchDelayMs: 2000,
  autoAcknowledgeSafetyChecks: false,
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
  telemetryEnabled: true,
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
}

const DEFAULT_MCP: McpServerConfig = {
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

function isLegacyScreenshotServer(server: McpServerConfig): boolean {
  const command = server.command.trim().toLowerCase()
  const name = server.name.trim().toLowerCase()
  return command === 'open-cowork-screenshot-mcp' || name === 'screenshot'
}

function migrateServer(server: McpServerConfig): McpServerConfig {
  if (
    !isLegacyFilesystemServer(server)
    && !isLegacyLocalDocsServer(server)
    && !isLegacyScreenshotServer(server)
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
      openAIComputerUse: DEFAULT_OPENAI_COMPUTER_USE,
      preferences: DEFAULT_PREFERENCES,
      mcpServer: DEFAULT_MCP,
      mcpServers: [DEFAULT_MCP],
      activeMcpServerName: DEFAULT_MCP.name,
      availableModels: [],
      setOllama: (patch) =>
        set((state) => ({ ollama: { ...state.ollama, ...patch } })),
      setOpenAIComputerUse: (patch) =>
        set((state) => ({
          openAIComputerUse: { ...state.openAIComputerUse, ...patch },
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
      setActiveMcpServer: (name) =>
        set((state) => {
          const servers = state.mcpServers.length > 0 ? state.mcpServers : [state.mcpServer]
          return {
            activeMcpServerName: name,
            mcpServer: chooseServer(servers, name),
          }
        }),
      upsertMcpServer: (server) =>
        set((state) => {
          const normalized = normalizeServer(server)
          const existing = state.mcpServers.length > 0 ? state.mcpServers : [state.mcpServer]
          const servers = existing.some((item) => item.name === normalized.name)
            ? existing.map((item) => (item.name === normalized.name ? normalized : item))
            : [...existing, normalized]
          return {
            mcpServers: servers,
            activeMcpServerName: normalized.name,
            mcpServer: normalized,
          }
        }),
      importMcpServers: (serversToImport) =>
        set((state) => {
          const existing = state.mcpServers.length > 0 ? state.mcpServers : [state.mcpServer]
          const byName = new Map(existing.map((server) => [server.name, server]))
          serversToImport.map(normalizeServer).forEach((server) => {
            if (server.name && server.command) {
              byName.set(server.name, server)
            }
          })
          const servers = Array.from(byName.values())
          const activeMcpServerName = serversToImport[0]?.name ?? state.activeMcpServerName
          return {
            mcpServers: servers,
            activeMcpServerName,
            mcpServer: chooseServer(servers, activeMcpServerName),
          }
        }),
      deleteMcpServer: (name) =>
        set((state) => {
          const servers = (state.mcpServers.length > 0 ? state.mcpServers : [state.mcpServer])
            .filter((server) => server.name !== name)
          const fallback = servers[0] ?? DEFAULT_MCP
          return {
            mcpServers: servers.length > 0 ? servers : [DEFAULT_MCP],
            activeMcpServerName: fallback.name,
            mcpServer: fallback,
          }
        }),
      setAvailableModels: (models) => set({ availableModels: models }),
    }),
    {
      name: 'open-cowork-config',
      merge: (persisted, current) => {
        const state = persisted as Partial<ConfigState>
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
        return {
          ...current,
          ...state,
          ollama: {
            ...DEFAULT_OLLAMA,
            ...(state.ollama ?? {}),
            timeoutMs: Math.max(DEFAULT_OLLAMA.timeoutMs, state.ollama?.timeoutMs ?? DEFAULT_OLLAMA.timeoutMs),
          },
          openAIComputerUse: {
            ...DEFAULT_OPENAI_COMPUTER_USE,
            ...(state.openAIComputerUse ?? {}),
          },
          preferences: {
            ...DEFAULT_PREFERENCES,
            ...(state.preferences ?? {}),
          },
          mcpServers,
          activeMcpServerName,
          mcpServer: chooseServer(mcpServers, activeMcpServerName),
          availableModels: Array.isArray(state.availableModels) ? state.availableModels : [],
        }
      },
    }
  )
)
