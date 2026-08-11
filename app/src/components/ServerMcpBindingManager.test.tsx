import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import ServerMcpBindingManager from './ServerMcpBindingManager'

const projectId = '10000000-0000-4000-8000-000000000201'
const entityId = '10000000-0000-4000-8000-000000000202'

describe('ServerMcpBindingManager', () => {
  it('sends the complete secret once and keeps it out of returned metadata', async () => {
    const setServerMcpBinding = vi.fn(async () => ({
      schema_version: 2, project_id: projectId, mcp_entity_id: entityId,
      revision: 1, etag: 'W/"binding:1"', name: 'Project docs', transport: 'stdio',
      executable_hint: 'docs-mcp', argument_count: 1, environment_keys: ['MCP_TOKEN'],
      created_at: '2026-08-11T12:00:00Z', updated_at: '2026-08-11T12:00:00Z',
    }))
    const client = {
      listProjects: vi.fn(async () => [{ id: projectId, name: 'Docs project' }]),
      listSyncedEntities: vi.fn(async () => ({
        schema_version: 2,
        items: [{
          schema_version: 2, entity_type: 'mcp_metadata', entity_id: entityId,
          revision: 4, etag: 'W/"mcp:4"', payload: { name: 'Project docs' },
          tombstone: false, updated_at: '2026-08-11T12:00:00Z',
        }],
        next_after: null, watermark_cursor: 4,
      })),
      listServerMcpBindings: vi.fn(async () => []),
      setServerMcpBinding,
    } as unknown as RemoteRuntimeClient

    render(<ServerMcpBindingManager compact client={client} />)
    fireEvent.click(screen.getByRole('button', { name: 'Server MCP' }))
    await screen.findByRole('option', { name: 'Project docs (r4)' })
    fireEvent.change(screen.getByLabelText('MCP binding metadata'), { target: { value: entityId } })
    fireEvent.change(screen.getByLabelText('MCP sandbox command'), { target: { value: '/opt/mcp/docs-mcp' } })
    fireEvent.change(screen.getByLabelText('MCP arguments JSON'), { target: { value: '["--stdio"]' } })
    fireEvent.change(screen.getByLabelText('MCP environment JSON'), { target: { value: '{"MCP_TOKEN":"one-time-secret"}' } })
    fireEvent.click(screen.getByRole('button', { name: 'Create encrypted binding' }))

    await waitFor(() => expect(setServerMcpBinding).toHaveBeenCalledWith(
      projectId,
      entityId,
      {
        expected_revision: null,
        name: 'Project docs',
        transport: 'stdio',
        command: '/opt/mcp/docs-mcp',
        args: ['--stdio'],
        environment: { MCP_TOKEN: 'one-time-secret' },
        url: '',
        headers: {},
      },
    ))
    await waitFor(() => expect(screen.getByLabelText('MCP environment JSON')).toHaveValue('{}'))
  })

  it('submits a streamable HTTP endpoint and credential headers without stdio fields', async () => {
    const setServerMcpBinding = vi.fn(async () => ({
      schema_version: 2, project_id: projectId, mcp_entity_id: entityId,
      revision: 1, etag: 'W/"binding:1"', name: 'Project docs', transport: 'streamable_http',
      executable_hint: 'HTTPS endpoint', argument_count: 0, environment_keys: ['Authorization'],
      created_at: '2026-08-11T12:00:00Z', updated_at: '2026-08-11T12:00:00Z',
    }))
    const client = {
      listProjects: vi.fn(async () => [{ id: projectId, name: 'Docs project' }]),
      listSyncedEntities: vi.fn(async () => ({
        schema_version: 2,
        items: [{
          schema_version: 2, entity_type: 'mcp_metadata', entity_id: entityId,
          revision: 4, etag: 'W/"mcp:4"', payload: { name: 'Project docs' },
          tombstone: false, updated_at: '2026-08-11T12:00:00Z',
        }],
        next_after: null, watermark_cursor: 4,
      })),
      listServerMcpBindings: vi.fn(async () => []),
      setServerMcpBinding,
    } as unknown as RemoteRuntimeClient

    render(<ServerMcpBindingManager compact client={client} />)
    fireEvent.click(screen.getByRole('button', { name: 'Server MCP' }))
    await screen.findByRole('option', { name: 'Project docs (r4)' })
    fireEvent.change(screen.getByLabelText('MCP binding metadata'), { target: { value: entityId } })
    fireEvent.change(screen.getByLabelText('MCP arguments JSON'), { target: { value: '{hidden invalid stdio json' } })
    fireEvent.change(screen.getByLabelText('MCP binding transport'), { target: { value: 'streamable_http' } })
    fireEvent.change(screen.getByLabelText('MCP HTTPS endpoint'), { target: { value: 'https://mcp.example.com/mcp' } })
    fireEvent.change(screen.getByLabelText('MCP credential headers JSON'), { target: { value: '{"Authorization":"Bearer one-time-secret"}' } })
    fireEvent.click(screen.getByRole('button', { name: 'Create encrypted binding' }))

    await waitFor(() => expect(setServerMcpBinding).toHaveBeenCalledWith(
      projectId,
      entityId,
      {
        expected_revision: null,
        name: 'Project docs',
        transport: 'streamable_http',
        command: '',
        args: [],
        environment: {},
        url: 'https://mcp.example.com/mcp',
        headers: { Authorization: 'Bearer one-time-secret' },
      },
    ))
    await waitFor(() => expect(screen.getByLabelText('MCP binding metadata')).toHaveValue(''))
  })
})
