import { afterEach, describe, expect, it, vi } from 'vitest'

import {
  AnthropicProtocolError,
  streamMessages,
  type AnthropicConfig,
  type SampleResult,
} from './anthropicClient'
import type { StreamEvent } from '../types'

const config: AnthropicConfig = {
  apiKey: 'test-key',
  model: 'claude-test',
}

function responseFromChunks(chunks: Uint8Array[]): Response {
  return {
    ok: true,
    body: new ReadableStream<Uint8Array>({
      start(controller) {
        chunks.forEach((chunk) => controller.enqueue(chunk))
        controller.close()
      },
    }),
  } as Response
}

async function consumeStream(source: string, splitEveryByte = false): Promise<{
  events: StreamEvent[]
  result: SampleResult
}> {
  const encoded = new TextEncoder().encode(source)
  const chunks = splitEveryByte
    ? Array.from(encoded, (_byte, index) => encoded.slice(index, index + 1))
    : [encoded]
  vi.stubGlobal('fetch', vi.fn(async () => responseFromChunks(chunks)))
  const generator = streamMessages(config, [{ role: 'user', content: 'test' }], 'system')
  const events: StreamEvent[] = []

  while (true) {
    const next = await generator.next()
    if (next.done) return { events, result: next.value }
    events.push(next.value)
  }
}

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('Anthropic SSE streaming', () => {
  it('drains CRLF events split at every byte and a final event without a newline', async () => {
    const records = [
      { type: 'message_start', message: { id: 'message-1', model: 'claude-test', usage: { input_tokens: 3 } } },
      { type: 'content_block_start', index: 0, content_block: { type: 'text', text: '' } },
      { type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text: 'Grüße' } },
      { type: 'content_block_stop', index: 0 },
      { type: 'message_delta', delta: { stop_reason: 'end_turn' }, usage: { output_tokens: 2 } },
      { type: 'message_stop' },
    ]
    const source = records.map((record) => `data: ${JSON.stringify(record)}`).join('\r\n\r\n')

    const { events, result } = await consumeStream(source, true)

    expect(events.map((event) => event.type)).toEqual([
      'message_start',
      'content_block_start',
      'content_block_delta',
      'content_block_stop',
      'message_delta',
      'message_stop',
    ])
    expect(result.content).toEqual([{ type: 'text', text: 'Grüße' }])
    expect(result.stopReason).toBe('end_turn')
    expect(result.usage).toMatchObject({ input_tokens: 3, output_tokens: 2 })
  })

  it.each([
    ['truncated JSON', '{"path":'],
    ['array input', '[]'],
    ['primitive input', '42'],
  ])('rejects %s instead of completing a tool call', async (_label, partialJson) => {
    const records = [
      { type: 'content_block_start', index: 0, content_block: { type: 'tool_use', id: 'tool-1', name: 'read_file', input: {} } },
      { type: 'content_block_delta', index: 0, delta: { type: 'input_json_delta', partial_json: partialJson } },
      { type: 'content_block_stop', index: 0 },
    ]
    const source = records.map((record) => `data:${JSON.stringify(record)}`).join('\n')
    vi.stubGlobal('fetch', vi.fn(async () => responseFromChunks([new TextEncoder().encode(source)])))
    const generator = streamMessages(config, [{ role: 'user', content: 'test' }], 'system')
    const yieldedTypes: string[] = []

    await expect((async () => {
      while (true) {
        const next = await generator.next()
        if (next.done) return
        yieldedTypes.push(next.value.type)
      }
    })()).rejects.toBeInstanceOf(AnthropicProtocolError)
    expect(yieldedTypes).not.toContain('content_block_stop')
  })
})
