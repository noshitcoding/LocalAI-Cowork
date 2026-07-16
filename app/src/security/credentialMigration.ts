import { useConfigStore, type McpServerConfig } from '../stores/configStore'
import { useCoworkStore } from '../stores/coworkStore'
import { useCrewStore } from '../stores/crewStore'
import { useEngineStore } from '../stores/engineStore'
import { sanitizeAppLogEntry, useLogStore } from '../stores/logStore'
import {
  migrateLegacyMemoryProviderConfigs,
  migrateLegacyToolGatewayConfigs,
} from './legacyConfigMigration'
import { hasTauriRuntime, safeInvoke } from '../utils/safeInvoke'
import {
  connectorLocator,
  crewProviderLocator,
  getCredential,
  llmApiKeyLocator,
  mcpCredentialOwner,
  setCredential,
  type CredentialLocator,
} from './credentialVault'

let initialization: Promise<void> | null = null

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

  const [llmProfiles, connectors, crews, mcpServers, engineApiKey] = await Promise.all([
    Promise.all(configState.llmProfiles.map(async (profile) => ({
      ...profile,
      apiKey: await migrateOrRead(llmApiKeyLocator(profile.id), profile.apiKey),
    }))),
    Promise.all(coworkState.connectors.map(async (connector) => ({
      ...connector,
      apiKey: connector.apiKey === undefined
        ? undefined
        : await migrateOrRead(connectorLocator(connector.key, 'api_key'), connector.apiKey),
      webhookUrl: connector.webhookUrl === undefined
        ? undefined
        : await migrateOrRead(connectorLocator(connector.key, 'webhook_url'), connector.webhookUrl),
    }))),
    Promise.all(crewState.crews.map(async (crew) => ({
      ...crew,
      providerProfiles: {
        ...crew.providerProfiles,
        openAICompatible: {
          ...crew.providerProfiles.openAICompatible,
          apiKey: await migrateOrRead(
            crewProviderLocator(crew.id, 'openai_compatible'),
            crew.providerProfiles.openAICompatible.apiKey,
          ),
        },
        openRouter: {
          ...crew.providerProfiles.openRouter,
          apiKey: await migrateOrRead(
            crewProviderLocator(crew.id, 'openrouter'),
            crew.providerProfiles.openRouter.apiKey,
          ),
        },
      },
    }))),
    Promise.all(configuredMcpServers.map(hydrateMcpServer)),
    migrateOrRead(
      { scope: 'engine', ownerId: 'legacy-engine', field: 'api_key' },
      engineState.config.apiKey,
    ),
  ])

  const activeMcpServer = mcpServers.find((server) => (
    server.name === configState.activeMcpServerName
  )) ?? mcpServers[0] ?? configState.mcpServer

  useConfigStore.setState({
    llmProfiles,
    mcpServers,
    mcpServer: activeMcpServer,
  })
  useCoworkStore.setState({ connectors })
  useCrewStore.setState({ crews })
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
