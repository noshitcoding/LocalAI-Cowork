import { beforeEach, describe, expect, it, vi } from 'vitest'

const invokeMock = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}))

describe('memory tools', () => {
  beforeEach(() => {
    invokeMock.mockReset()
  })

  it('registers curated mutation and past-chat search tools', async () => {
    const { getAllTools, registerAllBuiltinTools } = await import('./registry')
    registerAllBuiltinTools()

    expect(getAllTools().some((tool) => tool.name === 'MemoryWrite')).toBe(true)
    expect(getAllTools().some((tool) => tool.name === 'ChatSearch')).toBe(true)
  })

  it('maps add, replace, and remove inputs to the bounded backend mutation contract', async () => {
    const { getAllTools, registerAllBuiltinTools } = await import('./registry')
    registerAllBuiltinTools()
    const tool = getAllTools().find((entry) => entry.name === 'MemoryWrite')!
    invokeMock.mockResolvedValue({
      success: true,
      changed: true,
      action: 'replace',
      target: 'memory',
      message: 'Memory replace succeeded.',
      usageChars: 42,
      limitChars: 2200,
      entries: ['Project uses SQLite.'],
    })

    const result = await tool.call(
      { action: 'replace', target: 'memory', old_text: 'Postgres', content: 'Project uses SQLite.' },
      { cwd: 'C:/workspace', runId: 'run-1', threadId: 'thread-1' } as never,
    )

    expect(invokeMock).toHaveBeenCalledWith('memory_mutate', {
      action: 'replace',
      target: 'memory',
      oldText: 'Postgres',
      content: 'Project uses SQLite.',
      sourceRunId: 'run-1',
    })
    expect(String(result.data)).toContain('Usage: 42/2200 chars')
  })

  it('reads user-profile memory from its dedicated backend table', async () => {
    const { getAllTools, registerAllBuiltinTools } = await import('./registry')
    registerAllBuiltinTools()
    const tool = getAllTools().find((entry) => entry.name === 'MemoryRead')!
    invokeMock.mockResolvedValue([
      { key: 'style', value: 'Concise answers', source: 'agent', confidence: 1 },
    ])

    const result = await tool.call({ scope: 'user' }, { cwd: 'C:/workspace', runId: 'run-1' } as never)

    expect(invokeMock).toHaveBeenCalledWith('user_profile_list')
    expect(String(result.data)).toContain('[user/style]: Concise answers')
  })

  it('returns persisted chat matches through ChatSearch', async () => {
    const { getAllTools, registerAllBuiltinTools } = await import('./registry')
    registerAllBuiltinTools()
    const tool = getAllTools().find((entry) => entry.name === 'ChatSearch')!
    invokeMock.mockResolvedValue([
      {
        thread_id: 'thread-old',
        thread_title: 'Database decision',
        updated_at: '2026-07-10T10:00:00Z',
        matched_content: 'We selected SQLite.',
        matched_role: 'assistant',
      },
    ])

    const result = await tool.call({ query: 'SQLite', limit: 5 }, { cwd: 'C:/workspace', runId: 'run-1' } as never)

    expect(invokeMock).toHaveBeenCalledWith('chat_search', { query: 'SQLite', limit: 5 })
    expect(String(result.data)).toContain('We selected SQLite.')
  })
})
