import { beforeEach, describe, expect, it, vi } from 'vitest'

const readToolDef = {
  name: 'Read',
  description: 'Liest eine File.',
  input_schema: {
    type: 'object' as const,
    properties: {
      file_path: { type: 'string', description: 'Pfad zur File' },
    },
    required: ['file_path'] as string[],
  },
}

const invokeMock = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}))

describe('sampleOllamaMessage', () => {
  beforeEach(() => {
    invokeMock.mockReset()
    vi.restoreAllMocks()
    vi.unstubAllGlobals()
    Object.defineProperty(window, '__TAURI_INTERNALS__', {
      value: { invoke: vi.fn() },
      configurable: true,
      writable: true,
    })
  })

  describe('detectModelCapabilities', () => {
    it('treats gemma models as tool-capable when using Ollama chat tools', async () => {
      const { detectModelCapabilities } = await import('./ollamaClient')

      expect(detectModelCapabilities('gemma4:31b')).toMatchObject({
        family: 'gemma',
        supportsTools: true,
      })
    })
  })

  it('omits the thinking flag for known models that do not support it', async () => {
    const { buildOllamaChatRequest } = await import('./ollamaClient')

    const request = buildOllamaChatRequest(
      {
        baseUrl: 'http://localhost:11434',
        model: 'llama3.1:8b',
        timeoutMs: 200000,
        thinkingEnabled: true,
      },
      [{ role: 'user', content: 'Hello' }],
      'System prompt',
    )

    expect(request.body).not.toHaveProperty('think')
    expect(JSON.parse(request.debugPreview).think).toBeUndefined()
  })

  it('keeps thinking enabled for a compatible model family', async () => {
    const { buildOllamaChatRequest } = await import('./ollamaClient')

    const request = buildOllamaChatRequest(
      {
        baseUrl: 'http://localhost:11434',
        model: 'qwen3.6:35b',
        timeoutMs: 200000,
        thinkingEnabled: true,
      },
      [{ role: 'user', content: 'Hello' }],
      'System prompt',
    )

    expect(request.body).toHaveProperty('think', true)
  })

  it('forwards image attachments alongside tool results to Ollama chat requests', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        '{"model":"qwen3.6:35b","message":{"role":"assistant","content":"Analyse abclosed."},"done":true,"done_reason":"stop"}\n',
        {
          status: 200,
          headers: { 'Content-Type': 'application/x-ndjson' },
        },
      ),
    )
    vi.stubGlobal('fetch', fetchMock)

    const { sampleOllamaMessage } = await import('./ollamaClient')
    await sampleOllamaMessage(
      {
        baseUrl: 'http://localhost:11434',
        model: 'qwen3.6:35b',
        timeoutMs: 200000,
      },
      [{
        role: 'user',
        content: [
          { type: 'tool_result', tool_use_id: 'tool-1', content: 'screenshot captured.' },
          { type: 'text', text: 'Please analyze the current screen.' },
          { type: 'image', source: { type: 'base64', media_type: 'image/png', data: 'AAA' } },
        ],
      }],
      'Systemprompt',
    )

    const body = JSON.parse(String(fetchMock.mock.calls[0][1].body))
    expect(body.messages).toEqual([
      { role: 'system', content: 'Systemprompt' },
      { role: 'tool', content: 'screenshot captured.' },
      { role: 'user', content: 'Please analyze the current screen.', images: ['AAA'] },
    ])
  })

  it('builds a readable debug preview for the exact Ollama request context', async () => {
    const { buildOllamaChatRequest } = await import('./ollamaClient')

    const request = buildOllamaChatRequest(
      {
        baseUrl: 'http://localhost:11434',
        model: 'qwen3.6:35b',
        timeoutMs: 200000,
        temperature: 0.2,
        contextWindow: 65536,
      },
      [{
        role: 'user',
        content: [
          { type: 'tool_result', tool_use_id: 'tool-1', content: 'Desktop screenshot captured: 1280x720 auf \\\\.\\DISPLAY1.' },
          { type: 'text', text: 'Please nutze lokale Display coordinates.' },
          { type: 'image', source: { type: 'base64', media_type: 'image/png', data: 'AAAA' } },
        ],
      }],
      'Systemprompt for Desktopsteuerung',
      [readToolDef],
    )

    expect(request.body).toMatchObject({
      model: 'qwen3.6:35b',
      stream: true,
      options: {
        temperature: 0.2,
        num_ctx: 65536,
      },
    })
    const preview = JSON.parse(request.debugPreview)
    expect(preview.messageCount).toBe(3)
    expect(preview.messages[0].content).toBe('Systemprompt for Desktopsteuerung')
    expect(preview.messages[1].content).toContain('Desktop screenshot captured: 1280x720 auf')
    expect(preview.messages[1].content).toContain('DISPLAY1')
    expect(preview.messages[2].content).toBe('Please nutze lokale Display coordinates.')
    expect(preview.messages[2].images).toEqual(['[base64 image 1, 4 chars]'])
    expect(preview.toolCount).toBe(1)
    expect(preview.tools).toEqual(['Read'])
  })

  it('falls back to tauri chat when the stream ends without usable content', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        '{"model":"qwen3.6:35b","message":{"role":"assistant"},"done":true}\n',
        {
          status: 200,
          headers: { 'Content-Type': 'application/x-ndjson' },
        },
      ),
    )
    vi.stubGlobal('fetch', fetchMock)
    invokeMock.mockResolvedValue({
      endpoint: 'http://localhost:11434',
      model: 'qwen3.6:35b',
      assistantMessage: 'Fallback aus chat_turn',
      requiresApproval: false,
      proposedPlan: [],
    })

    const { sampleOllamaMessage } = await import('./ollamaClient')
    const result = await sampleOllamaMessage(
      {
        baseUrl: 'http://localhost:11434',
        model: 'qwen3.6:35b',
        timeoutMs: 200000,
        temperature: 0.1,
      },
      [{ role: 'user', content: 'Test the stream' }],
      'Systemprompt',
    )

    expect(fetchMock).toHaveBeenCalledTimes(1)
    expect(invokeMock).toHaveBeenCalledWith('chat_turn', expect.objectContaining({
      request: expect.objectContaining({
        prompt: expect.stringContaining('Test the stream'),
      }),
    }))
    expect(result.content).toEqual([
      { type: 'text', text: 'Fallback aus chat_turn' },
    ])
  })

  it('streams native thinking fields as live thinking deltas', async () => {
    Object.defineProperty(document, 'visibilityState', {
      value: 'hidden',
      configurable: true,
    })
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        '{"model":"qwen3.6:35b","message":{"role":"assistant","thinking":"kurz"},"done":false}\n{"model":"qwen3.6:35b","message":{"role":"assistant","content":"answer"},"done":true,"done_reason":"stop"}\n',
        {
          status: 200,
          headers: { 'Content-Type': 'application/x-ndjson' },
        },
      ),
    )
    vi.stubGlobal('fetch', fetchMock)

    const { streamOllamaMessages } = await import('./ollamaClient')
    const stream = streamOllamaMessages(
      {
        baseUrl: 'http://localhost:11434',
        model: 'qwen3.6:35b',
        timeoutMs: 200000,
        thinkingEnabled: true,
      },
      [{ role: 'user', content: 'Test thinking' }],
      'Systemprompt',
    )

    const events = []
    let result
    while (true) {
      const next = await stream.next()
      if (next.done) {
        result = next.value
        break
      }
      events.push(next.value)
    }

    expect(events).toContainEqual({
      type: 'content_block_delta',
      index: 0,
      delta: { type: 'thinking_delta', thinking: 'kurz' },
    })
    expect(result?.content).toContainEqual({ type: 'thinking', thinking: 'kurz' })
  })

  it('streams OpenWebUI-compatible reasoning fields as live thinking deltas', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        '{"model":"qwen3.6:35b","message":{"role":"assistant","reasoning":"Plan"},"done":false}\n{"model":"qwen3.6:35b","thinking":"Top"}\n{"model":"qwen3.6:35b","message":{"role":"assistant","content":"answer"},"done":true,"done_reason":"stop"}\n',
        {
          status: 200,
          headers: { 'Content-Type': 'application/x-ndjson' },
        },
      ),
    )
    vi.stubGlobal('fetch', fetchMock)

    const { streamOllamaMessages } = await import('./ollamaClient')
    const stream = streamOllamaMessages(
      {
        baseUrl: 'http://localhost:11434',
        model: 'qwen3.6:35b',
        timeoutMs: 200000,
        thinkingEnabled: true,
      },
      [{ role: 'user', content: 'Test reasoning' }],
      'Systemprompt',
    )

    const thinkingDeltas: string[] = []
    let result
    while (true) {
      const next = await stream.next()
      if (next.done) {
        result = next.value
        break
      }
      if (next.value.type === 'content_block_delta' && next.value.delta.type === 'thinking_delta') {
        thinkingDeltas.push(next.value.delta.thinking)
      }
    }

    expect(thinkingDeltas.join('')).toBe('PlanTop')
    expect(result?.content).toContainEqual({ type: 'thinking', thinking: 'PlanTop' })
  })

  it('parses split reasoning tags without leaking them into visible text', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        '{"model":"qwen3.6:35b","message":{"role":"assistant","content":"Vor "},"done":false}\n{"model":"qwen3.6:35b","message":{"role":"assistant","content":"<thi"},"done":false}\n{"model":"qwen3.6:35b","message":{"role":"assistant","content":"nk>abc</thi"},"done":false}\n{"model":"qwen3.6:35b","message":{"role":"assistant","content":"nk>answer"},"done":true,"done_reason":"stop"}\n',
        {
          status: 200,
          headers: { 'Content-Type': 'application/x-ndjson' },
        },
      ),
    )
    vi.stubGlobal('fetch', fetchMock)

    const { streamOllamaMessages } = await import('./ollamaClient')
    const stream = streamOllamaMessages(
      {
        baseUrl: 'http://localhost:11434',
        model: 'qwen3.6:35b',
        timeoutMs: 200000,
        thinkingEnabled: true,
      },
      [{ role: 'user', content: 'Test shared tags' }],
      'Systemprompt',
    )

    const textDeltas: string[] = []
    const thinkingDeltas: string[] = []
    let result
    while (true) {
      const next = await stream.next()
      if (next.done) {
        result = next.value
        break
      }
      if (next.value.type === 'content_block_delta' && next.value.delta.type === 'text_delta') {
        textDeltas.push(next.value.delta.text)
      }
      if (next.value.type === 'content_block_delta' && next.value.delta.type === 'thinking_delta') {
        thinkingDeltas.push(next.value.delta.thinking)
      }
    }

    expect(thinkingDeltas.join('')).toBe('abc')
    expect(textDeltas.join('')).toBe('Vor answer')
    expect(result?.content).toContainEqual({ type: 'thinking', thinking: 'abc' })
    expect(result?.content).toContainEqual({ type: 'text', text: 'Vor answer' })
  })

  it('announces native Ollama tool calls before the final assistant result', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        '{"model":"qwen3.6:35b","message":{"role":"assistant","tool_calls":[{"function":{"name":"Read","arguments":{"filename":"README.md"}}}]},"done":false}\n{"model":"qwen3.6:35b","message":{"role":"assistant","content":"Ich lese die File."},"done":true,"done_reason":"stop"}\n',
        {
          status: 200,
          headers: { 'Content-Type': 'application/x-ndjson' },
        },
      ),
    )
    vi.stubGlobal('fetch', fetchMock)

    const { streamOllamaMessages } = await import('./ollamaClient')
    const stream = streamOllamaMessages(
      {
        baseUrl: 'http://localhost:11434',
        model: 'qwen3.6:35b',
        timeoutMs: 200000,
      },
      [{ role: 'user', content: 'Lies README.md' }],
      'Systemprompt',
      [readToolDef],
    )

    const events = []
    let result
    while (true) {
      const next = await stream.next()
      if (next.done) {
        result = next.value
        break
      }
      events.push(next.value)
    }

    const liveToolEvent = events.find((event) =>
      event.type === 'content_block_start' && event.content_block.type === 'tool_use'
    )
    expect(liveToolEvent).toMatchObject({
      type: 'content_block_start',
      content_block: {
        type: 'tool_use',
        name: 'Read',
        input: {
          filename: 'README.md',
          file_path: 'README.md',
        },
      },
    })
    expect(result?.stopReason).toBe('tool_use')
    expect(result?.content).toEqual(expect.arrayContaining([
      expect.objectContaining({
        type: 'tool_use',
        name: 'Read',
        input: {
          filename: 'README.md',
          file_path: 'README.md',
        },
      }),
    ]))
  })

  it('splits large text deltas into OpenWebUI-style small chunks', async () => {
    Object.defineProperty(document, 'visibilityState', {
      value: 'hidden',
      configurable: true,
    })
    vi.spyOn(Math, 'random').mockReturnValue(0)
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        '{"model":"qwen3.6:35b","message":{"role":"assistant","content":"abcdef"},"done":true,"done_reason":"stop"}\n',
        {
          status: 200,
          headers: { 'Content-Type': 'application/x-ndjson' },
        },
      ),
    )
    vi.stubGlobal('fetch', fetchMock)

    const { streamOllamaMessages } = await import('./ollamaClient')
    const stream = streamOllamaMessages(
      {
        baseUrl: 'http://localhost:11434',
        model: 'qwen3.6:35b',
        timeoutMs: 200000,
      },
      [{ role: 'user', content: 'Stream please' }],
      'Systemprompt',
    )

    const textDeltas: string[] = []
    while (true) {
      const next = await stream.next()
      if (next.done) break
      if (next.value.type === 'content_block_delta' && next.value.delta.type === 'text_delta') {
        textDeltas.push(next.value.delta.text)
      }
    }

    expect(textDeltas).toEqual(['a', 'b', 'c', 'd', 'e', 'f'])
  })

  it('preserves structured tool calls from the tauri fallback response', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        '{"model":"qwen3.6:35b","message":{"role":"assistant"},"done":true}\n',
        {
          status: 200,
          headers: { 'Content-Type': 'application/x-ndjson' },
        },
      ),
    )
    vi.stubGlobal('fetch', fetchMock)
    invokeMock.mockResolvedValue({
      endpoint: 'http://localhost:11434',
      model: 'qwen3.6:35b',
      assistantMessage: '',
      requiresApproval: false,
      proposedPlan: [],
      toolCalls: [
        {
          function: {
            name: 'Read',
            arguments: { filename: 'C:\\workspace\\README.md' },
          },
        },
      ],
    })

    const { sampleOllamaMessage } = await import('./ollamaClient')
    const result = await sampleOllamaMessage(
      {
        baseUrl: 'http://localhost:11434',
        model: 'qwen3.6:35b',
        timeoutMs: 200000,
        temperature: 0.1,
      },
      [{ role: 'user', content: 'Lies README.md' }],
      'Systemprompt',
      [readToolDef],
    )

    expect(result.stopReason).toBe('tool_use')
    expect(result.content).toEqual([
      {
        type: 'tool_use',
        id: expect.stringContaining('ollama-fallback-tool-'),
        name: 'Read',
        input: {
          filename: 'C:\\workspace\\README.md',
          file_path: 'C:\\workspace\\README.md',
        },
      },
    ])
  })

  it('normalizes native ollama tool call arguments to the canonical schema keys', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        '{"model":"qwen3.6:35b","message":{"role":"assistant","tool_calls":[{"function":{"name":"Read","arguments":{"filename":"README.md"}}}]},"done":true,"done_reason":"stop"}\n',
        {
          status: 200,
          headers: { 'Content-Type': 'application/x-ndjson' },
        },
      ),
    )
    vi.stubGlobal('fetch', fetchMock)

    const { sampleOllamaMessage } = await import('./ollamaClient')
    const result = await sampleOllamaMessage(
      {
        baseUrl: 'http://localhost:11434',
        model: 'qwen3.6:35b',
        timeoutMs: 200000,
        temperature: 0.1,
      },
      [{ role: 'user', content: 'Lies README.md' }],
      'Systemprompt',
      [readToolDef],
    )

    expect(result.stopReason).toBe('tool_use')
    expect(result.content).toEqual([
      {
        type: 'tool_use',
        id: expect.stringContaining('ollama-tool-'),
        name: 'Read',
        input: {
          filename: 'README.md',
          file_path: 'README.md',
        },
      },
    ])
    expect(invokeMock).not.toHaveBeenCalled()
  })

  it('forwards tool definitions to tauri fallback requests', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        '{"model":"qwen3.6:35b","message":{"role":"assistant"},"done":true}\n',
        {
          status: 200,
          headers: { 'Content-Type': 'application/x-ndjson' },
        },
      ),
    )
    vi.stubGlobal('fetch', fetchMock)
    invokeMock.mockResolvedValue({
      endpoint: 'http://localhost:11434',
      model: 'qwen3.6:35b',
      assistantMessage: 'Fallback aus chat_turn',
      requiresApproval: false,
      proposedPlan: [],
    })

    const { sampleOllamaMessage } = await import('./ollamaClient')
    await sampleOllamaMessage(
      {
        baseUrl: 'http://localhost:11434',
        model: 'qwen3.6:35b',
        timeoutMs: 200000,
        temperature: 0.1,
      },
      [{ role: 'user', content: 'Lies README.md' }],
      'Systemprompt',
      [readToolDef],
    )

    expect(invokeMock).toHaveBeenCalledWith('chat_turn', expect.objectContaining({
      request: expect.objectContaining({
        tools: [
          {
            type: 'function',
            function: {
              name: 'Read',
              description: 'Liest eine File.',
              parameters: {
                type: 'object',
                properties: {
                  file_path: { type: 'string', description: 'Pfad zur File' },
                },
                required: ['file_path'],
              },
            },
          },
        ],
      }),
    }))
  })
})
