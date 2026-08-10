import { describe, expect, it } from 'vitest'

import { HybridRuntimeClient, RemoteRuntimeClient, normalizeServerUrl, type RuntimeClient } from './runtimeClient'
import type { CapabilityCatalog, ExecutorRecord } from './contracts'
import { eligibleTargets } from './targetRouting'

const unusedRuntime = (kind: 'local' | 'remote'): RuntimeClient => ({
  kind,
  version: async () => ({
    api_version: 'v1',
    schema_version: 1,
    minimum_compatible_schema_version: 1,
    build_version: 'test',
  }),
  capabilities: async () => ({ schema_version: 1, server_linux: [], executors: [] }),
  createRun: async () => Promise.reject(new Error('unused')),
  listRuns: async () => [],
  getRun: async () => Promise.reject(new Error('unused')),
  cancelRun: async () => Promise.reject(new Error('unused')),
  subscribeRunEvents: () => () => undefined,
})

describe('runtime routing', () => {
  it('keeps personal-device runs on the local runtime', () => {
    const local = unusedRuntime('local')
    const remote = unusedRuntime('remote')
    const hybrid = new HybridRuntimeClient({ local, remote })
    expect(
      hybrid.forTarget({
        kind: 'personal_device',
        device_id: '00000000-0000-0000-0000-000000000010',
      }),
    ).toBe(local)
    expect(hybrid.forTarget({ kind: 'server_linux' })).toBe(remote)
  })

  it('requires HTTPS away from loopback', () => {
    expect(normalizeServerUrl('http://localhost:8080/')).toBe('http://localhost:8080')
    expect(() => normalizeServerUrl('http://cowork.example.com')).toThrow(/HTTPS/)
  })

  it('does not offer Microsoft Office on a Linux-only target', () => {
    const catalog: CapabilityCatalog = {
      schema_version: 1,
      server_linux: [
        { schema_version: 1, name: 'office.libreoffice', version: '1', attributes: {} },
      ],
      executors: [],
    }
    const [server] = eligibleTargets(catalog, ['office.microsoft'])
    expect(server.available).toBe(false)
    expect(server.unavailableReason).toContain('office.microsoft')
  })

  it('updates only the owner-controlled server ceiling for a personal device', async () => {
    const executor: ExecutorRecord = {
      registration: {
        schema_version: 2,
        executor_id: '10000000-0000-4000-8000-000000000010',
        kind: 'personal_device',
        pool_id: null,
        owner_user_id: '10000000-0000-4000-8000-000000000020',
        display_name: 'Laptop',
        protocol_version: 2,
        capabilities: [],
        labels: { local_remote_control_mode: 'confirm_each_session' },
        personal_device_remote_control: 'confirm_each_session',
        max_concurrent_runs: 1,
      },
      online: true,
      draining: false,
      active_runs: 0,
      last_seen_at: '2026-08-09T12:00:00Z',
    }
    let submitted: Record<string, unknown> | undefined
    const client = new RemoteRuntimeClient({
      baseUrl: 'https://cowork.example.test',
      accessToken: () => 'access-token',
      fetch: async (_input, init) => {
        submitted = JSON.parse(String(init?.body)) as Record<string, unknown>
        return new Response(JSON.stringify({
          ...executor,
          registration: { ...executor.registration, personal_device_remote_control: 'off' },
        }), { status: 200, headers: { 'content-type': 'application/json' } })
      },
    })
    const updated = await client.setPersonalDeviceRemoteControl(executor, 'off')
    expect(submitted).toMatchObject({
      executor_id: executor.registration.executor_id,
      owner_user_id: null,
      personal_device_remote_control: 'off',
    })
    expect(updated.registration.personal_device_remote_control).toBe('off')
    expect(updated.registration.labels.local_remote_control_mode).toBe('confirm_each_session')
  })

  it('lists and revokes authentication sessions through the protected API', async () => {
    const requests: Array<{ url: string; method: string }> = []
    const session = {
      schema_version: 2,
      id: '10000000-0000-4000-8000-000000000001',
      device_id: '20000000-0000-4000-8000-000000000001',
      current: true,
      active: true,
      created_at: '2026-08-10T10:00:00Z',
      last_used_at: '2026-08-10T12:00:00Z',
      expires_at: '2026-09-09T10:00:00Z',
      revoked_at: null,
      revoke_reason: null,
    }
    const client = new RemoteRuntimeClient({
      baseUrl: 'https://cowork.example.test',
      accessToken: () => 'access-token',
      fetch: async (input, init) => {
        requests.push({ url: String(input), method: init?.method ?? 'GET' })
        return init?.method === 'DELETE'
          ? new Response(null, { status: 204 })
          : new Response(JSON.stringify([session]), { status: 200 })
      },
    })

    await expect(client.listAuthSessions()).resolves.toEqual([session])
    await client.revokeAuthSession(session.id)
    expect(requests).toEqual([
      { url: 'https://cowork.example.test/api/v1/auth/sessions', method: 'GET' },
      { url: `https://cowork.example.test/api/v1/auth/sessions/${session.id}`, method: 'DELETE' },
    ])
  })

  it('creates and lists the durable message/run pair through the thread API', async () => {
    const threadId = '10000000-0000-4000-8000-000000000011'
    const projectId = '10000000-0000-4000-8000-000000000012'
    const runId = '10000000-0000-4000-8000-000000000013'
    const userId = '10000000-0000-4000-8000-000000000014'
    const messageId = '10000000-0000-4000-8000-000000000015'
    const now = '2026-08-10T12:00:00Z'
    const run = {
      spec: {
        schema_version: 2,
        id: runId,
        thread_id: threadId,
        project_id: projectId,
        project: { id: projectId, revision: 1 },
        project_privacy: 'team_managed',
        task: null,
        creator_user_id: userId,
        executor_target: { kind: 'server_linux', pool_id: null },
        required_capabilities: [],
        input: { prompt: 'hello' },
        model_profile_id: null,
        snapshot_id: null,
        idempotency_key: 'message-run-test',
        created_at: now,
      },
      state: 'queued',
      revision: 1,
      etag: `W/"${runId}:1"`,
      assigned_executor_id: null,
      lease_expires_at: null,
      started_at: null,
      finished_at: null,
      result: null,
      error: null,
      updated_at: now,
    } as const
    const message = {
      schema_version: 2,
      id: messageId,
      revision: 1,
      etag: `W/"${messageId}:1"`,
      thread_id: threadId,
      author_user_id: userId,
      role: 'user',
      content: { text: 'hello' },
      run_id: runId,
      created_at: now,
      updated_at: now,
      deleted_at: null,
    } as const
    const requests: Array<{ url: string; method: string; body?: unknown }> = []
    const client = new RemoteRuntimeClient({
      baseUrl: 'https://cowork.example.test',
      accessToken: () => 'access-token',
      fetch: async (input, init) => {
        requests.push({
          url: String(input),
          method: init?.method ?? 'GET',
          body: init?.body ? JSON.parse(String(init.body)) : undefined,
        })
        const payload = init?.method === 'POST'
          ? { schema_version: 2, message, run }
          : [message]
        return new Response(JSON.stringify(payload), {
          status: init?.method === 'POST' ? 201 : 200,
          headers: { 'content-type': 'application/json' },
        })
      },
    })
    const request = {
      content: { text: 'hello' },
      run: {
        thread_id: threadId,
        project_id: projectId,
        project_revision: 1,
        project_privacy: 'team_managed' as const,
        task: null,
        executor_target: { kind: 'server_linux' as const, pool_id: null },
        required_capabilities: [],
        input: { prompt: 'hello' },
        model_profile_id: null,
        snapshot_id: null,
        idempotency_key: 'message-run-test',
      },
    }

    await expect(client.createThreadMessage(threadId, request)).resolves.toMatchObject({
      message: { id: messageId }, run: { spec: { id: runId } },
    })
    await expect(client.listThreadMessages(threadId, 10)).resolves.toEqual([message])
    expect(requests).toEqual([
      {
        url: `https://cowork.example.test/api/v1/threads/${threadId}/messages`,
        method: 'POST',
        body: request,
      },
      {
        url: `https://cowork.example.test/api/v1/threads/${threadId}/messages?limit=10`,
        method: 'GET',
        body: undefined,
      },
    ])
  })

  it('pushes idempotent metadata changes and advances the inbox cursor', async () => {
    const operationId = '10000000-0000-4000-8000-000000000021'
    const deviceId = '10000000-0000-4000-8000-000000000022'
    const entityId = '10000000-0000-4000-8000-000000000023'
    const timestamp = '2026-08-10T12:00:00Z'
    const change = {
      schema_version: 2,
      operation_id: operationId,
      device_id: deviceId,
      entity_type: 'memory',
      entity_id: entityId,
      base_revision: 0,
      operation: 'upsert' as const,
      payload: { text: 'durable memory' },
      client_timestamp: timestamp,
    }
    const entity = {
      schema_version: 2,
      entity_type: 'memory',
      entity_id: entityId,
      revision: 1,
      etag: `W/"${entityId}:1"`,
      payload: change.payload,
      tombstone: false,
      updated_at: timestamp,
    }
    const requests: Array<{ url: string; method: string; body?: unknown }> = []
    const client = new RemoteRuntimeClient({
      baseUrl: 'https://cowork.example.test',
      accessToken: () => 'access-token',
      fetch: async (input, init) => {
        requests.push({
          url: String(input),
          method: init?.method ?? 'GET',
          body: init?.body ? JSON.parse(String(init.body)) : undefined,
        })
        const payload = init?.method === 'POST'
          ? { schema_version: 2, results: [{ schema_version: 2, operation_id: operationId, status: 'applied', entity }] }
          : {
              schema_version: 2,
              changes: [{
                schema_version: 2,
                cursor: 7,
                entity_type: 'memory',
                entity_id: entityId,
                revision: 1,
                operation: 'upsert',
                payload: change.payload,
                created_at: timestamp,
              }],
              next_cursor: 7,
            }
        return new Response(JSON.stringify(payload), {
          status: 200, headers: { 'content-type': 'application/json' },
        })
      },
    })

    await expect(client.pushSyncChanges([change])).resolves.toMatchObject({
      results: [{ operation_id: operationId, status: 'applied', entity: { revision: 1 } }],
    })
    await expect(client.pullSyncChanges(3, 50)).resolves.toMatchObject({
      next_cursor: 7,
      changes: [{ entity_id: entityId, revision: 1 }],
    })
    expect(requests).toEqual([
      {
        url: 'https://cowork.example.test/api/v1/sync/changes',
        method: 'POST',
        body: { changes: [change] },
      },
      {
        url: 'https://cowork.example.test/api/v1/sync/changes?after=3&limit=50',
        method: 'GET',
        body: undefined,
      },
    ])
  })
})
