import { beforeEach, describe, expect, it, vi } from 'vitest'

const invokeMock = vi.fn(async (_command?: string, _args?: unknown): Promise<unknown> => undefined)
const buildSystemPromptWithMemoryMock = vi.fn(async (_cwd: string, systemPrompt: string, _options?: unknown) => ({
  systemPrompt,
  memoryContent: '',
}))
const loadFrozenMemorySnapshotMock = vi.fn(async (_threadId?: string) => null)
const captureAutomaticMemoryDraftMock = vi.fn(async (_cwd: string, _input: string, _runId?: string) => [])
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
        model: 'llama3.1:8b',
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

vi.mock('../engine/memory/memorySystem', () => ({
  buildSystemPromptWithMemory: (cwd: string, systemPrompt: string, options?: unknown) => buildSystemPromptWithMemoryMock(cwd, systemPrompt, options),
  loadFrozenMemorySnapshot: (threadId: string) => loadFrozenMemorySnapshotMock(threadId),
  captureAutomaticMemoryDraft: (cwd: string, input: string, runId: string) => captureAutomaticMemoryDraftMock(cwd, input, runId),
}))

describe('engineStore history seeding', () => {
  beforeEach(async () => {
    queryCalls.length = 0
    queryBarriers.length = 0
    invokeMock.mockClear()
    buildSystemPromptWithMemoryMock.mockClear()
    loadFrozenMemorySnapshotMock.mockClear()
    captureAutomaticMemoryDraftMock.mockClear()
    localStorage.clear()
    const { useEngineStore } = await import('./engineStore')
    useEngineStore.getState().clearMessages()
  })

  it('keeps legacy API keys out of frontend state', async () => {
    const { useEngineStore } = await import('./engineStore')

    await useEngineStore.getState().setApiKey('sk-must-remain-in-vault')

    expect(useEngineStore.getState().config.apiKey).toBe('')
    expect(localStorage.getItem('engine-store')).not.toContain('sk-must-remain-in-vault')
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
            content: 'sort all folders into 2 new folders',
            debugContent: 'sort all folders into 2 new folders\n\nConnected paths (1):\n1. Folder: C:/workspace',
          },
          {
            role: 'assistant',
            content: 'Please specify which criterion should be used to sort the folders.',
          },
        ],
      },
    )

    expect(queryCalls).toHaveLength(1)
    expect(queryCalls[0]?.messages).toHaveLength(2)
    expect(queryCalls[0]?.userInput).toBe('alphabetisch')
    expect(loadFrozenMemorySnapshotMock).toHaveBeenCalledWith(expect.any(String))
    // Browser/no-native-sandbox mode must not invoke unscoped filesystem memory helpers.
    expect(captureAutomaticMemoryDraftMock).not.toHaveBeenCalled()
    expect(buildSystemPromptWithMemoryMock).not.toHaveBeenCalled()
    expect(useEngineStore.getState().sandboxContext.mode).toBe('host_read_only_broker')

    const firstSeededMessage = queryCalls[0]?.messages[0] as { type: string; content: Array<{ type: string; text: string }> }
    expect(firstSeededMessage.type).toBe('user')
    expect(firstSeededMessage.content[0]?.text).toContain('Connected paths (1)')
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

  it('continues persisted chat history without flattening tool messages', async () => {
    const { useEngineStore } = await import('./engineStore')

    // Persisted chat messages retain the structured engine payload in debug content.
    const structuredAssistantMessage = JSON.stringify({
      type: 'assistant',
      uuid: 'assistant-tool-1',
      content: [{ type: 'tool_use', id: 'tool-1', name: 'Read', input: { file_path: 'C:/workspace/a.txt' } }],
      model: 'llama3.1:8b',
      usage: { input_tokens: 0, output_tokens: 0 },
      stopReason: 'tool_use',
      timestamp: 100,
    })

    // Send with the structured messages loaded from the chat database.
    await useEngineStore.getState().sendMessage(
      'und jetzt weiter',
      'C:/workspace',
      undefined,
      {
        threadId: 'thread-1',
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
      model: 'llama3.1:8b',
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

  it('rebuilds the same persisted chat history after switching providers', async () => {
    const { useEngineStore } = await import('./engineStore')
    const historySeed = {
      threadId: 'thread-provider-switch',
      messages: [
        { role: 'user' as const, content: 'Remember this decision.' },
        { role: 'assistant' as const, content: 'The decision is SQLite.' },
      ],
    }

    await useEngineStore.getState().sendMessage(
      'continue locally',
      'C:/workspace',
      undefined,
      historySeed,
      { backend: 'openai-compatible', profileId: 'default-ollama', model: 'llama3.1:8b' },
    )
    await useEngineStore.getState().sendMessage(
      'continue externally',
      'C:/workspace',
      undefined,
      historySeed,
      { backend: 'openai-compatible', profileId: 'default-openrouter', model: 'openai/gpt-test' },
    )

    expect(queryCalls).toHaveLength(2)
    expect(queryCalls[0]?.messages).toHaveLength(2)
    expect(queryCalls[1]?.messages).toHaveLength(2)
    const providerNeutralContent = (messages: unknown[]) => messages.map((message) => {
      const typed = message as { type: string; content: Array<{ type: string; text?: string }> }
      return [typed.type, typed.content.map((block) => block.text ?? block.type)]
    })
    expect(providerNeutralContent(queryCalls[1]?.messages ?? []))
      .toEqual(providerNeutralContent(queryCalls[0]?.messages ?? []))
  })

  it('keeps persisted attachment context but excludes provider debug previews', async () => {
    const { useEngineStore } = await import('./engineStore')

    await useEngineStore.getState().sendMessage(
      'continue',
      'C:/workspace',
      undefined,
      {
        threadId: 'thread-attachment-context',
        messages: [{
          role: 'user',
          content: 'Review the file.',
          debugContent: 'Review the file.\n\nAttached file: report.txt\nimportant contents\n\n[OLLAMA REQUEST PREVIEW]\ninternal request payload',
        }],
      },
    )

    const seeded = queryCalls[0]?.messages[0] as {
      content: Array<{ type: string; text?: string }>
    }
    const seededText = seeded.content.map((block) => block.text ?? '').join('\n')
    expect(seededText).toContain('Attached file: report.txt')
    expect(seededText).toContain('important contents')
    expect(seededText).not.toContain('OLLAMA REQUEST PREVIEW')
    expect(seededText).not.toContain('internal request payload')
  })
})
