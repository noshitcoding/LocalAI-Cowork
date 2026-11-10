import { beforeEach, describe, expect, it, vi } from 'vitest'
const mocks = vi.hoisted(() => ({ account: { serverUrl: 'https://manager.test', userId: '00000001-1111-4111-8111-111111111111' }, token: vi.fn(), fetch: vi.fn() }))
vi.mock('../stores/remoteRuntimeStore', () => ({ remoteTokenProvider: () => mocks.token, useRemoteRuntimeStore: { getState: () => mocks.account } }))
import { ManagementClient } from './managementClient'

const uuid = (n: number) => `${n.toString().padStart(8, '0')}-1111-4111-8111-111111111111`
const request = { source_instance_id: uuid(3), target_instance_id: uuid(4), source_revision: 2, target_revision: 3, backup_id: uuid(5), idempotency_key: uuid(6) }
const receipt = { id: uuid(7), phase: 'prepared', revision: 1, operation_id: uuid(8) }

describe('authenticated import client', () => {
  beforeEach(() => { vi.resetAllMocks(); mocks.account = { serverUrl: 'https://manager.test', userId: uuid(1) }; mocks.token.mockResolvedValue('private-fixture-token'); vi.stubGlobal('fetch', mocks.fetch); mocks.fetch.mockResolvedValue(new Response(JSON.stringify(receipt), { status: 200 })) })
  it('sends exactly one bounded authenticated POST without automatic retry and retains caller key', async () => {
    const client = new ManagementClient(mocks.account.serverUrl)
    expect(await client.prepareInstanceImport(uuid(2), request)).toEqual(receipt)
    expect(mocks.fetch).toHaveBeenCalledTimes(1)
    expect(mocks.fetch.mock.calls[0]).toEqual(['https://manager.test/management/v1/organisations/' + uuid(2) + '/instance-imports', expect.objectContaining({ method: 'POST', redirect: 'error', credentials: 'same-origin', body: JSON.stringify(request), headers: expect.objectContaining({ Authorization: 'Bearer private-fixture-token' }) })])
    mocks.fetch.mockRejectedValue(new Error('connection lost'))
    await expect(client.prepareInstanceImport(uuid(2), request)).rejects.toThrow('connection lost')
    expect(mocks.fetch).toHaveBeenCalledTimes(2)
  })
  it('rejects user changes during credential resolution before any request', async () => {
    const client = new ManagementClient(mocks.account.serverUrl)
    mocks.token.mockImplementation(async () => { mocks.account.userId = uuid(9); return 'other-user-token' })
    await expect(client.prepareInstanceImport(uuid(2), request)).rejects.toThrow('Server account changed')
    expect(mocks.fetch).not.toHaveBeenCalled()
  })
  it('does not deliver an old account response after server or actor changes', async () => {
    const client = new ManagementClient(mocks.account.serverUrl)
    mocks.fetch.mockImplementation(async () => { mocks.account.serverUrl = 'https://other.test'; return new Response(JSON.stringify(receipt)) })
    await expect(client.prepareInstanceImport(uuid(2), request)).rejects.toThrow('Server account changed')
  })
  it('rejects fabricated receipt fields and invalid instance/revision bindings', async () => {
    const client = new ManagementClient(mocks.account.serverUrl)
    expect(() => client.prepareInstanceImport(uuid(2), { ...request, source_revision: 0 })).toThrow()
    expect(() => client.prepareInstanceImport(uuid(2), { ...request, target_instance_id: request.source_instance_id })).toThrow()
    expect(mocks.fetch).not.toHaveBeenCalled()
    mocks.fetch.mockResolvedValue(new Response(JSON.stringify({ ...receipt, revision: 9 })))
    await expect(client.prepareInstanceImport(uuid(2), request)).rejects.toThrow()
  })
})
