import { beforeEach, describe, expect, it, vi } from 'vitest'
import { usePipelineStore } from './pipelineStore'
import { safeInvoke } from '../utils/safeInvoke'

vi.mock('../utils/safeInvoke', () => ({
  hasTauriRuntime: vi.fn(() => false),
  safeInvoke: vi.fn(),
}))

const safeInvokeMock = vi.mocked(safeInvoke)

function createFetchResponse(overrides: {
  ok: boolean
  status?: number
  responseText?: string
} = { ok: true }): Response {
  return {
    ok: overrides.ok,
    status: overrides.status ?? (overrides.ok ? 200 : 500),
    json: async () => ({ response: overrides.responseText ?? '' }),
  } as Response
}

describe('pipelineStore', () => {
  beforeEach(() => {
    vi.restoreAllMocks()
    safeInvokeMock.mockReset()
    safeInvokeMock.mockImplementation(async (_command, _args, fallback) => fallback ?? null)
    localStorage.clear()
    usePipelineStore.setState({
      pipelines: [],
      toolGateway: [],
      loading: false,
      error: null,
      executing: null,
      lastResult: null,
    })
  })

  it('reads local cache when backend invocation is unavailable', async () => {
    localStorage.setItem(
      'open-cowork-pipelines',
      JSON.stringify([
        {
          id: 'p-local',
          name: 'Local',
          description: null,
          steps_json: '[{"tool":"ollama","prompt":"Hallo"}]',
          zero_context: false,
          created_at: '2026-07-03T00:00:00.000Z',
          updated_at: '2026-07-03T00:00:00.000Z',
        },
      ]),
    )

    await usePipelineStore.getState().loadPipelines()

    expect(usePipelineStore.getState().pipelines).toHaveLength(1)
    expect(usePipelineStore.getState().pipelines[0]).toMatchObject({ id: 'p-local', name: 'Local' })
  })

  it('keeps browser-only gateway configuration volatile and removes legacy plaintext', async () => {
    const sentinel = 'gateway-config-must-not-persist'
    localStorage.setItem('open-cowork-gateway', JSON.stringify([{
      id: 'legacy-gateway',
      tool_type: 'custom',
      name: 'Legacy gateway',
      config_json: JSON.stringify({ arbitrary: sentinel }),
      enabled: true,
      created_at: '2026-07-10T00:00:00.000Z',
      updated_at: '2026-07-10T00:00:00.000Z',
    }]))

    await usePipelineStore.getState().loadToolGateway()

    expect(usePipelineStore.getState().toolGateway).toHaveLength(1)
    expect(usePipelineStore.getState().toolGateway[0].config_json).toContain(sentinel)
    expect(localStorage.getItem('open-cowork-gateway')).toBeNull()

    safeInvokeMock.mockRejectedValueOnce(new Error('desktop runtime unavailable'))
    await usePipelineStore.getState().upsertToolGateway({
      id: 'volatile-gateway',
      toolType: 'custom',
      name: 'Volatile gateway',
      configJson: JSON.stringify({ another: sentinel }),
      enabled: true,
    })
    expect(usePipelineStore.getState().toolGateway.some((entry) => entry.id === 'volatile-gateway')).toBe(true)
    expect(JSON.stringify(localStorage)).not.toContain(sentinel)
  })

  it('falls back to local execution when backend execute fails', async () => {
    safeInvokeMock.mockImplementation(async (cmd, _args, fallback) => {
      if (cmd === 'pipeline_execute') {
        throw new Error('Backend not available')
      }
      return fallback ?? null
    })

    vi.spyOn(globalThis, 'fetch').mockResolvedValueOnce(
      createFetchResponse({ ok: true, responseText: 'step 1 done' }),
    )
    vi.spyOn(globalThis, 'fetch').mockResolvedValueOnce(
      createFetchResponse({ ok: true, responseText: 'step 2 done' }),
    )

    usePipelineStore.setState({
      pipelines: [
        {
          id: 'p-fallback',
          name: 'Fallback',
          description: null,
          steps_json: '[{"tool":"ollama","prompt":"ein"},{"args":"zwei"}]',
          zero_context: false,
          created_at: '2026-07-03T00:00:00.000Z',
          updated_at: '2026-07-03T00:00:00.000Z',
        },
      ],
    })

    const result = await usePipelineStore.getState().executePipeline('p-fallback', 'http://localhost:11434', 'test-model')

    expect(result.status).toBe('completed')
    expect(result.stepResults).toHaveLength(2)
    expect(result.stepResults[0]).toMatchObject({ step: 1, success: true, tool: 'ollama' })
    expect(result.stepResults[1]).toMatchObject({ step: 2, success: true, tool: 'ollama' })
  })

  it('fails fast when local pipeline steps are invalid JSON', async () => {
    safeInvokeMock.mockImplementation(async (cmd, _args, fallback) => {
      if (cmd === 'pipeline_execute') {
        throw new Error('Backend not available')
      }
      return fallback ?? null
    })

    usePipelineStore.setState({
      pipelines: [
        {
          id: 'p-bad-json',
          name: 'Bad JSON',
          description: null,
          steps_json: '{"tool":"ollama"}',
          zero_context: false,
          created_at: '2026-07-03T00:00:00.000Z',
          updated_at: '2026-07-03T00:00:00.000Z',
        },
      ],
    })

    const result = await usePipelineStore.getState().executePipeline('p-bad-json', 'http://localhost:11434', 'test-model')

    expect(result.status).toBe('failed')
    expect(result.error).toContain('Ungueltiges Steps-JSON')
    expect(result.stepResults).toHaveLength(0)
  })

  it('marks failing local execution steps when Ollama returns an error response', async () => {
    safeInvokeMock.mockImplementation(async (cmd, _args, fallback) => {
      if (cmd === 'pipeline_execute') {
        throw new Error('Backend not available')
      }
      return fallback ?? null
    })

    vi.spyOn(globalThis, 'fetch').mockResolvedValueOnce(createFetchResponse({ ok: false, status: 503 }))

    usePipelineStore.setState({
      pipelines: [
        {
          id: 'p-http-fail',
          name: 'HTTP fail',
          description: null,
          steps_json: '[{"tool":"ollama","prompt":"ein"}]',
          zero_context: false,
          created_at: '2026-07-03T00:00:00.000Z',
          updated_at: '2026-07-03T00:00:00.000Z',
        },
      ],
    })

    const result = await usePipelineStore.getState().executePipeline('p-http-fail', 'http://localhost:11434', 'test-model')

    expect(result.status).toBe('failed')
    expect(result.stepResults).toHaveLength(1)
    expect(result.stepResults[0]).toMatchObject({ step: 1, success: false, result: 'HTTP 503' })
  })

  it('executes via backend when pipeline_execute returns a completed result', async () => {
    safeInvokeMock.mockImplementation(async (cmd, _args, fallback) => {
      if (cmd === 'pipeline_execute') {
        return {
          pipelineId: 'p-backend',
          status: 'completed',
          stepResults: [
            { step: 1, tool: 'ollama', result: 'backend ok', success: true },
          ],
          error: null,
        }
      }
      return fallback ?? null
    })

    usePipelineStore.setState({
      pipelines: [
        {
          id: 'p-backend',
          name: 'Backend',
          description: null,
          steps_json: '[{"tool":"ollama","prompt":"backend"}]',
          zero_context: false,
          created_at: '2026-07-03T00:00:00.000Z',
          updated_at: '2026-07-03T00:00:00.000Z',
        },
      ],
    })

    const result = await usePipelineStore.getState().executePipeline('p-backend', 'http://localhost:11434', 'test-model')

    expect(result.status).toBe('completed')
    expect(result.pipelineId).toBe('p-backend')
    expect(result.stepResults).toHaveLength(1)
    expect(result.stepResults[0]).toMatchObject({ result: 'backend ok', success: true })
  })
})
