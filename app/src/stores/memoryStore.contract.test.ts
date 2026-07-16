import { beforeEach, describe, expect, it, vi } from 'vitest'
import { safeInvoke } from '../utils/safeInvoke'
import { useMemoryStore, type MemoryEntry } from './memoryStore'

vi.mock('../utils/safeInvoke', () => ({
  hasTauriRuntime: vi.fn(() => true),
  safeInvoke: vi.fn(),
}))

const safeInvokeMock = vi.mocked(safeInvoke)

const entry: MemoryEntry = {
  id: 'agent-1',
  scope: 'agent',
  category: 'curated',
  key: 'stack',
  content: 'The project uses Rust.',
  source_session_id: null,
  confidence: 1,
  access_count: 0,
  last_accessed_at: '2026-07-16T10:00:00Z',
  created_at: '2026-07-16T10:00:00Z',
  updated_at: '2026-07-16T10:00:00Z',
}

describe('memory store backend contracts', () => {
  beforeEach(() => {
    safeInvokeMock.mockReset()
    useMemoryStore.setState({
      entries: [],
      searchResults: [],
      profileEntries: [],
      hints: [],
      lastSnapshot: null,
      loading: false,
      error: null,
    })
  })

  it('maps the backend frozen snapshot object into the UI snapshot shape', async () => {
    safeInvokeMock.mockResolvedValue({
      sessionId: 'snapshot-1',
      agentEntries: [entry],
      sharedEntries: [{ ...entry, id: 'shared-1', scope: 'shared', category: 'knowledge' }],
      userProfile: [{
        id: 'profile-1', key: 'style', value: 'Concise', source: 'test', confidence: 1,
        created_at: '2026-07-16T10:00:00Z', updated_at: '2026-07-16T10:00:00Z',
      }],
      createdAt: '2026-07-16T10:00:00Z',
    })

    const snapshot = await useMemoryStore.getState().createSnapshot()

    expect(safeInvokeMock).toHaveBeenCalledWith('memory_snapshot')
    expect(snapshot.total_entries).toBe(2)
    expect(snapshot.total_profile_keys).toBe(1)
    expect(snapshot.timestamp).toBe('2026-07-16T10:00:00Z')
  })

  it('maps deletedCount from compaction and reports the scoped remainder', async () => {
    useMemoryStore.setState({ entries: [entry, { ...entry, id: 'agent-2', key: 'other' }] })
    safeInvokeMock.mockResolvedValue({ scope: 'agent', deletedCount: 1 })

    await expect(useMemoryStore.getState().compactEntries('agent', 0.5)).resolves.toEqual({
      removed: 1,
      remaining: 1,
    })
  })

  it('requests all scopes when the memory panel has no scope filter', async () => {
    safeInvokeMock.mockResolvedValue([entry])

    await useMemoryStore.getState().loadEntries()

    expect(safeInvokeMock).toHaveBeenCalledWith('memory_search', {
      scope: null,
      category: null,
      keyword: null,
      limit: 200,
    }, expect.any(Array))
  })
})
