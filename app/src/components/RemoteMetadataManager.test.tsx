import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { SyncedEntity } from '../runtime/contracts'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import RemoteMetadataManager from './RemoteMetadataManager'

const deviceId = '10000000-0000-4000-8000-000000000101'
const entityId = '10000000-0000-4000-8000-000000000102'
const agentId = '10000000-0000-4000-8000-000000000103'
const taskId = '10000000-0000-4000-8000-000000000104'
function crewDefinition(name: string, extra: Record<string, unknown> = {}) {
  return {
    id: entityId,
    name,
    agents: [{ id: agentId, name: 'Researcher', enabled: true }],
    tasks: [{ id: taskId, agentId, description: 'Research' }],
    ...extra,
  }
}
const entity: SyncedEntity = {
  schema_version: 2,
  entity_type: 'crew',
  entity_id: entityId,
  revision: 3,
  etag: `W/"${entityId}:3"`,
  payload: { definition: crewDefinition('Research crew') },
  tombstone: false,
  updated_at: '2026-08-11T12:00:00Z',
}

describe('RemoteMetadataManager', () => {
  beforeEach(() => {
    window.localStorage.clear()
    window.localStorage.setItem('open-cowork-remote-device-v1', deviceId)
  })

  it('writes an optimistic revision with the authenticated device identity', async () => {
    const pushSyncChanges = vi.fn(async (changes: unknown[]) => ({
      schema_version: 2,
      results: [{ schema_version: 2, operation_id: (changes[0] as { operation_id: string }).operation_id, status: 'applied', entity: { ...entity, revision: 4 } }],
    }))
    const client = {
      listSyncedEntities: vi.fn(async () => ({
        schema_version: 2, items: [entity], next_after: null, watermark_cursor: 3,
      })),
      pushSyncChanges,
    } as unknown as RemoteRuntimeClient

    render(<RemoteMetadataManager compact client={client} />)
    fireEvent.click(screen.getByRole('button', { name: 'Metadata' }))
    fireEvent.click(await screen.findByRole('button', { name: 'Edit Research crew' }))
    fireEvent.click(screen.getByRole('button', { name: 'Advanced JSON' }))
    fireEvent.change(screen.getByLabelText('Metadata JSON'), {
      target: { value: JSON.stringify({ definition: crewDefinition('Reviewed crew') }) },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }))

    await waitFor(() => expect(pushSyncChanges).toHaveBeenCalledTimes(1))
    expect(pushSyncChanges.mock.calls[0]?.[0]).toMatchObject([{
      schema_version: 2,
      device_id: deviceId,
      entity_type: 'crew',
      entity_id: entityId,
      base_revision: 3,
      operation: 'upsert',
      payload: { definition: { id: entityId, name: 'Reviewed crew' } },
    }])
  })

  it('rejects credential fields before synchronized metadata leaves the client', async () => {
    const pushSyncChanges = vi.fn()
    const client = {
      listSyncedEntities: vi.fn(async () => ({
        schema_version: 2, items: [entity], next_after: null, watermark_cursor: 3,
      })),
      pushSyncChanges,
    } as unknown as RemoteRuntimeClient

    render(<RemoteMetadataManager compact client={client} />)
    fireEvent.click(screen.getByRole('button', { name: 'Metadata' }))
    fireEvent.click(await screen.findByRole('button', { name: 'Edit Research crew' }))
    fireEvent.click(screen.getByRole('button', { name: 'Advanced JSON' }))
    fireEvent.change(screen.getByLabelText('Metadata JSON'), {
      target: { value: JSON.stringify({ definition: crewDefinition('Unsafe', { api_key: 'secret' }) }) },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }))

    expect(await screen.findByRole('alert')).toHaveTextContent('encrypted profile/device binding flow')
    expect(pushSyncChanges).not.toHaveBeenCalled()
  })

  it('loads the current server revision instead of overwriting a conflict', async () => {
    const latest = {
      ...entity,
      revision: 4,
      payload: { definition: crewDefinition('Latest server crew') },
    }
    const client = {
      listSyncedEntities: vi.fn(async () => ({
        schema_version: 2, items: [entity], next_after: null, watermark_cursor: 3,
      })),
      pushSyncChanges: vi.fn(async (changes: unknown[]) => ({
        schema_version: 2,
        results: [{
          schema_version: 2,
          operation_id: (changes[0] as { operation_id: string }).operation_id,
          status: 'conflict',
          entity: latest,
        }],
      })),
    } as unknown as RemoteRuntimeClient

    render(<RemoteMetadataManager compact client={client} />)
    fireEvent.click(screen.getByRole('button', { name: 'Metadata' }))
    fireEvent.click(await screen.findByRole('button', { name: 'Edit Research crew' }))
    fireEvent.click(screen.getByRole('button', { name: 'Advanced JSON' }))
    fireEvent.change(screen.getByLabelText('Metadata JSON'), {
      target: { value: JSON.stringify({ definition: crewDefinition('Stale edit') }) },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }))

    expect(await screen.findByRole('alert')).toHaveTextContent('latest server revision was loaded')
    expect(screen.getByLabelText('Metadata JSON')).toHaveValue(JSON.stringify(latest.payload, null, 2))
    expect(screen.getByDisplayValue('4')).toBeInTheDocument()
  })

  it('requires a second explicit action before creating a tombstone', async () => {
    const pushSyncChanges = vi.fn(async (changes: unknown[]) => ({
      schema_version: 2,
      results: [{ schema_version: 2, operation_id: (changes[0] as { operation_id: string }).operation_id, status: 'applied', entity: { ...entity, revision: 4, tombstone: true, payload: null } }],
    }))
    const listSyncedEntities = vi.fn()
      .mockResolvedValueOnce({ schema_version: 2, items: [entity], next_after: null, watermark_cursor: 3 })
      .mockResolvedValue({ schema_version: 2, items: [], next_after: null, watermark_cursor: 4 })
    const client = { listSyncedEntities, pushSyncChanges } as unknown as RemoteRuntimeClient

    render(<RemoteMetadataManager compact client={client} />)
    fireEvent.click(screen.getByRole('button', { name: 'Metadata' }))
    const firstDelete = await screen.findByRole('button', { name: 'Delete Research crew' })
    fireEvent.click(firstDelete)
    expect(pushSyncChanges).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Confirm delete Research crew' }))

    await waitFor(() => expect(pushSyncChanges).toHaveBeenCalledTimes(1))
    expect(pushSyncChanges.mock.calls[0]?.[0]).toMatchObject([{
      entity_type: 'crew', entity_id: entityId, base_revision: 3,
      operation: 'delete', payload: null,
    }])
  })

  it('creates a skill through the guided editor without requiring JSON', async () => {
    const pushSyncChanges = vi.fn(async (changes: unknown[]) => ({
      schema_version: 2,
      results: [{ schema_version: 2, operation_id: (changes[0] as { operation_id: string }).operation_id, status: 'applied', entity }],
    }))
    const client = {
      listSyncedEntities: vi.fn(async () => ({
        schema_version: 2, items: [], next_after: null, watermark_cursor: 0,
      })),
      pushSyncChanges,
    } as unknown as RemoteRuntimeClient

    render(<RemoteMetadataManager compact client={client} />)
    fireEvent.click(screen.getByRole('button', { name: 'Metadata' }))
    fireEvent.change(screen.getByLabelText('Metadata type'), { target: { value: 'skill' } })
    fireEvent.click(await screen.findByRole('button', { name: 'Add skill' }))
    expect(screen.queryByLabelText('Metadata JSON')).not.toBeInTheDocument()
    fireEvent.change(screen.getByLabelText('Skill name'), { target: { value: 'Release checker' } })
    fireEvent.change(screen.getByLabelText('Skill description'), { target: { value: 'Checks release readiness.' } })
    fireEvent.change(screen.getByLabelText('Skill prompt template'), { target: { value: 'Review {{input}} before release.' } })
    fireEvent.change(screen.getByLabelText('Skill trigger pattern'), { target: { value: 'release' } })
    fireEvent.click(screen.getByRole('button', { name: 'Create skill' }))

    await waitFor(() => expect(pushSyncChanges).toHaveBeenCalledTimes(1))
    expect(pushSyncChanges.mock.calls[0]?.[0]).toMatchObject([{
      entity_type: 'skill', base_revision: 0, operation: 'upsert',
      payload: {
        name: 'Release checker', description: 'Checks release readiness.',
        prompt_template: 'Review {{input}} before release.', trigger_pattern: 'release',
        run_mode: 'execute', auto_generated: false,
      },
    }])
  })

  it('preserves advanced crew agents and tasks when guided fields change', async () => {
    const pushSyncChanges = vi.fn(async (changes: unknown[]) => ({
      schema_version: 2,
      results: [{ schema_version: 2, operation_id: (changes[0] as { operation_id: string }).operation_id, status: 'applied', entity }],
    }))
    const client = {
      listSyncedEntities: vi.fn(async () => ({
        schema_version: 2, items: [entity], next_after: null, watermark_cursor: 3,
      })),
      pushSyncChanges,
    } as unknown as RemoteRuntimeClient

    render(<RemoteMetadataManager compact client={client} />)
    fireEvent.click(screen.getByRole('button', { name: 'Metadata' }))
    fireEvent.click(await screen.findByRole('button', { name: 'Edit Research crew' }))
    fireEvent.change(screen.getByLabelText('Crew name'), { target: { value: 'Guided research crew' } })
    fireEvent.change(screen.getByLabelText('Crew execution guidelines'), { target: { value: 'Cite every source.' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }))

    await waitFor(() => expect(pushSyncChanges).toHaveBeenCalledTimes(1))
    expect(pushSyncChanges.mock.calls[0]?.[0]).toMatchObject([{
      payload: { definition: {
        id: entityId, name: 'Guided research crew', executionGuidelines: 'Cite every source.',
        agents: [{ id: agentId }], tasks: [{ id: taskId, agentId }],
      } },
    }])
  })

  it('does not leave advanced JSON mode while the document is invalid', async () => {
    const client = {
      listSyncedEntities: vi.fn(async () => ({
        schema_version: 2, items: [entity], next_after: null, watermark_cursor: 3,
      })),
    } as unknown as RemoteRuntimeClient

    render(<RemoteMetadataManager compact client={client} />)
    fireEvent.click(screen.getByRole('button', { name: 'Metadata' }))
    fireEvent.click(await screen.findByRole('button', { name: 'Edit Research crew' }))
    fireEvent.click(screen.getByRole('button', { name: 'Advanced JSON' }))
    fireEvent.change(screen.getByLabelText('Metadata JSON'), { target: { value: '{invalid' } })
    fireEvent.click(screen.getByRole('button', { name: 'Guided' }))

    expect(await screen.findByRole('alert')).toHaveTextContent('Fix the advanced JSON')
    expect(screen.getByLabelText('Metadata JSON')).toHaveValue('{invalid')
    expect(screen.getByRole('button', { name: 'Advanced JSON' })).toHaveAttribute('aria-pressed', 'true')
  })
})
