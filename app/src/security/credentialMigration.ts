import { normalizeLlmProfile, useConfigStore, type LlmProfile, type McpServerConfig } from '../stores/configStore'
import { useCoworkStore } from '../stores/coworkStore'
import { useCrewStore, type Crew, type CrewProviderKind } from '../stores/crewStore'
import { useEngineStore } from '../stores/engineStore'
import { sanitizeAppLogEntry, useLogStore } from '../stores/logStore'
import {
  migrateLegacyMemoryProviderConfigs,
  migrateLegacyToolGatewayConfigs,
} from './legacyConfigMigration'
import { hasTauriRuntime, safeInvoke } from '../utils/safeInvoke'
import {
  connectorLocator,
  copyCredentialIfMissing,
  crewProviderLocator,
  getCredential,
  hasCredential,
  llmApiKeyLocator,
  mcpCredentialOwner,
  setCredential,
  type CredentialLocator,
} from './credentialVault'
import type { ChatProviderSelection } from '../utils/chatProvider'

let initialization: Promise<void> | null = null

type ApiProfileRow = {
  id: string
  name: string
  preset: string
  authMode: string
  baseUrl: string
  model: string
  timeoutMs: number
  verifyTlsCertificates: boolean
  contextWindow: number | null
  temperature: number | null
  isExample: boolean
  createdAt: string
  updatedAt: string
}

function toApiProfileRow(profile: LlmProfile): ApiProfileRow {
  const timestamp = new Date().toISOString()
  return {
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
    isExample: profile.id.startsWith('default-'),
    createdAt: timestamp,
    updatedAt: timestamp,
  }
}

type ImportedCrewProfile = {
  crewId: string
  legacyProvider: 'openai-compatible' | 'openrouter'
  source: CredentialLocator
  profile: LlmProfile
}

function importedCrewProfileId(crewId: string, provider: 'openai-compatible' | 'openrouter'): string {
  return `imported-crew-${crewId}-${provider === 'openrouter' ? 'openrouter' : 'openai'}`
}

function collectImportedCrewProfiles(crews: Crew[]): ImportedCrewProfile[] {
  return crews.flatMap((crew) => {
    const rows: ImportedCrewProfile[] = []
    const openAi = crew.providerProfiles.openAICompatible
    if (openAi.enabled) {
      const preset = openAi.baseUrl.toLowerCase().includes('api.openai.com') ? 'openai' : 'custom'
      rows.push({
        crewId: crew.id,
        legacyProvider: 'openai-compatible',
        source: crewProviderLocator(crew.id, 'openai_compatible'),
        profile: normalizeLlmProfile({
          id: importedCrewProfileId(crew.id, 'openai-compatible'),
          name: `Importiert: ${crew.name} · OpenAI-kompatibel`,
          provider: 'openai-compatible',
          preset,
          authMode: 'bearer',
          baseUrl: openAi.baseUrl,
          model: openAi.model,
          apiKey: openAi.apiKey,
          hasApiKey: openAi.hasApiKey,
          timeoutMs: openAi.timeoutMs,
          verifyTlsCertificates: openAi.verifyTlsCertificates,
          contextWindow: 128000,
          temperature: null,
        }),
      })
    }
    const openRouter = crew.providerProfiles.openRouter
    if (openRouter.enabled) {
      rows.push({
        crewId: crew.id,
        legacyProvider: 'openrouter',
        source: crewProviderLocator(crew.id, 'openrouter'),
        profile: normalizeLlmProfile({
          id: importedCrewProfileId(crew.id, 'openrouter'),
          name: `Importiert: ${crew.name} · OpenRouter`,
          provider: 'openai-compatible',
          preset: 'openrouter',
          authMode: 'bearer',
          baseUrl: openRouter.baseUrl,
          model: openRouter.model,
          apiKey: openRouter.apiKey,
          hasApiKey: openRouter.hasApiKey,
          timeoutMs: openRouter.timeoutMs,
          verifyTlsCertificates: openRouter.verifyTlsCertificates,
          contextWindow: 128000,
          temperature: null,
        }),
      })
    }
    return rows
  })
}

