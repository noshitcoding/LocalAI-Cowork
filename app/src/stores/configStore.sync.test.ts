import { describe, expect, it } from 'vitest'
import {
  mcpMetadataForDaemon,
  mcpServerFromDaemonMetadata,
  providerProfileFromDaemonMetadata,
  providerProfileMetadataForDaemon,
  secretMetadataForProviderProfile,
  type LlmProfile,
  type McpServerConfig,
} from './configStore'

describe('config metadata sync contracts', () => {
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

  it('syncs MCP discovery metadata without local paths, arguments, or environment values', () => {
    const server: McpServerConfig = {
      id: 'mcp-private',
      name: 'Private MCP',
      command: 'C:/tools/private/mcp-server.exe',
      args: '--config C:/secret/mcp.json',
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
    expect(restored).toEqual(server)
  })
})
