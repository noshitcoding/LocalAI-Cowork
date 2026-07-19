import { describe, expect, it } from 'vitest'
import type { Message } from '../engine'
import { hydrateStoredMessage, serializeChatMessageForStorage, toChatMessages } from './sessionThreads'

describe('sessionThreads', () => {
  it('renders tool-only persisted messages into readable chat content', () => {
    const messages: Message[] = [
      {
        type: 'assistant',
        uuid: 'assistant-1',
        content: [{ type: 'tool_use', id: 'tool-1', name: 'ListDir', input: { path: 'C:/workspace' } }],
        model: 'llama3.1:8b',
        usage: { input_tokens: 0, output_tokens: 0 },
        stopReason: 'tool_use',
        timestamp: 1,
      },
      {
        type: 'user',
        uuid: 'user-1',
        content: [{ type: 'tool_result', tool_use_id: 'tool-1', content: 'File A\nFile B' }],
        timestamp: 2,
      },
    ]

    const mapped = toChatMessages(messages)

    expect(mapped[0]?.content).toContain('Tool-Aufruf: ListDir')
    expect(mapped[0]?.content).toContain('C:/workspace')
    expect(mapped[0]?.content).not.toBe('[assistant]')
    expect(mapped[0]?.debugContent).toContain('"tool_use"')
    expect(mapped[1]?.content).toContain('Tool-Result: File A')
    expect(mapped[1]?.content).not.toBe('[user]')
    expect(mapped[1]?.debugContent).toContain('"tool_result"')
    expect(mapped[1]?.role).toBe('assistant')
  })

  it('hydrates stored JSON messages from the database into readable chat entries', () => {
    const serialized = JSON.stringify({
      type: 'assistant',
      uuid: 'assistant-1',
      content: [{ type: 'tool_use', id: 'tool-1', name: 'Read', input: { file_path: 'C:/workspace/note.txt' } }],
      model: 'llama3.1:8b',
      usage: { input_tokens: 0, output_tokens: 0 },
      stopReason: 'tool_use',
      timestamp: 1,
    })

    const hydrated = hydrateStoredMessage({
      id: 'db-message-1',
      role: 'assistant',
      content: serialized,
      timestamp: 1,
    })

    expect(hydrated.content).toContain('Tool-Aufruf: Read')
    expect(hydrated.content).toContain('note.txt')
    expect(hydrated.debugContent).toBe(serialized)
  })

  it('hydrates LocalAI Cowork chat payload metadata from the database', () => {
    const serialized = serializeChatMessageForStorage({
      id: 'assistant-1',
      role: 'assistant',
      content: 'answer',
      timestamp: 10,
      thinkingContent: 'Gedanke',
      verboseContent: 'Verbose',
      liveToolCalls: [{
        id: 'tool-1',
        toolName: 'Read',
        input: { path: 'README.md' },
        status: 'completed',
        result: 'ok',
        startedAt: 10,
        finishedAt: 11,
      }],
    })

    const hydrated = hydrateStoredMessage({
      id: 'db-message-1',
      role: 'assistant',
      content: serialized,
      timestamp: 12,
    })

    expect(hydrated.id).toBe('db-message-1')
    expect(hydrated.content).toBe('answer')
    expect(hydrated.thinkingContent).toBe('Gedanke')
    expect(hydrated.verboseContent).toBe('Verbose')
    expect(hydrated.liveToolCalls?.[0]?.toolName).toBe('Read')
    expect(hydrated.streaming).toBe(false)
  })
})
