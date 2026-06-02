import { beforeEach, describe, expect, it, vi } from 'vitest'

const hasTauriRuntimeMock = vi.fn(() => false)
const safeInvokeMock = vi.fn()

vi.mock('../../utils/safeInvoke', () => ({
  hasTauriRuntime: () => hasTauriRuntimeMock(),
  safeInvoke: (...args: unknown[]) => safeInvokeMock(...args),
}))

const readToolDef = {
  name: 'Read',
  description: 'Liest eine Datei.',
  input_schema: {
    type: 'object' as const,
    properties: {
      file_path: { type: 'string', description: 'Pfad zur Datei' },
    },
    required: ['file_path'] as string[],
  },
}

describe('streamOpenAiCompatibleMessages', () => {
  beforeEach(() => {
    vi.restoreAllMocks()
    vi.unstubAllGlobals()
    hasTauriRuntimeMock.mockReturnValue(false)
    safeInvokeMock.mockReset()
  })

  it('serializes tool results and image inputs for chat completions', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          id: 'resp-1',
          model: 'gpt-4.1-mini',
          usage: { prompt_tokens: 10, completion_tokens: 4 },
          choices: [{ finish_reason: 'stop', message: { content: 'Analyse abgeschlossen.' } }],
        }),
        {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        },
      ),
    )
    vi.stubGlobal('fetch', fetchMock)

    const { streamOpenAiCompatibleMessages } = await import('./openaiCompatibleClient')
    const stream = streamOpenAiCompatibleMessages(
      {
        provider: 'openai-compatible',
        apiKey: 'sk-test',
        model: 'gpt-4.1-mini',
        baseUrl: 'https://api.openai.com/v1',
      },
      [{
        role: 'user',
        content: [
          { type: 'tool_result', tool_use_id: 'tool-1', content: 'Screenshot aufgenommen.' },
          { type: 'text', text: 'Bitte analysiere den aktuellen Bildschirm.' },
          { type: 'image', source: { type: 'base64', media_type: 'image/png', data: 'AAA' } },
        ],
      }],
      'Systemprompt',
      [readToolDef],
    )

    while (!(await stream.next()).done) {
      // consume stream
    }

    const [url, requestInit] = fetchMock.mock.calls[0] as [string, RequestInit]
    const body = JSON.parse(String(requestInit.body))
    expect(url).toBe('https://api.openai.com/v1/chat/completions')
    expect(body.messages).toEqual([
      { role: 'system', content: 'Systemprompt' },
      { role: 'tool', content: 'Screenshot aufgenommen.', tool_call_id: 'tool-1' },
      {
        role: 'user',
        content: [
          { type: 'text', text: 'Bitte analysiere den aktuellen Bildschirm.' },
          { type: 'image_url', image_url: { url: 'data:image/png;base64,AAA' } },
        ],
      },
    ])
    expect(body.tools).toMatchObject([
      {
        type: 'function',
        function: { name: 'Read' },
      },
    ])
  })

  it('adds /v1/chat/completions for service-root OpenAI-compatible endpoints', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          id: 'resp-root',
          model: '0xSero/Hy3-preview-nvfp4',
          usage: { prompt_tokens: 3, completion_tokens: 1 },
          choices: [{ finish_reason: 'stop', message: { content: 'ok' } }],
        }),
        {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        },
      ),
    )
    vi.stubGlobal('fetch', fetchMock)

    const { streamOpenAiCompatibleMessages } = await import('./openaiCompatibleClient')
    const stream = streamOpenAiCompatibleMessages(
      {
        provider: 'openai-compatible',
        apiKey: 'sk-test',
        model: '0xSero/Hy3-preview-nvfp4',
        baseUrl: 'https://mlis.example.test',
      },
      [{ role: 'user', content: 'Ping' }],
      'Systemprompt',
    )

    while (!(await stream.next()).done) {
      // consume stream
    }

    const [url] = fetchMock.mock.calls[0] as [string, RequestInit]
    expect(url).toBe('https://mlis.example.test/v1/chat/completions')
  })

  it('retries model-not-found errors with a matching fully-qualified provider model id', async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(
        new Response(
          JSON.stringify({
            error: {
              message: "The model 'Hy3-preview-nvfp4' does not exist.",
              type: 'NotFoundError',
              param: 'model',
              code: 404,
            },
          }),
          {
            status: 404,
            headers: { 'Content-Type': 'application/json' },
          },
        ),
      )
      .mockResolvedValueOnce(
        new Response(
          JSON.stringify({
            data: [{ id: '0xSero/Hy3-preview-nvfp4' }],
          }),
          {
            status: 200,
            headers: { 'Content-Type': 'application/json' },
          },
        ),
      )
      .mockResolvedValueOnce(
        new Response(
          JSON.stringify({
            id: 'resp-qualified-model',
            model: '0xSero/Hy3-preview-nvfp4',
            usage: { prompt_tokens: 4, completion_tokens: 2 },
            choices: [{ finish_reason: 'stop', message: { content: 'ok' } }],
          }),
          {
            status: 200,
            headers: { 'Content-Type': 'application/json' },
          },
        ),
      )
    vi.stubGlobal('fetch', fetchMock)

    const { streamOpenAiCompatibleMessages } = await import('./openaiCompatibleClient')
    const stream = streamOpenAiCompatibleMessages(
      {
        provider: 'openai-compatible',
        apiKey: 'sk-test',
        model: 'Hy3-preview-nvfp4',
        baseUrl: 'https://mlis.example.test/v1',
      },
      [{ role: 'user', content: 'Ping' }],
      'Systemprompt',
    )

    let result: Awaited<ReturnType<typeof stream.next>>['value'] | null = null
    while (true) {
      const next = await stream.next()
      if (next.done) {
        result = next.value
        break
      }
    }

    expect(fetchMock).toHaveBeenCalledTimes(3)
    expect(fetchMock.mock.calls[1][0]).toBe('https://mlis.example.test/v1/models')

    const [, firstInit] = fetchMock.mock.calls[0] as [string, RequestInit]
    const [, retryInit] = fetchMock.mock.calls[2] as [string, RequestInit]
    expect(JSON.parse(String(firstInit.body)).model).toBe('Hy3-preview-nvfp4')
    expect(JSON.parse(String(retryInit.body)).model).toBe('0xSero/Hy3-preview-nvfp4')
    expect(result).toMatchObject({
      model: '0xSero/Hy3-preview-nvfp4',
      content: [{ type: 'text', text: 'ok' }],
    })
  })

  it('maps tool calls into tool_use blocks for the engine loop', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          id: 'resp-2',
          model: 'openai/gpt-4o-mini',
          usage: { prompt_tokens: 21, completion_tokens: 9 },
          choices: [{
            finish_reason: 'tool_calls',
            message: {
              tool_calls: [{
                id: 'call-1',
                function: {
                  name: 'Read',
                  arguments: '{"file_path":"README.md"}',
                },
              }],
            },
          }],
        }),
        {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        },
      ),
    )
    vi.stubGlobal('fetch', fetchMock)

    const { streamOpenAiCompatibleMessages } = await import('./openaiCompatibleClient')
    const stream = streamOpenAiCompatibleMessages(
      {
        provider: 'openrouter',
        apiKey: 'sk-or-test',
        model: 'openai/gpt-4o-mini',
        baseUrl: 'https://openrouter.ai/api/v1',
      },
      [{ role: 'user', content: 'Lies die README.' }],
      'Systemprompt',
      [readToolDef],
    )

    const events = [] as Array<{ type: string; [key: string]: unknown }>
    let result: Awaited<ReturnType<typeof stream.next>>['value'] | null = null

    while (true) {
      const next = await stream.next()
      if (next.done) {
        result = next.value
        break
      }
      events.push(next.value as { type: string; [key: string]: unknown })
    }

    expect(events).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          type: 'content_block_start',
          content_block: expect.objectContaining({
            type: 'tool_use',
            id: 'call-1',
            name: 'Read',
            input: { file_path: 'README.md' },
          }),
        }),
      ]),
    )
    expect(result).toMatchObject({
      model: 'openai/gpt-4o-mini',
      stopReason: 'tool_use',
      usage: { input_tokens: 21, output_tokens: 9 },
    })
  })

  it('requests OpenRouter reasoning and maps it into thinking deltas', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          id: 'resp-reasoning',
          model: 'nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free',
          usage: { prompt_tokens: 12, completion_tokens: 8 },
          choices: [{
            finish_reason: 'stop',
            message: {
              reasoning: 'Ich pruefe zuerst die Bedingung.',
              content: 'Die Antwort ist 4.',
            },
          }],
        }),
        {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        },
      ),
    )
    vi.stubGlobal('fetch', fetchMock)

    const { streamOpenAiCompatibleMessages } = await import('./openaiCompatibleClient')
    const stream = streamOpenAiCompatibleMessages(
      {
        provider: 'openrouter',
        apiKey: 'sk-or-test',
        model: 'nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free',
        baseUrl: 'https://openrouter.ai/api/v1',
      },
      [{ role: 'user', content: 'Was ist 2+2?' }],
      'Systemprompt',
    )

    const events = [] as Array<{ type: string; [key: string]: unknown }>
    let result: Awaited<ReturnType<typeof stream.next>>['value'] | null = null

    while (true) {
      const next = await stream.next()
      if (next.done) {
        result = next.value
        break
      }
      events.push(next.value as { type: string; [key: string]: unknown })
    }

    const [, requestInit] = fetchMock.mock.calls[0] as [string, RequestInit]
    const body = JSON.parse(String(requestInit.body))
    expect(body.model).toBe('nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free')
    expect(body.reasoning).toEqual({ enabled: true, exclude: false })
    expect(events).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          type: 'content_block_delta',
          delta: {
            type: 'thinking_delta',
            thinking: 'Ich pruefe zuerst die Bedingung.',
          },
        }),
        expect.objectContaining({
          type: 'content_block_delta',
          delta: {
            type: 'text_delta',
            text: 'Die Antwort ist 4.',
          },
        }),
      ]),
    )
    expect(result).toMatchObject({
      content: [
        { type: 'thinking', thinking: 'Ich pruefe zuerst die Bedingung.' },
        { type: 'text', text: 'Die Antwort ist 4.' },
      ],
    })
  })

  it('preserves assistant reasoning for follow-up OpenRouter turns', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          id: 'resp-follow-up',
          model: 'openai/gpt-4o-mini',
          usage: { prompt_tokens: 15, completion_tokens: 3 },
          choices: [{ finish_reason: 'stop', message: { content: 'Weiter geht es.' } }],
        }),
        {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        },
      ),
    )
    vi.stubGlobal('fetch', fetchMock)

    const { streamOpenAiCompatibleMessages } = await import('./openaiCompatibleClient')
    const stream = streamOpenAiCompatibleMessages(
      {
        provider: 'openrouter',
        apiKey: 'sk-or-test',
        model: 'openai/gpt-4o-mini',
        baseUrl: 'https://openrouter.ai/api/v1',
      },
      [
        {
          role: 'assistant',
          content: [
            { type: 'thinking', thinking: 'Ich muss erst den Dateibaum auswerten.' },
            { type: 'text', text: 'Ich pruefe jetzt die Dateien.' },
          ],
        },
        { role: 'user', content: 'Bitte mache weiter.' },
      ],
      'Systemprompt',
    )

    while (!(await stream.next()).done) {
      // consume stream
    }

    const [, requestInit] = fetchMock.mock.calls[0] as [string, RequestInit]
    const body = JSON.parse(String(requestInit.body))
    expect(body.messages).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          role: 'assistant',
          content: 'Ich pruefe jetzt die Dateien.',
          reasoning: 'Ich muss erst den Dateibaum auswerten.',
        }),
      ]),
    )
  })

  it('retries OpenRouter requests without image input after unsupported-image 404 errors', async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(
        new Response(
          JSON.stringify({
            error: {
              message: 'No endpoints found that support image input',
              code: 404,
            },
          }),
          {
            status: 404,
            headers: { 'Content-Type': 'application/json' },
          },
        ),
      )
      .mockResolvedValueOnce(
        new Response(
          JSON.stringify({
            id: 'resp-fallback',
            model: 'meta-llama/llama-3.3-70b-instruct',
            usage: { prompt_tokens: 18, completion_tokens: 7 },
            choices: [{
              finish_reason: 'stop',
              message: {
                content: 'Ich nutze den Textkontext ohne Bild.',
              },
            }],
          }),
          {
            status: 200,
            headers: { 'Content-Type': 'application/json' },
          },
        ),
      )
    vi.stubGlobal('fetch', fetchMock)

    const { streamOpenAiCompatibleMessages } = await import('./openaiCompatibleClient')
    const stream = streamOpenAiCompatibleMessages(
      {
        provider: 'openrouter',
        apiKey: 'sk-or-test',
        model: 'meta-llama/llama-3.3-70b-instruct',
        baseUrl: 'https://openrouter.ai/api/v1',
      },
      [{
        role: 'user',
        content: [
          { type: 'text', text: 'Bitte analysiere den Screenshot.' },
          { type: 'image', source: { type: 'base64', media_type: 'image/png', data: 'AAA' } },
        ],
      }],
      'Systemprompt',
      [readToolDef],
    )

    let result: Awaited<ReturnType<typeof stream.next>>['value'] | null = null
    while (true) {
      const next = await stream.next()
      if (next.done) {
        result = next.value
        break
      }
    }

    expect(fetchMock).toHaveBeenCalledTimes(2)

    const [, firstInit] = fetchMock.mock.calls[0] as [string, RequestInit]
    const firstBody = JSON.parse(String(firstInit.body))
    expect(firstBody.messages).toEqual([
      { role: 'system', content: 'Systemprompt' },
      {
        role: 'user',
        content: [
          { type: 'text', text: 'Bitte analysiere den Screenshot.' },
          { type: 'image_url', image_url: { url: 'data:image/png;base64,AAA' } },
        ],
      },
    ])

    const [, secondInit] = fetchMock.mock.calls[1] as [string, RequestInit]
    const secondBody = JSON.parse(String(secondInit.body))
    expect(secondBody.messages).toEqual([
      { role: 'system', content: 'Systemprompt' },
      { role: 'user', content: 'Bitte analysiere den Screenshot.' },
    ])
    expect(result).toMatchObject({
      model: 'meta-llama/llama-3.3-70b-instruct',
      stopReason: 'end_turn',
      content: [{ type: 'text', text: 'Ich nutze den Textkontext ohne Bild.' }],
    })
  })

  it('surfaces normalized OpenRouter choice errors instead of masking them as empty responses', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          id: 'resp-choice-error',
          model: 'openai/gpt-4o-mini',
          choices: [{
            finish_reason: 'error',
            error: {
              code: 502,
              message: 'Upstream provider timeout',
            },
            message: { content: null },
          }],
        }),
        {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        },
      ),
    )
    vi.stubGlobal('fetch', fetchMock)

    const { streamOpenAiCompatibleMessages } = await import('./openaiCompatibleClient')
    const stream = streamOpenAiCompatibleMessages(
      {
        provider: 'openrouter',
        apiKey: 'sk-or-test',
        model: 'openai/gpt-4o-mini',
        baseUrl: 'https://openrouter.ai/api/v1',
      },
      [{ role: 'user', content: 'Bitte antworte.' }],
      'Systemprompt',
    )

    await expect((async () => {
      while (!(await stream.next()).done) {
        // consume stream
      }
    })()).rejects.toThrow('OpenRouter API Error (502): Upstream provider timeout')
  })

  it('uses the native Tauri request path when TLS verification is disabled', async () => {
    hasTauriRuntimeMock.mockReturnValue(true)
    safeInvokeMock.mockResolvedValue({
      status: 200,
      body: JSON.stringify({
        id: 'resp-native',
        model: 'local/gpt',
        usage: { prompt_tokens: 5, completion_tokens: 2 },
        choices: [{ finish_reason: 'stop', message: { content: 'OK' } }],
      }),
    })
    const fetchMock = vi.fn()
    vi.stubGlobal('fetch', fetchMock)

    const { streamOpenAiCompatibleMessages } = await import('./openaiCompatibleClient')
    const stream = streamOpenAiCompatibleMessages(
      {
        provider: 'openai-compatible',
        apiKey: 'sk-test',
        model: 'local/gpt',
        baseUrl: 'https://self-signed.local/v1',
        verifyTlsCertificates: false,
      },
      [{ role: 'user', content: 'Hallo' }],
      'Systemprompt',
    )

    while (!(await stream.next()).done) {
      // consume stream
    }

    expect(fetchMock).not.toHaveBeenCalled()
    expect(safeInvokeMock).toHaveBeenCalledWith('openai_compatible_chat_completion', {
      request: expect.objectContaining({
        endpoint: 'https://self-signed.local/v1/chat/completions',
        verifyTlsCertificates: false,
      }),
    })
  })
})
