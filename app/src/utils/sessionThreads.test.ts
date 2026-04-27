import { describe, expect, it } from 'vitest'
import type { Message } from '../engine'
import { hydrateStoredMessage, toChatMessages } from './sessionThreads'

describe('sessionThreads', () => {
  it('renders tool-only persisted messages into readable chat content', () => {
    const messages: Message[] = [
      {
        type: 'assistant',
        uuid: 'assistant-1',
        content: [{ type: 'tool_use', id: 'tool-1', name: 'ListDir', input: { path: 'C:/workspace' } }],
        model: 'gpt-oss:20b',
        usage: { input_tokens: 0, output_tokens: 0 },
        stopReason: 'tool_use',
        timestamp: 1,
      },
      {
        type: 'user',
        uuid: 'user-1',
        content: [{ type: 'tool_result', tool_use_id: 'tool-1', content: 'Datei A\nDatei B' }],
        timestamp: 2,
      },
    ]

    const mapped = toChatMessages(messages)

    expect(mapped[0]?.content).toContain('Tool-Aufruf: ListDir')
    expect(mapped[0]?.content).toContain('C:/workspace')
    expect(mapped[0]?.content).not.toBe('[assistant]')
    expect(mapped[0]?.debugContent).toContain('"tool_use"')
    expect(mapped[1]?.content).toContain('Tool-Ergebnis: Datei A')
    expect(mapped[1]?.content).not.toBe('[user]')
    expect(mapped[1]?.debugContent).toContain('"tool_result"')
    expect(mapped[1]?.role).toBe('assistant')
  })

  it('hydrates stored JSON messages from the database into readable chat entries', () => {
    const serialized = JSON.stringify({
      type: 'assistant',
      uuid: 'assistant-1',
      content: [{ type: 'tool_use', id: 'tool-1', name: 'Read', input: { file_path: 'C:/workspace/note.txt' } }],
      model: 'gpt-oss:20b',
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
})