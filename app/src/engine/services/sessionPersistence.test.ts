import { beforeEach, describe, expect, it, vi } from 'vitest'
import { generateSessionTitle, listSessions, loadSession } from './sessionPersistence'

const invokeMock = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}))

describe('sessionPersistence', () => {
  beforeEach(() => {
    invokeMock.mockReset()
  })

  it('generates a readable session title from the first user message', () => {
    const title = generateSessionTitle([
      {
        type: 'user',
        content: [{ type: 'text', text: 'Analyze the latest build errors and prioritize the fixes for me.' }],
      } as never,
    ])

    expect(title).toContain('Analyze the latest build errors')
    expect(title.endsWith('...')).toBe(true)
  })

  it('maps db thread summaries into session summaries', async () => {
    invokeMock.mockImplementation((command: string) => {
      if (command === 'session_list') {
        return Promise.resolve([
          {
            id: 'thread-1',
            title: 'Build Review',
            total_messages: 7,
            started_at: '1970-01-01T00:00:01.000Z', // 1000ms since epoch
            ended_at: '1970-01-01T00:00:02.000Z', // 2000ms since epoch
          },
        ])
      }
      return Promise.resolve([])
    })

    await expect(listSessions()).resolves.toEqual([
      {
        id: 'thread-1',
        title: 'Build Review',
        cwd: '',
        messageCount: 7,
        createdAt: 1000,
        updatedAt: 2000,
      },
    ])
  })

  it('loads legacy role/content rows as valid session messages', async () => {
    // Mock session_get to return a legacy session
    invokeMock.mockImplementation((command: string) => {
      if (command === 'session_get') {
        return Promise.resolve({
          id: 'legacy-1',
          title: 'Alte Session',
          thread_id: 'thread-legacy-1',
          cwd: 'C:/workspace',
          total_messages: 2,
          started_at: '1970-01-01T00:00:01.000Z',
          ended_at: '1970-01-01T00:00:02.000Z',
        })
      }
      return Promise.resolve(null)
    })

    const session = await loadSession('legacy-1')
    expect(session).toMatchObject({
      id: 'legacy-1',
      title: 'Alte Session',
      threadId: 'thread-legacy-1',
    })
    expect(session?.messages).toEqual([])
  })
})
