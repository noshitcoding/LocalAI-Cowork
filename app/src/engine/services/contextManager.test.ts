import { describe, expect, it } from 'vitest'
import type { Message } from '../types'
import { ContextManager } from './contextManager'

function user(id: string, text: string): Message {
  return {
    type: 'user',
    uuid: id,
    content: [{ type: 'text', text }],
    timestamp: 1,
  }
}

function assistant(id: string, text: string): Message {
  return {
    type: 'assistant',
    uuid: id,
    content: [{ type: 'text', text }],
    model: 'test-model',
    stopReason: 'end_turn',
    usage: { input_tokens: 0, output_tokens: 0 },
    timestamp: 1,
  }
}

describe('ContextManager deterministic trimming', () => {
  it('drops the oldest complete turn while retaining the current user message', () => {
    const manager = new ContextManager({ maxContextTokens: 100 })
    const messages = [
      user('old-user', 'u'.repeat(160)),
      assistant('old-assistant', 'a'.repeat(160)),
      user('current-user', 'current request'),
    ]

    const result = manager.trimToBudget(messages, 5, 0.8)

    expect(result.fits).toBe(true)
    expect(result.droppedCount).toBe(2)
    expect(result.messages.map((message) => message.uuid)).toEqual(['current-user'])
    expect(messages).toHaveLength(3)
  })

  it('rejects an oversized current request without dropping that request', () => {
    const manager = new ContextManager({ maxContextTokens: 40 })
    const messages = [user('current-user', 'x'.repeat(500))]

    const result = manager.trimToBudget(messages, 20, 0.8)

    expect(result.fits).toBe(false)
    expect(result.droppedCount).toBe(0)
    expect(result.messages[0]?.uuid).toBe('current-user')
  })
})
