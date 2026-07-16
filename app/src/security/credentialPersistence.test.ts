import { beforeEach, describe, expect, it } from 'vitest'
import { useConfigStore, type McpServerConfig } from '../stores/configStore'
import { useCoworkStore } from '../stores/coworkStore'
import { useCrewStore } from '../stores/crewStore'
import { useEngineStore } from '../stores/engineStore'
import {
  initializeCredentialVault,
  resetCredentialInitializationForTests,
} from './credentialMigration'
import { resetVolatileCredentialsForTests } from './credentialVault'

const SENTINELS = {
  llm: 'sentinel-llm-api-key',
  connector: 'sentinel-connector-api-key',
  webhook: 'https://example.test/hook/sentinel-webhook-token',
  crewOpenAi: 'sentinel-crew-openai-key',
  crewOpenRouter: 'sentinel-crew-openrouter-key',
  mcp: 'sentinel-mcp-environment-value',
  engine: 'sentinel-engine-api-key',
  legacyJson: 'sentinel-legacy-json-config',
}

function localStorageContents(): string {
  return Object.keys(localStorage)
    .map((key) => localStorage.getItem(key) ?? '')
    .join('\n')
}

function configureSecretBearingState() {
  useConfigStore.setState((state) => {
    const llmProfiles = state.llmProfiles.map((profile) => (
      profile.id === 'default-openai-compatible'
        ? { ...profile, apiKey: SENTINELS.llm }
        : profile
    ))
    const server: McpServerConfig = {
      id: 'mcp-secret-test',
      name: 'secret-test',
      command: 'node',
      args: 'server.js',
      env: { SERVICE_TOKEN: SENTINELS.mcp },
    }
    return {
      llmProfiles,
      mcpServers: [server],
      mcpServer: server,
      activeMcpServerName: server.name,
    }
  })

  useCoworkStore.setState((state) => ({
    connectors: state.connectors.map((connector) => (
      connector.key === 'slack'
        ? { ...connector, apiKey: SENTINELS.connector, webhookUrl: SENTINELS.webhook }
        : connector
    )),
  }))

  useCrewStore.setState({ crews: [] })
  useCrewStore.getState().createCrew('crew-secret-test', 'Secret Test', [])
  useCrewStore.setState((state) => ({
    crews: state.crews.map((crew) => (
      crew.id === 'crew-secret-test'
        ? {
            ...crew,
            providerProfiles: {
              openAICompatible: {
                ...crew.providerProfiles.openAICompatible,
                apiKey: SENTINELS.crewOpenAi,
              },
              openRouter: {
                ...crew.providerProfiles.openRouter,
                apiKey: SENTINELS.crewOpenRouter,
              },
            },
          }
        : crew
    )),
  }))

  useEngineStore.setState((state) => ({
    config: { ...state.config, apiKey: SENTINELS.engine },
  }))
}

function clearRuntimeSecrets() {
  useConfigStore.setState((state) => ({
    llmProfiles: state.llmProfiles.map((profile) => ({ ...profile, apiKey: '' })),
    mcpServers: state.mcpServers.map((server) => ({
      ...server,
      env: Object.fromEntries(Object.keys(server.env).map((key) => [key, ''])),
    })),
    mcpServer: {
      ...state.mcpServer,
      env: Object.fromEntries(Object.keys(state.mcpServer.env).map((key) => [key, ''])),
    },
  }))
  useCoworkStore.setState((state) => ({
    connectors: state.connectors.map((connector) => ({
      ...connector,
      apiKey: connector.apiKey === undefined ? undefined : '',
      webhookUrl: connector.webhookUrl === undefined ? undefined : '',
    })),
  }))
  useCrewStore.setState((state) => ({
    crews: state.crews.map((crew) => ({
      ...crew,
      providerProfiles: {
        openAICompatible: { ...crew.providerProfiles.openAICompatible, apiKey: '' },
        openRouter: { ...crew.providerProfiles.openRouter, apiKey: '' },
      },
    })),
  }))
  useEngineStore.setState((state) => ({
    config: { ...state.config, apiKey: '' },
  }))
}

beforeEach(() => {
  delete (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__
  localStorage.clear()
  resetCredentialInitializationForTests()
  resetVolatileCredentialsForTests()
})

describe('credential persistence boundary', () => {
  it('keeps every supported secret out of localStorage and restores it after restart', async () => {
    localStorage.setItem('open-cowork-providers-local', JSON.stringify([{
      id: 'legacy-memory-config',
      name: 'Legacy memory config',
      provider_type: 'custom',
      config_json: JSON.stringify({ arbitrary: SENTINELS.legacyJson }),
      enabled: true,
    }]))
    localStorage.setItem('open-cowork-gateway', JSON.stringify([{
      id: 'legacy-gateway-config',
      name: 'Legacy gateway config',
      tool_type: 'custom',
      config_json: JSON.stringify({ arbitrary: SENTINELS.legacyJson }),
      enabled: true,
    }]))
    configureSecretBearingState()
    await initializeCredentialVault()

    expect(localStorage.getItem('open-cowork-providers-local')).toBeNull()
    expect(localStorage.getItem('open-cowork-gateway')).toBeNull()
    const persisted = localStorageContents()
    Object.values(SENTINELS).forEach((secret) => {
      expect(persisted).not.toContain(secret)
    })
    expect(persisted).toContain('SERVICE_TOKEN')

    clearRuntimeSecrets()
    resetCredentialInitializationForTests()
    await initializeCredentialVault()

    const config = useConfigStore.getState()
    expect(config.llmProfiles.find((profile) => profile.id === 'default-openai-compatible')?.apiKey)
      .toBe(SENTINELS.llm)
    expect(config.mcpServer.env.SERVICE_TOKEN).toBe(SENTINELS.mcp)
    const connector = useCoworkStore.getState().connectors.find((entry) => entry.key === 'slack')
    expect(connector?.apiKey).toBe(SENTINELS.connector)
    expect(connector?.webhookUrl).toBe(SENTINELS.webhook)
    const crew = useCrewStore.getState().crews.find((entry) => entry.id === 'crew-secret-test')
    expect(crew?.providerProfiles.openAICompatible.apiKey).toBe(SENTINELS.crewOpenAi)
    expect(crew?.providerProfiles.openRouter.apiKey).toBe(SENTINELS.crewOpenRouter)
    expect(useEngineStore.getState().config.apiKey).toBe(SENTINELS.engine)

    const persistedAfterRestart = localStorageContents()
    Object.values(SENTINELS).forEach((secret) => {
      expect(persistedAfterRestart).not.toContain(secret)
    })
  })
})
