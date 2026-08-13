import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { LocalDaemonRuntimeClient } from '../runtime/localDaemonClient'
import LocalSyncConflicts from './LocalSyncConflicts'

const client = {
  health: vi.fn(),
  syncState: vi.fn(),
  listSyncConflicts: vi.fn(),
  resolveSyncConflict: vi.fn(),
} as unknown as LocalDaemonRuntimeClient

vi.mock('../runtime/localDaemonExecution', () => ({
  createLocalDaemonRuntimeClient: () => client,
}))
vi.mock('../runtime/localDaemonEntities', () => ({
  reconcileDurableLocalEntities: vi.fn(async () => undefined),
}))

describe('LocalSyncConflicts', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    Object.defineProperty(window, '__TAURI_INTERNALS__', {
      configurable: true,
      value: {},
    })
    vi.mocked(client.health).mockResolvedValue({
      status: 'ok',
      schema_version: 2,
      device_id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
      daemon_version: 'test',
    })
    vi.mocked(client.syncState).mockResolvedValue({
      peer_id: 'https://cowork.example.test#aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
      local_cursor: 4,
      remote_cursor: 7,
      open_conflicts: 1,
    })
    vi.mocked(client.listSyncConflicts).mockResolvedValue([{
      id: 'conflict-1',
      peer_id: 'https://cowork.example.test#aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
      entity_type: 'project',
      entity_id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
      local_entity: null,
      remote_entity: {
        entity_type: 'project',
        entity_id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
        revision: 2,
        payload: { title: 'Server project' },
        tombstone: false,
        updated_at: '2026-08-10T00:00:00Z',
      },
      created_at: '2026-08-10T00:00:00Z',
      resolved_at: null,
      resolution: null,
    }])
    vi.mocked(client.resolveSyncConflict).mockResolvedValue({
      id: 'conflict-1',
      resolution: 'use_remote',
      resolved_at: '2026-08-10T00:01:00Z',
    })
  })

  it('shows durable cursors and resolves a conflict with the server version', async () => {
    render(<LocalSyncConflicts serverUrl="https://cowork.example.test" />)

    await waitFor(() => expect(client.listSyncConflicts).toHaveBeenCalled())
    fireEvent.click(screen.getByRole('button', { name: 'Open local sync status' }))
    expect(screen.getByText('1 conflict needs a decision')).toBeInTheDocument()
    expect(screen.getByText(/Uploaded cursor 4; downloaded cursor 7/)).toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: 'Use server version' }))
    await waitFor(() => expect(client.resolveSyncConflict).toHaveBeenCalledWith(
      'https://cowork.example.test#aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
      'conflict-1',
      'use_remote',
    ))
  })
})
