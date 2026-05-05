import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { SessionRecord } from '../engine/services/sessionPersistence'

const invokeMock = vi.fn(async (_command?: string, _args?: unknown): Promise<unknown> => undefined)
const autoSaveSessionMock = vi.fn(async () => undefined)
const loadSessionMock = vi.fn(async (_sessionId?: string): Promise<SessionRecord | null> => null)
const buildSystemPromptWithMemoryMock = vi.fn(async (_cwd: string, systemPrompt: string) => ({
  systemPrompt,
  memoryContent: '',
}))
const queryCalls: Array<{ messages: unknown[]; userInput?: string }> = []
const queryBarriers: Array<Promise<void>> = []

function createQueryBarrier(): () => void {
  let resolveBarrier = () => {}
  const barrier = new Promise<void>((resolve) => {
    resolveBarrier = resolve
  })
  queryBarriers.push(barrier)
  return resolveBarrier
}

class FakeQueryEngine {
  updateConfig = vi.fn()
  setToolUICallback = vi.fn()
  abort = vi.fn()
  resolveApproval = vi.fn()

  constructor(_config: unknown) {}

  getAppState() {
    return {
      turnCount: 1,
      totalTokens: { input: 0, output: 0 },
      totalCostUsd: 0,
      planMode: false,
    }
  }

  getContextSnapshot() {
    return null
  }

  async *query(messages: unknown[], userInput?: string) {
    queryCalls.push({ messages, userInput })
    const barrier = queryBarriers.shift()
    if (barrier) {
      await barrier
    }
    yield {
      type: 'done' as const,
      messages: [
        ...messages as unknown[],
        {
          type: 'assistant',
          uuid: 'assistant-1',
          content: [{ type: 'text', text: 'ok' }],
          model: 'test-model',
          usage: { input_tokens: 0, output_tokens: 0 },
          stopReason: 'end_turn',
          timestamp: Date.now(),
        },
      ],
      totalUsage: { input_tokens: 0, output_tokens: 0 },
      totalCostUsd: 0,
    }
  }
}

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (command: string, args?: unknown) => invokeMock(command, args),
}))

vi.mock('./configStore', () => ({
  useConfigStore: {
    getState: () => ({
      ollama: {
        baseUrl: 'http://localhost:11434',
        model: 'gpt-oss:20b',
        temperature: 0,
        contextWindow: 32000,
        timeoutMs: 1000,
      },
      preferences: {
        verboseMode: false,
      },
    }),
  },
}))

vi.mock('../engine/core/queryEngine', () => ({
  QueryEngine: FakeQueryEngine,
}))

vi.mock('../engine/commands/registry', () => ({
  registerBuiltinCommands: vi.fn(),
  getAllCommands: vi.fn(() => []),
}))

vi.mock('../engine/api/ollamaClient', () => ({
  listOllamaModels: vi.fn(async () => []),
  checkOllamaConnection: vi.fn(async () => true),
}))

vi.mock('../engine/services/sessionPersistence', () => ({
  autoSaveSession: () => autoSaveSessionMock(),
  createSession: vi.fn(async () => undefined),
  generateSessionTitle: vi.fn(() => 'Seeded Session'),
  loadSession: (sessionId: string) => loadSessionMock(sessionId),
  listSessions: vi.fn(async () => []),
  deleteSession: vi.fn(async () => undefined),
}))

vi.mock('../engine/memory/memorySystem', () => ({
  buildSystemPromptWithMemory: (cwd: string, systemPrompt: string) => buildSystemPromptWithMemoryMock(cwd, systemPrompt),
}))

