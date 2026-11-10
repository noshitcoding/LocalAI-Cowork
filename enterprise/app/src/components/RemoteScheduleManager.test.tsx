import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import RemoteScheduleManager from './RemoteScheduleManager'

describe('RemoteScheduleManager', () => {
  it('uses one atomic request and reuses its idempotency key after a failure', async () => {
    const projectId = '40000000-0000-4000-8000-000000000001'
    const taskId = '40000000-0000-4000-8000-000000000002'
    const createThreadSchedule = vi.fn(async (_request: { idempotency_key: string }) => { throw new Error('schedule rejected') })
    const client = {
      listProjects: vi.fn(async () => [{
        schema_version: 2,
        id: projectId,
        revision: 1,
        etag: 'W/"project:1"',
        owner_user_id: '40000000-0000-4000-8000-000000000004',
        team_id: null,
        privacy: 'private_local',
        name: 'Schedule project',
        description: '',
        preferred_executor_target: null,
        current_version_id: null,
        policy: {},
        created_at: '2026-08-11T12:00:00Z',
        updated_at: '2026-08-11T12:00:00Z',
        deleted_at: null,
      }]),
      capabilities: vi.fn(async () => ({ schema_version: 2, server_linux: [], executors: [] })),
      listProviderProfiles: vi.fn(async () => []),
      listTasks: vi.fn(async () => [{
        schema_version: 2,
        id: taskId,
        revision: 2,
        etag: 'W/"task:2"',
        project_id: projectId,
        name: 'Daily report',
        instructions: 'Create a report',
        required_capabilities: [],
        default_target: { kind: 'server_linux', pool_id: null },
        config: {},
        released: true,
        created_at: '2026-08-11T12:00:00Z',
        deleted_at: null,
      }]),
      listSchedules: vi.fn(async () => []),
      createThreadSchedule,
    } as unknown as RemoteRuntimeClient

    render(<RemoteScheduleManager compact client={client} />)
    fireEvent.click(screen.getByRole('button', { name: 'Schedules' }))
    await waitFor(() => expect(screen.getByRole('button', { name: 'Add schedule' })).toBeEnabled())
    fireEvent.click(screen.getByRole('button', { name: 'Add schedule' }))
    fireEvent.click(screen.getByRole('button', { name: 'Create schedule' }))

    await waitFor(() => expect(createThreadSchedule).toHaveBeenCalledTimes(1))
    expect(await screen.findByRole('alert')).toHaveTextContent('schedule rejected')
    fireEvent.click(screen.getByRole('button', { name: 'Create schedule' }))
    await waitFor(() => expect(createThreadSchedule).toHaveBeenCalledTimes(2))
    expect(createThreadSchedule.mock.calls[1]?.[0].idempotency_key)
      .toBe(createThreadSchedule.mock.calls[0]?.[0].idempotency_key)
  })
})
