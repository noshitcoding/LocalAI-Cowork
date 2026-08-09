import { beforeEach, describe, expect, it, vi } from 'vitest'

type MockEntity = {
  entity_type: string
  id: string
  revision: number
  etag: string
  payload: Record<string, unknown>
  tombstone: boolean
  created_at: string
  updated_at: string
}

let entity: MockEntity | null = null
const operationOrder: string[] = []

const invokeMock = vi.fn(async (command: string, args?: unknown): Promise<unknown> => {
  if (command !== 'local_daemon_call') throw new Error(`Unexpected command ${command}`)
  const call = args as { method: string; params?: Record<string, unknown> }
  if (call.method === 'health') {
    operationOrder.push('health')
    return { status: 'ok', schema_version: 2, device_id: 'device-test', daemon_version: 'test' }
  }
  if (call.method === 'entities.list') {
    operationOrder.push('list')
    await Promise.resolve()
    return entity ? [{ ...entity }] : []
  }
  if (call.method === 'entities.upsert') {
    const expectedRevision = call.params?.expected_revision as number
    const currentRevision = entity?.revision ?? 0
    if (expectedRevision !== currentRevision) throw new Error('revision conflict')
    const revision = currentRevision + 1
    operationOrder.push(`upsert:${revision}`)
    entity = {
      entity_type: String(call.params?.entity_type),
      id: String(call.params?.id),
      revision,
      etag: `W/"message:test:${revision}"`,
      payload: call.params?.payload as Record<string, unknown>,
      tombstone: false,
      created_at: '2026-08-09T00:00:00Z',
      updated_at: '2026-08-09T00:00:00Z',
    }
    return { ...entity }
  }
  if (call.method === 'entities.delete') {
    const expectedRevision = call.params?.expected_revision as number
    if (!entity || expectedRevision !== entity.revision) throw new Error('revision conflict')
    operationOrder.push(`delete:${entity.revision + 1}`)
    entity = { ...entity, revision: entity.revision + 1, tombstone: true }
    return { ...entity }
  }
  if (call.method === 'mcp_bindings.upsert') {
    operationOrder.push('mcp-binding')
    return {
      server_id: call.params?.server_id,
      bound: true,
      name: call.params?.name,
      executable_hint: call.params?.command,
      argument_count: Array.isArray(call.params?.args) ? call.params.args.length : 0,
      environment_keys: Object.keys((call.params?.env ?? {}) as Record<string, string>),
    }
  }
  throw new Error(`Unexpected daemon method ${call.method}`)
})

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (command: string, args?: unknown) => invokeMock(command, args),
  convertFileSrc: (path: string) => path,
}))

import {
  mirrorDurableLocalEntity,
  mirrorMcpDeviceBinding,
  tombstoneDurableLocalEntity,
} from './localDaemonEntities'

describe('durable local entity write queue', () => {
  beforeEach(() => {
    entity = null
    operationOrder.length = 0
    invokeMock.mockClear()
  })

  it('serializes rapid writes and a following tombstone per entity', async () => {
    const first = mirrorDurableLocalEntity('message', 'message-test', { content: '' })
    const final = mirrorDurableLocalEntity('message', 'message-test', { content: 'final' })
    const deleted = tombstoneDurableLocalEntity('message', 'message-test')

    const [firstResult, finalResult, deletedResult] = await Promise.all([first, final, deleted])

    expect(firstResult.revision).toBe(1)
    expect(finalResult).toMatchObject({ revision: 2, payload: { content: 'final' } })
    expect(deletedResult).toMatchObject({ revision: 3, tombstone: true })
    expect(operationOrder).toEqual([
      'health', 'list', 'upsert:1',
      'health', 'list', 'upsert:2',
      'health', 'list', 'delete:3',
    ])
  })

  it('mirrors the complete MCP binding only to the encrypted device-binding RPC', async () => {
    await mirrorMcpDeviceBinding({
      id: 'mcp-docs',
      name: 'docs',
      command: 'C:\\Tools\\docs-mcp.exe',
      args: '--stdio "C:\\Work Files"',
      env: { MCP_TOKEN: 'device-secret' },
    })

    expect(invokeMock).toHaveBeenCalledWith('local_daemon_call', {
      method: 'mcp_bindings.upsert',
      params: {
        server_id: 'mcp-docs',
        name: 'docs',
        command: 'C:\\Tools\\docs-mcp.exe',
        args: ['--stdio', 'C:\\Work Files'],
        env: { MCP_TOKEN: 'device-secret' },
      },
    })
    expect(operationOrder).toEqual(['mcp-binding'])
  })
})
