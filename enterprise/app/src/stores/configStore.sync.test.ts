import { beforeEach, describe, expect, it, vi } from 'vitest'
import { MODEL_CATALOG_MAX_AGE_MS, providerCatalogTarget } from '../utils/providerModelCatalog'
import { getCredential, resetVolatileCredentialsForTests } from '../security/credentialVault'
import {
  ensureLlmProfiles,
  normalizeLlmProfile,
  mcpMetadataForDaemon,
  mcpServerFromDaemonMetadata,
  providerProfileFromDaemonMetadata,
  providerProfileMetadataForDaemon,
  secretMetadataForProviderProfile,
  useConfigStore,
  type LlmProfile,
  type McpServerConfig,
} from './configStore'

beforeEach(() => {
  resetVolatileCredentialsForTests()
})

describe('config metadata sync contracts', () => {
  it('expires catalog evidence, discards old-endpoint replies, and does not persist freshness', async () => {
    vi.useFakeTimers()
    try {
      const profile = normalizeLlmProfile({ id: 'fresh-router', preset: 'openrouter', model: 'saved/model' })
      useConfigStore.setState({ llmProfiles: [profile], llmProfileModels: {}, llmProfileModelCatalogs: {} })
      useConfigStore.getState().setLlmProfileModels(profile.id, ['openrouter/free'], providerCatalogTarget(profile))
      expect(useConfigStore.getState().llmProfileModelCatalogs[profile.id]?.checkedAt).toBe(Date.now())
      await useConfigStore.persist.rehydrate()
      expect(useConfigStore.getState().llmProfileModelCatalogs).toEqual({})
      expect(useConfigStore.getState().llmProfiles.find((entry) => entry.id === profile.id)?.model).toBe('saved/model')
      useConfigStore.getState().setLlmProfileModels(profile.id, ['openrouter/free'], providerCatalogTarget(profile))
      vi.advanceTimersByTime(MODEL_CATALOG_MAX_AGE_MS + 1)
      expect(useConfigStore.getState().llmProfileModelCatalogs[profile.id]).toBeUndefined()
      useConfigStore.getState().updateLlmProfile(profile.id, { baseUrl: 'https://new.test/v1' })
      useConfigStore.getState().setLlmProfileModels(profile.id, ['obsolete'], providerCatalogTarget(profile))
      expect(useConfigStore.getState().llmProfileModels[profile.id]).toEqual([])
      expect(useConfigStore.getState().llmProfileModelCatalogs[profile.id]).toBeUndefined()
    } finally {
      vi.useRealTimers()
    }
  })
  it('defaults only empty OpenRouter profiles to free and preserves unavailable explicit choices', () => {
    expect(normalizeLlmProfile({ preset: 'openrouter', model: '' }).model).toBe('openrouter/free')
    const profile = normalizeLlmProfile({ id: 'stale-router', preset: 'openrouter', model: 'retired/model:free' })
    useConfigStore.setState({ llmProfiles: [profile], llmProfileModels: {} })
    useConfigStore.getState().setLlmProfileModels(profile.id, ['openrouter/free'])
    expect(useConfigStore.getState().llmProfiles[0].model).toBe('retired/model:free')
    useConfigStore.getState().updateLlmProfile(profile.id, { baseUrl: 'https://new.example.test/v1' })
    expect(useConfigStore.getState().llmProfileModels[profile.id]).toEqual([])
  })
  it('does not recreate a deleted OpenRouter seed when a persisted replacement exists', () => {
    const replacement: LlmProfile = {
      id: 'openrouter-personal',
      name: 'Personal OpenRouter',
      provider: 'openai-compatible',
      preset: 'openrouter',
      authMode: 'bearer',
      baseUrl: 'https://openrouter.ai/api/v1',
      model: 'openai/gpt-test',
      apiKey: '',
      hasApiKey: true,
      timeoutMs: 600_000,
      verifyTlsCertificates: true,
      contextWindow: 128_000,
      temperature: null,
    }

    const profiles = ensureLlmProfiles(undefined, [replacement])

    expect(profiles.filter((profile) => profile.preset === 'openrouter')).toEqual([replacement])
    expect(profiles.some((profile) => profile.id === 'default-openrouter')).toBe(false)
    expect(profiles.some((profile) => profile.id === 'default-ollama')).toBe(true)
    expect(profiles.some((profile) => profile.id === 'default-openai-compatible')).toBe(true)
  })

  it('keeps provider endpoints and API keys device-bound', () => {
    const profile: LlmProfile = {
      id: 'profile-local-vllm',
      name: 'Laptop vLLM',
      provider: 'openai-compatible',
      preset: 'custom',
      authMode: 'bearer',
      baseUrl: 'http://127.0.0.1:8000/v1',
      model: 'qwen3:14b',
      apiKey: 'super-secret',
      hasApiKey: true,
      timeoutMs: 600_000,
      verifyTlsCertificates: true,
      contextWindow: 32_768,
      temperature: null,
    }

    const metadata = providerProfileMetadataForDaemon(profile)
    const secretMetadata = secretMetadataForProviderProfile(profile)
    expect(JSON.stringify(metadata)).not.toContain('127.0.0.1')
    expect(JSON.stringify(metadata)).not.toContain('super-secret')
    expect(metadata).toMatchObject({ model: 'qwen3:14b', endpoint_binding: 'per_device' })
    expect(secretMetadata).toMatchObject({
      configured_on_source_device: true,
      value_included: false,
    })

    const restored = providerProfileFromDaemonMetadata(profile.id, metadata, profile)
    expect(restored?.baseUrl).toBe('http://127.0.0.1:8000/v1')
    expect(restored?.apiKey).toBe('')
    expect(restored?.hasApiKey).toBe(true)
  })

  it('clamps legacy provider timeouts to the daemon-supported 24-hour limit', () => {
    const current = {
      id: 'default-ollama',
      name: 'Ollama',
      provider: 'openai-compatible',
      preset: 'ollama',
      authMode: 'none',
      baseUrl: 'http://127.0.0.1:11434/v1',
      model: 'qwen3:latest',
      apiKey: '',
      timeoutMs: 200_000_000,
      verifyTlsCertificates: true,
      contextWindow: 128_000,
      temperature: 0.1,
    } satisfies LlmProfile

    const restored = providerProfileFromDaemonMetadata(current.id, {
      name: current.name,
      timeout_ms: 200_000_000,
    }, current)

    expect(restored?.timeoutMs).toBe(86_400_000)
    expect(providerProfileMetadataForDaemon(current).timeout_ms).toBe(86_400_000)
  })

  it('syncs MCP discovery metadata without local paths, arguments, or environment values', () => {
    const server: McpServerConfig = {
      id: 'mcp-private',
      name: 'Private MCP',
      command: 'C:/tools/private/mcp-server.exe',
      args: '--config C:/secret/mcp.json',
      cwd: 'C:/private/workspace',
      env: { ACCESS_TOKEN: 'token-value', REGION: 'eu' },
    }

    const metadata = mcpMetadataForDaemon(server)
    const serialized = JSON.stringify(metadata)
    expect(metadata).toMatchObject({
      name: 'Private MCP',
      executable_hint: 'mcp-server.exe',
      environment_keys: ['ACCESS_TOKEN', 'REGION'],
      device_binding_required: true,
    })
    expect(serialized).not.toContain('C:/')
    expect(serialized).not.toContain('token-value')
    expect(serialized).not.toContain('--config')

    const restored = mcpServerFromDaemonMetadata(server.id!, metadata, server)
    expect(restored).toEqual({ ...server, kind: 'stdio' })
  })

  it('migrates only the exact bundled websearch signature and preserves other commands', async () => {
    await useConfigStore.getState().importMcpServers([
      { name: 'bundled', command: 'node', args: 'scripts/mcp/duckduckgo-websearch-server.mjs', env: {} },
      { name: 'custom', command: 'node', args: 'scripts/custom.mjs', cwd: 'C:/Fixture', env: {} },
    ])
    expect(useConfigStore.getState().mcpServers.find((server) => server.name === 'bundled')).toMatchObject({ kind: 'builtin', builtinId: 'duckduckgo-websearch', args: '' })
    expect(useConfigStore.getState().mcpServers.find((server) => server.name === 'custom')).toMatchObject({ kind: 'stdio', command: 'node', args: 'scripts/custom.mjs', cwd: 'C:/Fixture' })
  })

  it('keeps the credential owner stable when an imported MCP server replaces the same name', async () => {
    const existing: McpServerConfig = {
      id: 'stable-mcp-id',
      name: 'filesystem',
      command: 'old-command',
      args: '.',
      env: { TOKEN: 'old-token', REMOVE_ME: 'old-value' },
    }
    useConfigStore.setState({
      mcpServer: existing,
      mcpServers: [existing],
      activeMcpServerName: existing.name,
    })
    await useConfigStore.getState().upsertMcpServer(existing)

    await useConfigStore.getState().importMcpServers([{
      name: 'filesystem',
      command: 'new-command',
      args: '--new',
      env: { TOKEN: 'new-token' },
    }])

    const imported = useConfigStore.getState().mcpServers[0]
    expect(imported).toMatchObject({ id: 'stable-mcp-id', name: 'filesystem', command: 'new-command' })
    expect(await getCredential({ scope: 'mcp_env', ownerId: 'stable-mcp-id', field: 'TOKEN' }))
      .toBe('new-token')
    expect(await getCredential({ scope: 'mcp_env', ownerId: 'stable-mcp-id', field: 'REMOVE_ME' }))
      .toBeNull()
  })
})
