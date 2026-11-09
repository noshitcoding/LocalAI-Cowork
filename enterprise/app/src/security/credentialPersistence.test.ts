import { beforeEach, describe, expect, it } from 'vitest'
import { useConfigStore, type McpServerConfig } from '../stores/configStore'
import { useCoworkStore } from '../stores/coworkStore'
import { useCrewStore } from '../stores/crewStore'
import { useEngineStore } from '../stores/engineStore'
import {
  hydrateDatabaseLlmProfiles,
  initializeCredentialVault,
  resetCredentialInitializationForTests,
  type ApiProfileRow,
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
  it('restores the credential status for a database-only profile after reinstall', async () => {
    const databaseProfile: ApiProfileRow = {
      id: 'openrouter-from-database',
      name: 'Persisted OpenRouter',
      preset: 'openrouter',
      authMode: 'bearer',
      baseUrl: 'https://openrouter.ai/api/v1',
      model: 'openai/gpt-test',
      timeoutMs: 600_000,
      verifyTlsCertificates: true,
      contextWindow: 128_000,
      temperature: null,
      isExample: false,
      createdAt: '2026-08-15T10:00:00.000Z',
      updatedAt: '2026-08-15T10:00:00.000Z',
    }

    const profiles = await hydrateDatabaseLlmProfiles(
      [databaseProfile],
      async (profileId) => profileId === databaseProfile.id,
    )

    expect(profiles).toHaveLength(1)
    expect(profiles[0]).toMatchObject({
      id: databaseProfile.id,
      apiKey: '',
      hasApiKey: true,
    })
  })

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
      .toBe('')
    expect(config.llmProfiles.find((profile) => profile.id === 'default-openai-compatible')?.hasApiKey)
      .toBe(true)
    expect(config.mcpServer.env.SERVICE_TOKEN).toBe(SENTINELS.mcp)
    const connector = useCoworkStore.getState().connectors.find((entry) => entry.key === 'slack')
    expect(connector?.apiKey).toBe(SENTINELS.connector)
    expect(connector?.webhookUrl).toBe(SENTINELS.webhook)
    const crew = useCrewStore.getState().crews.find((entry) => entry.id === 'crew-secret-test')
    expect(crew?.providerProfiles.openAICompatible.apiKey).toBe('')
    expect(crew?.providerProfiles.openAICompatible.hasApiKey).toBe(true)
    expect(crew?.providerProfiles.openRouter.apiKey).toBe('')
    expect(crew?.providerProfiles.openRouter.hasApiKey).toBe(true)
    expect(useEngineStore.getState().config.apiKey).toBe('')

    const persistedAfterRestart = localStorageContents()
    Object.values(SENTINELS).forEach((secret) => {
      expect(persistedAfterRestart).not.toContain(secret)
    })
  })

  it('imports a divergent legacy crew provider once and pins its vault-backed profile', async () => {
    useConfigStore.setState((state) => ({
      llmProfiles: state.llmProfiles.filter((profile) => !profile.id.startsWith('imported-crew-')),
    }))
    useCrewStore.setState({ crews: [] })
    useCrewStore.getState().createCrew('crew-import-test', 'Imported Crew', [])
    useCrewStore.setState((state) => ({
      crews: state.crews.map((crew) => crew.id === 'crew-import-test'
        ? {
            ...crew,
            defaultProvider: 'openrouter' as const,
            defaultBackendSelection: undefined,
            providerProfiles: {
              ...crew.providerProfiles,
              openRouter: {
                ...crew.providerProfiles.openRouter,
                enabled: true,
                baseUrl: 'https://openrouter.ai/api/v1',
                model: 'openai/gpt-4o-mini',
                apiKey: SENTINELS.crewOpenRouter,
              },
            },
          }
        : crew),
    }))

    await initializeCredentialVault()

    const importedId = 'imported-crew-crew-import-test-openrouter'
    const imported = useConfigStore.getState().llmProfiles.filter((profile) => profile.id === importedId)
    expect(imported).toHaveLength(1)
    expect(imported[0]).toMatchObject({
      provider: 'openai-compatible',
      preset: 'openrouter',
      apiKey: '',
      hasApiKey: true,
    })
    expect(useCrewStore.getState().crews[0].defaultBackendSelection).toEqual({
      backend: 'openai-compatible',
      profileId: importedId,
    })

    clearRuntimeSecrets()
    resetCredentialInitializationForTests()
    await initializeCredentialVault()
    expect(useConfigStore.getState().llmProfiles.filter((profile) => profile.id === importedId)).toHaveLength(1)
  })

  it('uses a credential-bearing shared OpenRouter profile when a new crew import has no credential', async () => {
    const fallbackProfileId = 'default-openrouter'
    const crewId = 'crew-openrouter-new-import-test'

    useConfigStore.setState((state) => ({
      llmProfiles: state.llmProfiles.map((profile) => profile.id === fallbackProfileId
        ? { ...profile, apiKey: SENTINELS.llm, hasApiKey: false }
        : profile),
      defaultLlmProfileIds: {
        ...state.defaultLlmProfileIds,
        openrouter: fallbackProfileId,
      },
    }))
    useCrewStore.setState({ crews: [] })
    useCrewStore.getState().createCrew(crewId, 'OpenRouter New Import Crew', [])
    useCrewStore.setState((state) => ({
      crews: state.crews.map((crew) => crew.id === crewId
        ? {
            ...crew,
            defaultProvider: 'openrouter' as const,
            defaultBackendSelection: undefined,
            providerProfiles: {
              ...crew.providerProfiles,
              openRouter: {
                ...crew.providerProfiles.openRouter,
                enabled: true,
                baseUrl: 'https://openrouter.ai/api/v1',
                model: '',
                apiKey: '',
                hasApiKey: false,
              },
            },
          }
        : crew),
    }))

    await initializeCredentialVault()

    expect(useCrewStore.getState().crews[0].defaultBackendSelection).toEqual({
      backend: 'openai-compatible',
      profileId: fallbackProfileId,
    })
  })

  it('repairs a crew selection that points to an imported OpenRouter profile without a credential', async () => {
    const fallbackProfileId = 'default-openrouter'
    const crewId = 'crew-openrouter-fallback-test'
    const importedId = `imported-crew-${crewId}-openrouter`

    useConfigStore.setState((state) => {
      const fallbackProfile = state.llmProfiles.find((profile) => profile.id === fallbackProfileId)!
      return {
        llmProfiles: [
          ...state.llmProfiles.map((profile) => profile.id === fallbackProfileId
            ? { ...profile, apiKey: SENTINELS.llm, hasApiKey: false }
            : profile),
          {
            ...fallbackProfile,
            id: importedId,
            name: 'Deleted legacy Crew import',
            model: '',
            apiKey: '',
            hasApiKey: false,
          },
        ],
        defaultLlmProfileIds: {
          ...state.defaultLlmProfileIds,
          api: importedId,
          openrouter: importedId,
        },
      }
    })
    useCrewStore.setState({ crews: [], agents: [] })
    useCrewStore.getState().loadAgents()
    const agentId = useCrewStore.getState().agents[0].id
    useCrewStore.getState().createCrew(crewId, 'OpenRouter Fallback Crew', [agentId])
    useCrewStore.setState((state) => ({
      crews: state.crews.map((crew) => crew.id === crewId
        ? {
            ...crew,
            defaultProvider: 'openrouter' as const,
            defaultBackendSelection: {
              backend: 'openai-compatible' as const,
              profileId: importedId,
            },
            providerProfiles: {
              ...crew.providerProfiles,
              openRouter: {
                ...crew.providerProfiles.openRouter,
                enabled: true,
                baseUrl: 'https://openrouter.ai/api/v1',
                model: '',
                apiKey: '',
                hasApiKey: false,
              },
            },
            agents: crew.agents.map((agent) => ({
              ...agent,
              providerKind: 'openrouter' as const,
              backendSelection: {
                backend: 'openai-compatible' as const,
                profileId: importedId,
              },
            })),
          }
        : crew),
    }))

    await initializeCredentialVault()

    const migratedCrew = useCrewStore.getState().crews.find((crew) => crew.id === crewId)
    expect(migratedCrew?.defaultBackendSelection).toEqual({
      backend: 'openai-compatible',
      profileId: fallbackProfileId,
    })
    expect(migratedCrew?.agents[0]?.backendSelection).toEqual({
      backend: 'openai-compatible',
      profileId: fallbackProfileId,
    })
    expect(useConfigStore.getState().llmProfiles.find((profile) => profile.id === fallbackProfileId))
      .toMatchObject({ apiKey: '', hasApiKey: true })
    expect(useConfigStore.getState().llmProfiles.find((profile) => profile.id === importedId)).toBeUndefined()
    expect(useConfigStore.getState().defaultLlmProfileIds.api).toBe(fallbackProfileId)
    expect(useConfigStore.getState().defaultLlmProfileIds.openrouter).toBe(fallbackProfileId)

    clearRuntimeSecrets()
    resetCredentialInitializationForTests()
    await initializeCredentialVault()

    expect(useConfigStore.getState().llmProfiles.find((profile) => profile.id === importedId)).toBeUndefined()
  })
})
