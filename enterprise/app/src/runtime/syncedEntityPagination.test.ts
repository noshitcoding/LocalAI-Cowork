import { describe, expect, it, vi } from 'vitest'

import type { RemoteRuntimeClient } from './runtimeClient'
import { listAllSyncedEntities } from './syncedEntityPagination'

describe('listAllSyncedEntities', () => {
  it('loads every page in order', async () => {
    const cursor = crypto.randomUUID()
    const listSyncedEntities = vi.fn()
      .mockResolvedValueOnce({ items: [{ entity_id: 'one' }], next_after: cursor })
      .mockResolvedValueOnce({ items: [{ entity_id: 'two' }], next_after: null })
    const client = { listSyncedEntities } as unknown as RemoteRuntimeClient

    await expect(listAllSyncedEntities(client, 'memory')).resolves.toEqual([
      { entity_id: 'one' },
      { entity_id: 'two' },
    ])
    expect(listSyncedEntities).toHaveBeenNthCalledWith(1, 'memory', null, 500)
    expect(listSyncedEntities).toHaveBeenNthCalledWith(2, 'memory', cursor, 500)
  })

  it('stops after a repeated cursor', async () => {
    const cursor = crypto.randomUUID()
    const listSyncedEntities = vi.fn().mockResolvedValue({ items: [], next_after: cursor })
    const client = { listSyncedEntities } as unknown as RemoteRuntimeClient

    await expect(listAllSyncedEntities(client, 'mcp_metadata')).rejects.toThrow(
      'Server returned a repeated metadata cursor',
    )
    expect(listSyncedEntities).toHaveBeenCalledTimes(2)
  })
})
