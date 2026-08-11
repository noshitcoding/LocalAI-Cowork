import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { RunRecord } from '../runtime/contracts'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import RemoteRunComposer from './RemoteRunComposer'

describe('RemoteRunComposer thread continuation', () => {
  it('creates a message/run pair in the existing thread without creating another thread', async () => {
    const threadId = '10000000-0000-4000-8000-000000000011'
    const projectId = '10000000-0000-4000-8000-000000000012'
    const run = {
      spec: { id: '10000000-0000-4000-8000-000000000013' },
    } as RunRecord
    const createThread = vi.fn()
    const createThreadMessage = vi.fn(async () => ({ message: {}, run }))
    const onCreated = vi.fn()
    const client = {
      listProjects: vi.fn(async () => [{
        schema_version: 2,
        id: projectId,
        revision: 4,
        etag: 'W/"project:4"',
        owner_user_id: '10000000-0000-4000-8000-000000000014',
        team_id: null,
        privacy: 'team_managed',
        name: 'Durable project',
        description: '',
        preferred_executor_target: { kind: 'server_linux', pool_id: null },
        current_version_id: null,
        policy: {},
        created_at: '2026-08-10T12:00:00Z',
        updated_at: '2026-08-10T12:00:00Z',
        deleted_at: null,
      }]),
      capabilities: vi.fn(async () => ({ schema_version: 2, server_linux: [], executors: [] })),
      listProviderProfiles: vi.fn(async () => []),
      createThread,
      createThreadMessage,
    } as unknown as RemoteRuntimeClient

    render(
      <RemoteRunComposer
        compact
        client={client}
        threadId={threadId}
        threadProjectId={projectId}
        initialTarget={{ kind: 'server_linux', pool_id: null }}
        onCreated={onCreated}
      />,
    )
    fireEvent.click(screen.getByRole('button', { name: 'Continue thread' }))
    const project = await screen.findByRole('combobox', { name: 'Project' })
    expect(project).toBeDisabled()
    fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), {
      target: { value: 'Continue the analysis' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Start run' }))

    await waitFor(() => expect(createThreadMessage).toHaveBeenCalledTimes(1))
    expect(createThread).not.toHaveBeenCalled()
    expect(createThreadMessage).toHaveBeenCalledWith(threadId, {
      content: { text: 'Continue the analysis' },
      run: expect.objectContaining({
        thread_id: threadId,
        project_id: projectId,
        project_revision: 4,
        executor_target: { kind: 'server_linux', pool_id: null },
        input: { prompt: 'Continue the analysis' },
      }),
    })
    await waitFor(() => expect(onCreated).toHaveBeenCalledWith(run))
  })
})
