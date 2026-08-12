import { describe, expect, it, vi } from 'vitest'

import type { LocalDaemonRuntimeClient } from './localDaemonClient'
import { localDaemonSyncPeerId, readLocalDaemonSyncSnapshot } from './localDaemonSync'

describe('local daemon sync snapshot', () => {
  it('uses the same canonical peer identity as the device agent', () => {
    expect(localDaemonSyncPeerId(
      ' https://cowork.example.test/// ',
      'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
    )).toBe('https://cowork.example.test#aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa')
  })

  it('does not request conflict details when the state is clean', async () => {
    const listSyncConflicts = vi.fn()
    const client = {
      health: vi.fn(async () => ({
        status: 'ok',
        schema_version: 2,
        device_id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
        daemon_version: 'test',
      })),
      syncState: vi.fn(async (peerId: string) => ({
        peer_id: peerId,
        local_cursor: 12,
        remote_cursor: 18,
        open_conflicts: 0,
      })),
      listSyncConflicts,
    } as unknown as LocalDaemonRuntimeClient

    const snapshot = await readLocalDaemonSyncSnapshot(client, 'https://cowork.example.test/')

    expect(snapshot.peerId).toBe('https://cowork.example.test#aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa')
    expect(snapshot.state.remote_cursor).toBe(18)
    expect(snapshot.conflicts).toEqual([])
    expect(listSyncConflicts).not.toHaveBeenCalled()
  })

  it('loads open conflict records for review', async () => {
    const conflict = {
      id: 'conflict-1',
      peer_id: 'peer',
      entity_type: 'project',
      entity_id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
      local_entity: null,
      remote_entity: {
        entity_type: 'project',
        entity_id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
        revision: 2,
        payload: { title: 'Server' },
        tombstone: false,
        updated_at: '2026-08-10T00:00:00Z',
      },
      created_at: '2026-08-10T00:00:00Z',
      resolved_at: null,
      resolution: null,
    }
    const client = {
      health: vi.fn(async () => ({
        status: 'ok',
        schema_version: 2,
        device_id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
        daemon_version: 'test',
      })),
      syncState: vi.fn(async (peerId: string) => ({
        peer_id: peerId,
        local_cursor: 12,
        remote_cursor: 18,
        open_conflicts: 1,
      })),
      listSyncConflicts: vi.fn(async () => [conflict]),
    } as unknown as LocalDaemonRuntimeClient

    const snapshot = await readLocalDaemonSyncSnapshot(client, 'https://cowork.example.test')

    expect(snapshot.conflicts).toEqual([conflict])
    expect(client.listSyncConflicts).toHaveBeenCalledWith(snapshot.peerId)
  })
})