describe('engineStore history seeding', () => {
  beforeEach(async () => {
    queryCalls.length = 0
    queryBarriers.length = 0
    invokeMock.mockClear()
    autoSaveSessionMock.mockClear()
    loadSessionMock.mockClear()
    buildSystemPromptWithMemoryMock.mockClear()
    localStorage.clear()
    const { useEngineStore } = await import('./engineStore')
    useEngineStore.getState().clearMessages()
  })

  it('hydrates prior chat messages before the first Cowork engine turn', async () => {
    const { useEngineStore } = await import('./engineStore')

    await useEngineStore.getState().sendMessage(
      'alphabetisch',
      'C:/workspace',
      undefined,
      {
        threadId: 'thread-1',
        messages: [
          {
            role: 'user',
            content: 'sortiere alle ordner in 2 neue ordner',
            debugContent: 'sortiere alle ordner in 2 neue ordner\n\nVerbundene Pfade (1):\n1. Ordner: C:/workspace',
          },
          {
            role: 'assistant',
            content: 'Bitte geben Sie an, nach welchem Kriterium die Ordner sortiert werden sollen.',
          },
        ],
      },
    )

    expect(queryCalls).toHaveLength(1)
    expect(queryCalls[0]?.messages).toHaveLength(2)
    expect(queryCalls[0]?.userInput).toBe('alphabetisch')

    const firstSeededMessage = queryCalls[0]?.messages[0] as { type: string; content: Array<{ type: string; text: string }> }
    expect(firstSeededMessage.type).toBe('user')
    expect(firstSeededMessage.content[0]?.text).toContain('Verbundene Pfade (1)')
  })

  it('serializes concurrent sendMessage calls instead of rejecting', async () => {
    const { useEngineStore } = await import('./engineStore')
    const releaseFirst = createQueryBarrier()
    const releaseSecond = createQueryBarrier()

    const firstPromise = useEngineStore.getState().sendMessage('erste anfrage', 'C:/workspace')
    await vi.waitFor(() => {
      expect(queryCalls.map((call) => call.userInput)).toEqual(['erste anfrage'])
    })

    const secondPromise = useEngineStore.getState().sendMessage('zweite anfrage', 'C:/workspace')
    await Promise.resolve()

    expect(queryCalls.map((call) => call.userInput)).toEqual(['erste anfrage'])

    releaseFirst()
    await firstPromise
    await vi.waitFor(() => {
      expect(queryCalls.map((call) => call.userInput)).toEqual(['erste anfrage', 'zweite anfrage'])
    })

    releaseSecond()
    await expect(secondPromise).resolves.toBeUndefined()
  })

  it('continues a loaded persisted session without flattening tool messages', async () => {
    const { useEngineStore } = await import('./engineStore')

    // Mock session with engine-format messages (structured content)
    const structuredAssistantMessage = JSON.stringify({
      type: 'assistant',
      uuid: 'assistant-tool-1',
      content: [{ type: 'tool_use', id: 'tool-1', name: 'Read', input: { file_path: 'C:/workspace/a.txt' } }],
      model: 'gpt-oss:20b',
      usage: { input_tokens: 0, output_tokens: 0 },
      stopReason: 'tool_use',
      timestamp: 100,
    })

    loadSessionMock.mockResolvedValueOnce({
      id: 'session-1',
      title: 'Persistierte Analyse',
      cwd: 'C:/workspace',
      messages: [
        {
          type: 'assistant',
          uuid: 'assistant-tool-1',
          content: [{ type: 'tool_use', id: 'tool-1', name: 'Read', input: { file_path: 'C:/workspace/a.txt' } }],
          model: 'gpt-oss:20b',
          usage: { input_tokens: 0, output_tokens: 0 },
          stopReason: 'tool_use',
          timestamp: 100,
        },
      ],
      totalUsage: { input_tokens: 0, output_tokens: 0 },
      totalCostUsd: 0,
      appState: {},
      createdAt: 100,
      updatedAt: 200,
    })

    await useEngineStore.getState().loadSessionById('session-1')

    // Send message with historySeed containing the structured messages from the session
    await useEngineStore.getState().sendMessage(
      'und jetzt weiter',
      'C:/workspace',
      undefined,
      {
        threadId: 'session-1',
        messages: [
          {
            role: 'assistant',
            content: 'Tool-Aufruf: Read {"file_path":"C:/workspace/a.txt"}',
            debugContent: structuredAssistantMessage,
          },
        ],
      },
    )

    expect(queryCalls).toHaveLength(1)
    const firstLoadedMessage = queryCalls[0]?.messages[0] as {
      type: string
      content: Array<{ type: string; name?: string }>
    }
    expect(firstLoadedMessage.type).toBe('assistant')
    expect(firstLoadedMessage.content[0]?.type).toBe('tool_use')
    expect(firstLoadedMessage.content[0]?.name).toBe('Read')
  })

  it('reconstructs structured history from persisted debug content', async () => {
    const { useEngineStore } = await import('./engineStore')

    const assistantStructuredMessage = JSON.stringify({
      type: 'assistant',
      uuid: 'assistant-tool-2',
      content: [{ type: 'tool_use', id: 'tool-2', name: 'ListDir', input: { path: 'C:/workspace' } }],
      model: 'gpt-oss:20b',
      usage: { input_tokens: 0, output_tokens: 0 },
      stopReason: 'tool_use',
      timestamp: 101,
    })

    await useEngineStore.getState().sendMessage(
      'weitermachen',
      'C:/workspace',
      undefined,
      {
        threadId: 'thread-json',
        messages: [
          {
            role: 'assistant',
            content: 'Tool-Aufruf: ListDir {"path":"C:/workspace"}',
            debugContent: assistantStructuredMessage,
          },
        ],
      },
    )

    expect(queryCalls).toHaveLength(1)
    const firstSeededMessage = queryCalls[0]?.messages[0] as {
      type: string
      content: Array<{ type: string; name?: string }>
    }
    expect(firstSeededMessage.type).toBe('assistant')
    expect(firstSeededMessage.content[0]?.type).toBe('tool_use')
    expect(firstSeededMessage.content[0]?.name).toBe('ListDir')
  })
})
