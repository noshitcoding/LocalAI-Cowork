import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { RunRecord } from '../runtime/contracts'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import RemoteProviderProfileManager from './RemoteProviderProfileManager'
import RemoteOrganizationManager from './RemoteOrganizationManager'
import RemoteTaskManager from './RemoteTaskManager'

const projectId = '10000000-0000-4000-8000-000000000070'
const taskId = '10000000-0000-4000-8000-000000000071'
const project = {
  schema_version: 2, id: projectId, revision: 3, etag: 'W/"project:3"',
  owner_user_id: '10000000-0000-4000-8000-000000000072', team_id: null,
  privacy: 'team_managed', name: 'Project', description: '',
  preferred_executor_target: { kind: 'server_linux', pool_id: null },
  current_version_id: null, policy: {}, created_at: '2026-08-10T12:00:00Z',
  updated_at: '2026-08-10T12:00:00Z', deleted_at: null,
} as const

describe('remote task and provider management', () => {
  it('starts only the frozen released task revision', async () => {
    const run = { spec: { id: crypto.randomUUID() } } as RunRecord
    const createThreadMessage = vi.fn(async (_threadId: string, _request: unknown) => ({ message: {}, run }))
    const onRunCreated = vi.fn()
    const client = {
      listProjects: vi.fn(async () => [project]),
      capabilities: vi.fn(async () => ({ schema_version: 2, server_linux: [], executors: [] })),
      listProviderProfiles: vi.fn(async () => []),
      listTasks: vi.fn(async () => [{
        schema_version: 2, id: taskId, revision: 4, etag: 'W/"task:4"', project_id: projectId,
        name: 'Published task', instructions: 'Do the frozen work', required_capabilities: [],
        default_target: { kind: 'server_linux', pool_id: null }, config: {}, released: true,
        created_at: '2026-08-10T12:00:00Z', deleted_at: null,
      }]),
      createThread: vi.fn(async () => ({ id: '10000000-0000-4000-8000-000000000073' })),
      createThreadMessage,
    } as unknown as RemoteRuntimeClient
    render(<RemoteTaskManager compact client={client} onRunCreated={onRunCreated} />)
    fireEvent.click(screen.getByRole('button', { name: 'Tasks' }))
    fireEvent.click(await screen.findByRole('button', { name: 'Run Published task' }))
    fireEvent.click(screen.getByRole('button', { name: 'Start task' }))
    await waitFor(() => expect(createThreadMessage).toHaveBeenCalledTimes(1))
    expect(createThreadMessage.mock.calls[0]?.[1]).toMatchObject({
      run: {
        project_id: projectId,
        project_revision: 3,
        task: { id: taskId, revision: 4 },
        executor_target: { kind: 'server_linux', pool_id: null },
        input: {},
      },
    })
    await waitFor(() => expect(onRunCreated).toHaveBeenCalledWith(run))
  })

  it('creates a server-bound profile through the secret-safe API', async () => {
    const createProviderProfile = vi.fn(async (_request: unknown) => ({}))
    const client = {
      listProviderProfiles: vi.fn(async () => []),
      listTeams: vi.fn(async () => []),
      createProviderProfile,
    } as unknown as RemoteRuntimeClient
    render(<RemoteProviderProfileManager compact client={client} />)
    fireEvent.click(screen.getByRole('button', { name: 'Models' }))
    fireEvent.click(await screen.findByRole('button', { name: 'Add server profile' }))
    fireEvent.change(screen.getByRole('textbox', { name: 'Name' }), { target: { value: 'Server model' } })
    fireEvent.change(screen.getByLabelText('API key'), { target: { value: 'temporary-secret' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save profile' }))
    await waitFor(() => expect(createProviderProfile).toHaveBeenCalledTimes(1))
    expect(createProviderProfile.mock.calls[0]?.[0]).toMatchObject({
      team_id: null,
      name: 'Server model',
      provider_kind: 'openai_compatible',
      api_key: 'temporary-secret',
    })
    expect((createProviderProfile.mock.calls[0]?.[0] as { model_defaults: unknown }).model_defaults)
      .not.toHaveProperty('endpoint_binding')
  })

  it('creates a private local-first project from the remote clients', async () => {
    const createProject = vi.fn(async (_request: unknown) => project)
    const client = {
      listTeams: vi.fn(async () => []),
      listProjects: vi.fn(async () => []),
      createProject,
    } as unknown as RemoteRuntimeClient
    render(<RemoteOrganizationManager compact client={client} currentUserId={project.owner_user_id} />)
    fireEvent.click(screen.getByRole('button', { name: 'Projects' }))
    fireEvent.change(await screen.findByRole('textbox', { name: 'Project name' }), {
      target: { value: 'Offline research' },
    })
    fireEvent.change(screen.getByRole('textbox', { name: 'Description' }), {
      target: { value: 'Files remain on the personal device' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Create project' }))
    await waitFor(() => expect(createProject).toHaveBeenCalledWith({
      name: 'Offline research',
      description: 'Files remain on the personal device',
      privacy: 'private_local',
      team_id: null,
      preferred_executor_target: null,
      policy: { tool_policy: 'autonomous' },
    }))
  })
})
