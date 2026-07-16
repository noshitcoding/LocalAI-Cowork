import { beforeEach, describe, expect, it, vi } from 'vitest'

const invokeMock = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}))

describe('memorySystem IPC contracts', () => {
  beforeEach(() => {
    invokeMock.mockReset()
  })

  it('stores memory entries via memory_upsert with mapped scope', async () => {
    const { storeMemoryEntry } = await import('./memorySystem')

    invokeMock.mockResolvedValue(undefined)

    await storeMemoryEntry({
      scope: 'project',
      category: 'notes',
      key: 'k1',
      content: 'value',
      confidence: 0.8,
    })

    expect(invokeMock).toHaveBeenCalledWith(
      'memory_upsert',
      expect.objectContaining({
        scope: 'agent',
        category: 'notes',
        key: 'k1',
        content: 'value',
        confidence: 0.8,
      }),
    )
  })

  it('reads memory entries via memory_search and maps backend shape', async () => {
    const { getMemoryEntries } = await import('./memorySystem')

    invokeMock.mockResolvedValue([
      {
        id: 'm1',
        scope: 'shared',
        category: 'prefs',
        key: 'lang',
        content: 'de',
        confidence: 1,
        created_at: '2026-04-22T10:00:00.000Z',
        updated_at: '2026-04-22T11:00:00.000Z',
      },
    ])

    const result = await getMemoryEntries('global', 'prefs')

    expect(invokeMock).toHaveBeenCalledWith(
      'memory_search',
      expect.objectContaining({
        scope: 'shared',
        category: 'prefs',
      }),
    )
    expect(result).toHaveLength(1)
    expect(result[0].scope).toBe('global')
    expect(Number.isFinite(result[0].createdAt)).toBe(true)
    expect(Number.isFinite(result[0].updatedAt)).toBe(true)
  })

  it('captures high-signal facts in the draft file and draft database scope', async () => {
    const { captureAutomaticMemoryDraft } = await import('./memorySystem')

    invokeMock.mockImplementation(async (command: string) => {
      if (command === 'fs_extract_text') throw new Error('not found')
      return undefined
    })

    const candidates = await captureAutomaticMemoryDraft(
      'C:/workspace',
      'Merke dir: Das Projekt nutzt SQLite fuer die lokale Langzeit-Memory.',
      'session-1',
    )

    expect(candidates).toEqual([
      { target: 'memory', content: 'Das Projekt nutzt SQLite fuer die lokale Langzeit-Memory.' },
    ])
    expect(invokeMock).toHaveBeenCalledWith(
      'memory_upsert',
      expect.objectContaining({
        scope: 'shared',
        category: 'draft_knowledge',
        sourceSessionId: 'session-1',
      }),
    )
    expect(invokeMock).toHaveBeenCalledWith(
      'fs_write_text_file',
      expect.objectContaining({
        path: 'C:/workspace/.cowork/DRAFT_KNOWLEDGE.md',
        content: expect.stringContaining('- [memory] Das Projekt nutzt SQLite'),
      }),
    )
  })

  it('returns base prompt and memory separately so QueryEngine injects memory once', async () => {
    const { buildSystemPromptWithMemory } = await import('./memorySystem')
    invokeMock.mockImplementation(async (command: string) => {
      if (command === 'fs_extract_text') throw new Error('not found')
      if (command === 'memory_search' || command === 'runtime_instruction_effective') return []
      return null
    })

    const result = await buildSystemPromptWithMemory('C:/workspace', 'BASE PROMPT', {
      frozenSnapshot: {
        sessionId: 'session-1',
        agentEntries: [{
          id: 'a1', scope: 'agent', category: 'curated', key: 'stack', content: 'The project uses Rust.', confidence: 1,
        }],
        sharedEntries: [],
        userProfile: [],
        createdAt: '2026-07-16T10:00:00Z',
      },
    })

    expect(result.systemPrompt).toBe('BASE PROMPT')
    expect(result.systemPrompt).not.toContain('<memory>')
    expect(result.memoryContent).toContain('The project uses Rust.')
  })
})
