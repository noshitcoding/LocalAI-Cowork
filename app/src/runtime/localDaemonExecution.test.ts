import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { RunRecord } from './contracts'
import type { LocalDaemonRuntimeClient } from './localDaemonClient'
import {
  createDurableCodexRun,
  createDurableCrewRun,
  createDurableLocalRun,
  localRuntimeEntityUuid,
  upsertDurableCrewSchedule,
  upsertDurableLocalSchedule,
} from './localDaemonExecution'

const getCredential = vi.fn(async (_locator: unknown) => 'vault-secret')

vi.mock('../security/credentialVault', () => ({
  getCredential: (locator: unknown) => getCredential(locator),
  llmApiKeyLocator: (profileId: string) => ({
    scope: 'llm_profile', ownerId: profileId, field: 'api_key',
  }),
  crewProviderLocator: (crewId: string, provider: string) => ({
    scope: 'crew', ownerId: crewId, field: `${provider}_api_key`,
  }),
}))

describe('durable local execution', () => {
  beforeEach(() => {
    window.localStorage.clear()
    getCredential.mockClear()
  })

  it('keeps stable UUID bindings for legacy desktop entity IDs', () => {
    const first = localRuntimeEntityUuid('thread', 'legacy-thread-1')
    const second = localRuntimeEntityUuid('thread', 'legacy-thread-1')
    const project = localRuntimeEntityUuid('project', 'legacy-thread-1')
    expect(first).toMatch(/^[0-9a-f-]{36}$/)
    expect(second).toBe(first)
    expect(project).not.toBe(first)
  })

  it('binds the workspace and submits the selected model secret outside run input', async () => {
    const bindProjectWorkspace = vi.fn(async () => undefined)
    const upsertProviderBinding = vi.fn(async () => ({ profile_id: 'profile-vllm', bound: true }))
    const upsertMcpBinding = vi.fn(async () => ({ server_id: 'docs', bound: true }))
    const createConfiguredRun = vi.fn(async (request: unknown, _modelConfig: unknown) => ({
      spec: { id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa' },
      state: 'queued',
      request,
    }) as unknown as RunRecord)
    const client = {
      health: vi.fn(async () => ({
        status: 'ok', schema_version: 2,
        device_id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
        daemon_version: '0.3.0',
      })),
      bindProjectWorkspace,
      upsertProviderBinding,
      upsertMcpBinding,
      createConfiguredRun,
    } as unknown as LocalDaemonRuntimeClient

    await createDurableLocalRun({
      clientThreadId: 'legacy-thread',
      clientProjectId: 'legacy-project',
      assistantMessageId: 'assistant-1',
      prompt: 'finish the report',
      workspacePath: 'C:\\work\\report',
      mcpServers: [{
        name: 'docs',
        command: 'docs-mcp',
        args: '--stdio "C:\\work\\report"',
        env: { MCP_TOKEN: 'mcp-secret' },
      }],
      provider: {
        provider: 'openai-compatible',
        backend: 'openai-compatible',
        label: 'Local vLLM',
        endpoint: 'http://127.0.0.1:8000/v1',
        model: 'local-model',
        apiKey: '',
        timeoutMs: 120_000,
        verifyTlsCertificates: true,
        contextWindow: 32_000,
        selectableModels: ['local-model'],
        profileId: 'profile-vllm',
        preset: 'custom',
      },
      source: 'chat',
    }, client)

    expect(bindProjectWorkspace).toHaveBeenCalledWith(expect.any(String), 'C:\\work\\report')
    expect(upsertProviderBinding).toHaveBeenCalledWith(
      'profile-vllm',
      'http://127.0.0.1:8000/v1',
      'vault-secret',
    )
    expect(getCredential).toHaveBeenCalledWith({
      scope: 'llm_profile', ownerId: 'profile-vllm', field: 'api_key',
    })
    expect(upsertMcpBinding).toHaveBeenCalledWith('docs', {
      name: 'docs',
      command: 'docs-mcp',
      args: ['--stdio', 'C:\\work\\report'],
      env: { MCP_TOKEN: 'mcp-secret' },
    })
    const [request, modelConfig] = createConfiguredRun.mock.calls[0]
    const runRequest = request as { input: unknown; required_capabilities: string[] }
    expect(runRequest.input).not.toHaveProperty('api_key')
    expect(JSON.stringify(runRequest.input)).not.toContain('mcp-secret')
    expect(runRequest.required_capabilities).toEqual(expect.arrayContaining([
      'model.vllm', 'files', 'shell', 'tool.mcp.invoke',
    ]))
    expect(modelConfig).toMatchObject({
      base_url: 'http://127.0.0.1:8000/v1',
      api_key: 'vault-secret',
      model: 'local-model',
      mcp_servers: [{
        name: 'docs',
        command: 'docs-mcp',
        args: ['--stdio', 'C:\\work\\report'],
        env: { MCP_TOKEN: 'mcp-secret' },
      }],
    })
  })

  it('resolves scheduled MCP servers from current encrypted device bindings', async () => {
    const upsertMcpBinding = vi.fn(async () => ({ server_id: 'mcp-docs', bound: true }))
    const upsertSchedule = vi.fn(async (request: unknown) => ({
      id: 'schedule-id',
      expression: 'every 1h',
      timezone: 'Europe/Berlin',
      enabled: true,
      next_run_at: '2026-08-09T20:00:00Z',
      last_triggered_at: null,
      last_error: null,
      created_at: '2026-08-09T19:00:00Z',
      updated_at: '2026-08-09T19:00:00Z',
      request,
    }))
    const client = {
      health: vi.fn(async () => ({
        status: 'ok', schema_version: 2,
        device_id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
        daemon_version: '0.3.0',
      })),
      bindProjectWorkspace: vi.fn(async () => undefined),
      upsertProviderBinding: vi.fn(async () => ({ profile_id: 'profile-local', bound: true })),
      upsertMcpBinding,
      upsertSchedule,
    } as unknown as LocalDaemonRuntimeClient

    await upsertDurableLocalSchedule({
      scheduleClientId: 'task-schedule',
      expression: 'every 1h',
      timezone: 'Europe/Berlin',
      enabled: true,
      clientThreadId: 'thread-schedule',
      clientProjectId: 'project-schedule',
      clientTaskId: 'task-schedule',
      prompt: 'Use current MCP configuration',
      provider: {
        provider: 'openai-compatible',
        backend: 'openai-compatible',
        label: 'Local API',
        endpoint: 'http://127.0.0.1:8000/v1',
        model: 'local-model',
        apiKey: '',
        timeoutMs: 120_000,
        verifyTlsCertificates: true,
        contextWindow: 32_000,
        selectableModels: ['local-model'],
        profileId: 'profile-local',
        preset: 'custom',
      },
      mcpServers: [{
        id: 'mcp-docs',
        name: 'docs',
        command: 'docs-mcp',
        args: '--stdio',
        env: { MCP_TOKEN: 'current-secret' },
      }],
    }, client)

    expect(upsertMcpBinding).toHaveBeenCalledWith('mcp-docs', {
      name: 'docs',
      command: 'docs-mcp',
      args: ['--stdio'],
      env: { MCP_TOKEN: 'current-secret' },
    })
    expect(upsertSchedule).toHaveBeenCalledWith(expect.objectContaining({
      run_request: expect.objectContaining({
        input: expect.objectContaining({
          client_mcp_server_ids: ['mcp-docs'],
          resolve_current_mcp_bindings: true,
        }),
      }),
      model_config: expect.objectContaining({
        mcp_servers: [expect.objectContaining({ command: 'docs-mcp' })],
      }),
    }))
  })

  it('keeps the complete Crew request and hydrated provider secret outside public run input', async () => {
    const createConfiguredRun = vi.fn(async (request: unknown, _modelConfig: unknown) => ({
      spec: { id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa' },
      state: 'queued',
      request,
    }) as unknown as RunRecord)
    const client = {
      health: vi.fn(async () => ({
        status: 'ok', schema_version: 2,
        device_id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
        daemon_version: '0.3.0',
      })),
      bindProjectWorkspace: vi.fn(async () => undefined),
      createConfiguredRun,
    } as unknown as LocalDaemonRuntimeClient

    await createDurableCrewRun({
      clientThreadId: 'thread-crew',
      clientProjectId: 'project-crew',
      clientTaskId: 'task-crew',
      assistantMessageId: 'assistant-crew',
      crewLiveMessageId: 'monitor-crew',
      crewLiveTitle: 'Research crew',
      prompt: 'Research the topic',
      workspacePath: 'C:\\work\\crew',
      crewId: 'crew-1',
      source: 'task',
      crewRequest: {
        id: 'crew-1',
        streamId: 'stream-1',
        config: { baseUrl: 'http://127.0.0.1:11434', model: 'qwen3', timeoutMs: 60_000 },
        providerConfigs: {
          openAICompatible: {
            profileId: 'profile-openai',
            baseUrl: 'https://api.openai.com/v1',
            model: 'gpt-test',
            apiKey: '',
            timeoutMs: 120_000,
          },
        },
        agents: [{ id: 'agent-1' }],
        tasks: [{ id: 'task-1', description: 'secret mission details' }],
      },
    }, client)

    const [request, modelConfig] = createConfiguredRun.mock.calls[0]
    expect(JSON.stringify((request as { input: unknown }).input)).not.toContain('secret mission details')
    expect(JSON.stringify((request as { input: unknown }).input)).not.toContain('vault-secret')
    expect(request).toMatchObject({
      required_capabilities: ['crew.python', 'files', 'shell'],
      input: {
        client_crew_live_message_id: 'monitor-crew',
        crew_stream_id: 'stream-1',
        source: 'crew_task',
      },
    })
    expect(modelConfig).toMatchObject({
      crew_request: {
        providerConfigs: {
          openAICompatible: { apiKey: 'vault-secret' },
        },
        tasks: [{ description: 'secret mission details' }],
      },
    })
  })

  it('binds Crew schedule provider profiles for trigger-time credential resolution', async () => {
    const upsertProviderBinding = vi.fn(async () => ({ profile_id: 'profile-openai', bound: true }))
    const upsertSchedule = vi.fn(async () => ({
      id: 'crew-schedule',
      expression: 'daily 09:00',
      timezone: 'Europe/Berlin',
      enabled: true,
      next_run_at: '2026-08-10T07:00:00Z',
      last_triggered_at: null,
      last_error: null,
      created_at: '2026-08-09T19:00:00Z',
      updated_at: '2026-08-09T19:00:00Z',
    }))
    const client = {
      health: vi.fn(async () => ({
        status: 'ok', schema_version: 2,
        device_id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
        daemon_version: '0.3.0',
      })),
      bindProjectWorkspace: vi.fn(async () => undefined),
      upsertProviderBinding,
      upsertSchedule,
    } as unknown as LocalDaemonRuntimeClient

    await upsertDurableCrewSchedule({
      scheduleClientId: 'task-crew-schedule',
      expression: 'daily 09:00',
      timezone: 'Europe/Berlin',
      enabled: true,
      clientThreadId: 'thread-crew-schedule',
      clientProjectId: 'project-crew-schedule',
      clientTaskId: 'task-crew-schedule',
      crewLiveTitle: 'Scheduled crew',
      prompt: 'Run with current provider settings',
      workspacePath: 'C:\\work\\crew',
      crewId: 'crew-1',
      crewRequest: {
        id: 'crew-1',
        config: { baseUrl: 'http://127.0.0.1:11434', model: 'qwen3', timeoutMs: 60_000 },
        providerConfigs: {
          openAICompatible: {
            profileId: 'profile-openai',
            baseUrl: 'https://api.openai.com/v1',
            model: 'gpt-test',
            apiKey: '',
            timeoutMs: 120_000,
          },
        },
        agents: [],
        tasks: [],
      },
    }, client)

    expect(upsertProviderBinding).toHaveBeenCalledWith(
      'profile-openai',
      'https://api.openai.com/v1',
      'vault-secret',
    )
    expect(upsertSchedule).toHaveBeenCalledWith(expect.objectContaining({
      run_request: expect.objectContaining({
        input: expect.objectContaining({
          resolve_current_crew_provider_bindings: true,
          client_crew_provider_profile_ids: ['profile-openai'],
        }),
      }),
      model_config: expect.objectContaining({
        crew_request: expect.objectContaining({
          providerConfigs: expect.objectContaining({
            openAICompatible: expect.objectContaining({ apiKey: 'vault-secret' }),
          }),
        }),
      }),
    }))
  })

  it('routes Codex through the personal daemon without putting conversation history in public run input', async () => {
    const createConfiguredRun = vi.fn(async (request: unknown, _modelConfig: unknown) => ({
      spec: { id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa' },
      state: 'queued',
      request,
    }) as unknown as RunRecord)
    const client = {
      health: vi.fn(async () => ({
        status: 'ok', schema_version: 2,
        device_id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
        daemon_version: '0.3.0',
      })),
      bindProjectWorkspace: vi.fn(async () => undefined),
      createConfiguredRun,
    } as unknown as LocalDaemonRuntimeClient

    await createDurableCodexRun({
      clientThreadId: 'thread-codex',
      clientProjectId: 'project-codex',
      assistantMessageId: 'assistant-codex',
      userMessageId: 'user-codex',
      prompt: 'Current request',
      history: [{ role: 'assistant', content: 'private prior answer' }],
      workspacePath: 'C:\\work\\codex',
      profileId: 'codex-account-1',
      model: 'gpt-5.3-codex',
      reasoningEffort: 'high',
      source: 'chat',
    }, client)

    const [request, modelConfig] = createConfiguredRun.mock.calls[0]
    expect(JSON.stringify((request as { input: unknown }).input)).not.toContain('private prior answer')
    expect(request).toMatchObject({
      required_capabilities: ['model.codex', 'files', 'shell'],
      input: { codex_profile_id: 'codex-account-1' },
    })
    expect(modelConfig).toMatchObject({
      codex_request: {
        profile_id: 'codex-account-1',
        model: 'gpt-5.3-codex',
        reasoning_effort: 'high',
      },
    })
    expect(JSON.stringify(modelConfig)).toContain('private prior answer')
  })
})