function migrateLegacyCrewSelections(
  crew: Crew,
  imported: ImportedCrewProfile[],
  defaultProfileIds: { api?: string; ollama: string; 'openai-compatible': string; openrouter: string },
): Crew {
  const importedId = (provider: CrewProviderKind): string | undefined => imported.find((entry) => (
    entry.crewId === crew.id && entry.legacyProvider === provider
  ))?.profile.id
  const legacySelection = (provider: CrewProviderKind | undefined, model?: string | null): ChatProviderSelection => {
    if (provider === 'codex') return { backend: 'codex', ...(model?.trim() ? { model: model.trim() } : {}) }
    const profileId = provider === 'openrouter'
      ? importedId('openrouter') ?? defaultProfileIds.openrouter
      : provider === 'openai-compatible'
        ? importedId('openai-compatible') ?? defaultProfileIds['openai-compatible']
        : defaultProfileIds.ollama
    return {
      backend: 'openai-compatible',
      profileId,
      ...(model?.trim() ? { model: model.trim() } : {}),
    }
  }
  const defaultBackendSelection = crew.defaultBackendSelection
    ?? legacySelection(crew.defaultProvider, crew.defaultModel)
  return {
    ...crew,
    defaultBackendSelection,
    agents: crew.agents.map((agent) => {
      if (agent.backendSelection || agent.providerKind === crew.defaultProvider) return agent
      return { ...agent, backendSelection: legacySelection(agent.providerKind, agent.modelOverride) }
    }),
  }
}

async function migrateOrRead(locator: CredentialLocator, legacyValue: string): Promise<string> {
  const storedValue = await getCredential(locator)
  if (storedValue !== null) return storedValue
  if (!legacyValue) return ''
  await setCredential(locator, legacyValue)
  return legacyValue
}

async function hydrateMcpServer(server: McpServerConfig): Promise<McpServerConfig> {
  const ownerId = mcpCredentialOwner(server)
  const entries = await Promise.all(Object.entries(server.env ?? {}).map(async ([field, value]) => [
    field,
    await migrateOrRead({ scope: 'mcp_env', ownerId, field }, value),
  ] as const))
  return { ...server, env: Object.fromEntries(entries) }
}

