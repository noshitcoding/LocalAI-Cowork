import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { RunRecord } from '../runtime/contracts'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import RemoteRunComposer from './RemoteRunComposer'

describe('RemoteRunComposer thread continuation', () => {
  it('creates a message/run pair in the existing thread without creating another thread', async () => {
    const threadId = '10000000-0000-4000-8000-000000000011'
    const projectId = '10000000-0000-4000-8000-000000000012'
    const skillId = '10000000-0000-4000-8000-000000000015'
    const memoryId = '10000000-0000-4000-8000-000000000016'
    const mcpId = '10000000-0000-4000-8000-000000000017'
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
      capabilities: vi.fn(async () => ({
        schema_version: 2,
        server_linux: [{
          name: 'tool.mcp.invoke', available: true, interactive: false,
          supports_offline: false, constraints: {},
        }],
        executors: [],
      })),
      listProviderProfiles: vi.fn(async () => []),
      listSyncedEntities: vi.fn(async (entityType: string) => ({
        schema_version: 2,
        items: entityType === 'skill' ? [{
          schema_version: 2, entity_type: 'skill', entity_id: skillId, revision: 2,
          etag: `W/"${skillId}:2"`, payload: { name: 'Evidence review' }, tombstone: false,
          updated_at: '2026-08-10T12:00:00Z',
        }] : entityType === 'memory' ? [{
          schema_version: 2, entity_type: 'memory', entity_id: memoryId, revision: 5,
          etag: `W/"${memoryId}:5"`, payload: { key: 'tone' }, tombstone: false,
          updated_at: '2026-08-10T12:00:00Z',
        }] : [{
          schema_version: 2, entity_type: 'mcp_metadata', entity_id: mcpId, revision: 3,
          etag: `W/"${mcpId}:3"`, payload: { name: 'Project docs' }, tombstone: false,
          updated_at: '2026-08-10T12:00:00Z',
        }],
        next_after: null,
        watermark_cursor: 5,
      })),
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
    const skillOption = await screen.findByRole('option', { name: 'Evidence review (r2)' }) as HTMLOptionElement
    skillOption.selected = true
    fireEvent.change(screen.getByRole('listbox', { name: 'Frozen skills' }))
    const memoryOption = screen.getByRole('option', { name: 'tone (r5)' }) as HTMLOptionElement
    memoryOption.selected = true
    fireEvent.change(screen.getByRole('listbox', { name: 'Frozen memory' }))
    const mcpOption = screen.getByRole('option', { name: 'Project docs (r3)' }) as HTMLOptionElement
    mcpOption.selected = true
    fireEvent.change(screen.getByRole('listbox', { name: 'Executor-bound MCP' }))
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
        input: {
          prompt: 'Continue the analysis',
          skill_ids: [skillId],
          memory_ids: [memoryId],
          mcp_metadata_ids: [mcpId],
        },
      }),
    })
    await waitFor(() => expect(onCreated).toHaveBeenCalledWith(run))
  })

  it('removes Windows pools that do not bind every selected MCP server', async () => {
    const projectId = '20000000-0000-4000-8000-000000000001'
    const mcpId = '20000000-0000-4000-8000-000000000002'
    const poolId = '20000000-0000-4000-8000-000000000003'
    const client = {
      listProjects: vi.fn(async () => [{
        schema_version: 2,
        id: projectId,
        revision: 1,
        etag: 'W/"project:1"',
        owner_user_id: '20000000-0000-4000-8000-000000000004',
        team_id: null,
        privacy: 'private_local',
        name: 'Private project',
        description: '',
        preferred_executor_target: null,
        current_version_id: null,
        policy: {},
        created_at: '2026-08-11T12:00:00Z',
        updated_at: '2026-08-11T12:00:00Z',
        deleted_at: null,
      }]),
      capabilities: vi.fn(async () => ({
        schema_version: 2,
        server_linux: [{
          schema_version: 2, name: 'tool.mcp.invoke', version: 'test', attributes: {},
        }],
        executors: [{
          registration: {
            schema_version: 2,
            executor_id: '20000000-0000-4000-8000-000000000005',
            kind: 'managed_windows',
            pool_id: poolId,
            owner_user_id: null,
            display_name: 'Windows Office',
            protocol_version: 2,
            capabilities: [{
              schema_version: 2,
              name: 'tool.mcp.invoke',
              version: 'test',
              attributes: { server_names: ['CRM'] },
            }],
            labels: {},
            max_concurrent_runs: 1,
          },
          online: true,
          draining: false,
          active_runs: 0,
          last_seen_at: '2026-08-11T12:00:00Z',
        }],
      })),
      listProviderProfiles: vi.fn(async () => []),
      listSyncedEntities: vi.fn(async (entityType: string) => ({
        schema_version: 2,
        items: entityType === 'mcp_metadata' ? [{
          schema_version: 2,
          entity_type: 'mcp_metadata',
          entity_id: mcpId,
          revision: 1,
          etag: `W/"${mcpId}:1"`,
          payload: { name: 'Project docs' },
          tombstone: false,
          updated_at: '2026-08-11T12:00:00Z',
        }] : [],
        next_after: null,
        watermark_cursor: 1,
      })),
    } as unknown as RemoteRuntimeClient

    render(<RemoteRunComposer client={client} onCreated={vi.fn()} />)
    fireEvent.click(screen.getByRole('button', { name: 'New run' }))
    expect(await screen.findByRole('option', { name: 'Windows pool · Windows Office' }))
      .toBeInTheDocument()

    const mcpOption = screen.getByRole('option', { name: 'Project docs (r1)' }) as HTMLOptionElement
    mcpOption.selected = true
    fireEvent.change(screen.getByRole('listbox', { name: 'Executor-bound MCP' }))

    await waitFor(() => expect(
      screen.queryByRole('option', { name: 'Windows pool · Windows Office' }),
    ).not.toBeInTheDocument())
    expect(screen.getByRole('option', { name: 'Linux server' })).toBeInTheDocument()
  })
})