async function initializeCredentialVaultOnce(): Promise<void> {
  if (hasTauriRuntime()) {
    await safeInvoke('secure_config_migrate')
  }
  await Promise.all([
    migrateLegacyMemoryProviderConfigs(),
    migrateLegacyToolGatewayConfigs(),
  ])
  const configState = useConfigStore.getState()
  const coworkState = useCoworkStore.getState()
  const crewState = useCrewStore.getState()
  const engineState = useEngineStore.getState()
  const sanitizedLogs = useLogStore.getState().entries.map(sanitizeAppLogEntry)
  const configuredMcpServers = configState.mcpServers.length > 0
    ? configState.mcpServers
    : [configState.mcpServer]
  const importedCrewProfiles = collectImportedCrewProfiles(crewState.crews)
  const profilesForMigration = Array.from(new Map([
    ...importedCrewProfiles.map((entry) => [entry.profile.id, entry.profile] as const),
    ...configState.llmProfiles.map((profile) => [profile.id, profile] as const),
  ]).values())

  const [initialLlmProfiles, connectors, crews, mcpServers, engineApiKey] = await Promise.all([
    Promise.all(profilesForMigration.map(async (profile) => {
      const locator = llmApiKeyLocator(profile.id)
      const legacyApiKey = profile.apiKey.trim()
      if (legacyApiKey) await setCredential(locator, legacyApiKey)
      return {
        ...profile,
        apiKey: '',
        hasApiKey: legacyApiKey ? true : await hasCredential(locator),
      }
    })),
    Promise.all(coworkState.connectors.map(async (connector) => ({
      ...connector,
      apiKey: connector.apiKey === undefined
        ? undefined
        : await migrateOrRead(connectorLocator(connector.key, 'api_key'), connector.apiKey),
      webhookUrl: connector.webhookUrl === undefined
        ? undefined
        : await migrateOrRead(connectorLocator(connector.key, 'webhook_url'), connector.webhookUrl),
    }))),
    Promise.all(crewState.crews.map(async (crew) => {
      const openAiLocator = crewProviderLocator(crew.id, 'openai_compatible')
      const openRouterLocator = crewProviderLocator(crew.id, 'openrouter')
      const legacyOpenAiKey = crew.providerProfiles.openAICompatible.apiKey.trim()
      const legacyOpenRouterKey = crew.providerProfiles.openRouter.apiKey.trim()
      if (legacyOpenAiKey) await setCredential(openAiLocator, legacyOpenAiKey)
      if (legacyOpenRouterKey) await setCredential(openRouterLocator, legacyOpenRouterKey)
      return {
        ...crew,
        providerProfiles: {
          ...crew.providerProfiles,
          openAICompatible: {
            ...crew.providerProfiles.openAICompatible,
            apiKey: '',
            hasApiKey: legacyOpenAiKey ? true : await hasCredential(openAiLocator),
          },
          openRouter: {
            ...crew.providerProfiles.openRouter,
            apiKey: '',
            hasApiKey: legacyOpenRouterKey ? true : await hasCredential(openRouterLocator),
          },
        },
      }
    })),
    Promise.all(configuredMcpServers.map(hydrateMcpServer)),
    (async () => {
      const source = { scope: 'engine', ownerId: 'legacy-engine', field: 'api_key' } as const
      const legacyValue = engineState.config.apiKey.trim()
      if (legacyValue) await setCredential(source, legacyValue)
      return ''
    })(),
  ])

  let llmProfiles = initialLlmProfiles
  const legacyEngineDestinationId = configState.defaultLlmProfileIds.api
    ?? configState.defaultLlmProfileIds['openai-compatible']
  if (legacyEngineDestinationId) {
    await copyCredentialIfMissing(
      { scope: 'engine', ownerId: 'legacy-engine', field: 'api_key' },
      llmApiKeyLocator(legacyEngineDestinationId),
    )
  }
  await Promise.all(importedCrewProfiles.map((entry) => copyCredentialIfMissing(
    entry.source,
    llmApiKeyLocator(entry.profile.id),
  )))
  llmProfiles = await Promise.all(llmProfiles.map(async (profile) => ({
    ...profile,
    apiKey: '',
    hasApiKey: await hasCredential(llmApiKeyLocator(profile.id)),
  })))
  const migratedCrews = crews.map((crew) => migrateLegacyCrewSelections(
    crew,
    importedCrewProfiles,
    configState.defaultLlmProfileIds,
  ))

  let persistedLlmProfiles: LlmProfile[] = llmProfiles
  if (hasTauriRuntime()) {
    await Promise.all(llmProfiles.map((profile) => safeInvoke('api_profile_upsert', {
      profile: toApiProfileRow(profile),
    })))
    const databaseProfiles = await safeInvoke<ApiProfileRow[]>('api_profile_list')
    persistedLlmProfiles = databaseProfiles.map((row) => {
      const hydrated = llmProfiles.find((profile) => profile.id === row.id)
      return normalizeLlmProfile({
        ...row,
        provider: 'openai-compatible',
        preset: row.preset as LlmProfile['preset'],
        authMode: row.authMode as LlmProfile['authMode'],
        apiKey: '',
        hasApiKey: hydrated?.hasApiKey ?? false,
      })
    })
  }

  const activeMcpServer = mcpServers.find((server) => (
    server.name === configState.activeMcpServerName
  )) ?? mcpServers[0] ?? configState.mcpServer

  useConfigStore.setState({
    llmProfiles: persistedLlmProfiles,
    mcpServers,
    mcpServer: activeMcpServer,
  })
  useCoworkStore.setState({ connectors })
  useCrewStore.setState({ crews: migratedCrews })
  useEngineStore.setState((state) => ({
    config: { ...state.config, apiKey: engineApiKey },
  }))
  useLogStore.setState({ entries: sanitizedLogs })
}

export function initializeCredentialVault(): Promise<void> {
  if (!initialization) {
    initialization = initializeCredentialVaultOnce().catch((error) => {
      initialization = null
      throw error
    })
  }
  return initialization
}

export function resetCredentialInitializationForTests(): void {
  initialization = null
}
