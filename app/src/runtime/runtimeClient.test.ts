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

  it('updates and deletes projects and threads with optimistic revisions', async () => {
    const projectId = '10000000-0000-4000-8000-000000000031'
    const threadId = '10000000-0000-4000-8000-000000000032'
    const userId = '10000000-0000-4000-8000-000000000033'
    const now = '2026-08-10T12:00:00Z'
    const project = {
      schema_version: 2,
      id: projectId,
      revision: 2,
      etag: `W/"${projectId}:2"`,
      owner_user_id: userId,
      team_id: null,
      privacy: 'private_local',
      name: 'Updated project',
      description: '',
      preferred_executor_target: null,
      current_version_id: null,
      policy: {},
      created_at: now,
      updated_at: now,
      deleted_at: null,
    } as const
    const thread = {
      schema_version: 2,
      id: threadId,
      revision: 2,
      etag: `W/"${threadId}:2"`,
      project_id: projectId,
      forked_from_thread_id: null,
      forked_from_message_id: null,
      title: 'Updated thread',
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
        if (init?.method === 'DELETE') return new Response(null, { status: 204 })
        return new Response(JSON.stringify(String(input).includes('/threads/') ? thread : project), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        })
      },
    })
    const projectUpdate = {
      expected_revision: 1,
      name: project.name,
      description: project.description,
      preferred_executor_target: null,
      policy: {},
    }
    const threadUpdate = { expected_revision: 1, title: thread.title }

    await expect(client.updateProject(projectId, projectUpdate)).resolves.toEqual(project)
    await expect(client.updateThread(threadId, threadUpdate)).resolves.toEqual(thread)
    await client.deleteThread(threadId, thread.revision)
    await client.deleteProject(projectId, project.revision)

    expect(requests).toEqual([
      {
        url: `https://cowork.example.test/api/v1/projects/${projectId}`,
        method: 'PUT',
        body: projectUpdate,
      },
      {
        url: `https://cowork.example.test/api/v1/threads/${threadId}`,
        method: 'PUT',
        body: threadUpdate,
      },
      {
        url: `https://cowork.example.test/api/v1/threads/${threadId}?expected_revision=2`,
        method: 'DELETE',
        body: undefined,
      },
      {
        url: `https://cowork.example.test/api/v1/projects/${projectId}?expected_revision=2`,
        method: 'DELETE',
        body: undefined,
      },
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
          : String(input).includes('/sync/entities/')
            ? { schema_version: 2, items: [entity], next_after: null, watermark_cursor: 7 }
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
    await expect(client.listSyncedEntities('memory', null, 50)).resolves.toMatchObject({
      items: [{ entity_id: entityId, revision: 1 }], watermark_cursor: 7,
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
      {
        url: 'https://cowork.example.test/api/v1/sync/entities/memory?limit=50',
        method: 'GET',
        body: undefined,
      },
    ])
  })

  it('scopes task and schedule listings to the selected project', async () => {
    const projectId = '10000000-0000-4000-8000-000000000030'
    const urls: string[] = []
    const client = new RemoteRuntimeClient({
      baseUrl: 'https://cowork.example.test',
      accessToken: () => 'access-token',
      fetch: async (input) => {
        urls.push(String(input))
        return new Response('[]', {
          status: 200,
          headers: { 'content-type': 'application/json' },
        })
      },
    })

    await expect(client.listTasks(projectId)).resolves.toEqual([])
    await expect(client.listSchedules(projectId)).resolves.toEqual([])
    expect(urls).toEqual([
      `https://cowork.example.test/api/v1/tasks?project_id=${projectId}`,
      `https://cowork.example.test/api/v1/schedules?project_id=${projectId}`,
    ])
  })

  it('manages server provider profiles without returning secret material', async () => {
    const profileId = '10000000-0000-4000-8000-000000000040'
    const requests: Array<{ url: string; method: string; body: unknown }> = []
    const profile = {
      schema_version: 2,
      id: profileId,
      revision: 1,
      etag: `W/"${profileId}:1"`,
      owner_user_id: '10000000-0000-4000-8000-000000000041',
      team_id: null,
      name: 'Server OpenAI-compatible',
      provider_kind: 'openai_compatible',
      model_defaults: {
        base_url: 'https://models.example.test/v1',
        model: 'example-model',
        endpoint_binding: 'server',
      },
      has_secret: false,
      created_at: '2026-08-10T12:00:00Z',
      updated_at: '2026-08-10T12:00:00Z',
      deleted_at: null,
    }
    const client = new RemoteRuntimeClient({
      baseUrl: 'https://cowork.example.test',
      accessToken: () => 'access-token',
      fetch: async (input, init) => {
        requests.push({
          url: String(input),
          method: init?.method ?? 'GET',
          body: init?.body ? JSON.parse(String(init.body)) : undefined,
        })
        if (init?.method === 'DELETE') return new Response(null, { status: 204 })
        return new Response(JSON.stringify(init?.method ? profile : [profile]), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        })
      },
    })

    await expect(client.listProviderProfiles()).resolves.toEqual([profile])
    await expect(client.createProviderProfile({
      team_id: null,
      name: profile.name,
      provider_kind: profile.provider_kind,
      model_defaults: profile.model_defaults,
      api_key: null,
    })).resolves.toEqual(profile)
    await expect(client.setProviderProfileSecret(profileId, 1, 'one-time-secret'))
      .resolves.toEqual(profile)
    await client.deleteProviderProfile(profileId, 1)

    expect(requests.map(({ url, method }) => ({ url, method }))).toEqual([
      { url: 'https://cowork.example.test/api/v1/provider-profiles', method: 'GET' },
      { url: 'https://cowork.example.test/api/v1/provider-profiles', method: 'POST' },
      {
        url: `https://cowork.example.test/api/v1/provider-profiles/${profileId}/secret`,
        method: 'PUT',
      },
      {
        url: `https://cowork.example.test/api/v1/provider-profiles/${profileId}?expected_revision=1`,
        method: 'DELETE',
      },
    ])
    expect(requests[2].body).toEqual({ expected_revision: 1, api_key: 'one-time-secret' })
  })

  it('creates, versions, releases, and deletes reusable server tasks', async () => {
    const projectId = '10000000-0000-4000-8000-000000000050'
    const taskId = '10000000-0000-4000-8000-000000000051'
    const requests: Array<{ url: string; method: string; body: unknown }> = []
    const task = {
      schema_version: 2, id: taskId, revision: 1, etag: `W/"${taskId}:1"`, project_id: projectId,
      name: 'Reusable task', instructions: 'Complete the task', required_capabilities: [],
      default_target: { kind: 'server_linux' as const, pool_id: null }, config: {}, released: true,
      created_at: '2026-08-10T12:00:00Z', deleted_at: null,
    }
    const client = new RemoteRuntimeClient({
      baseUrl: 'https://cowork.example.test', accessToken: () => 'access-token',
      fetch: async (input, init) => {
        requests.push({
          url: String(input), method: init?.method ?? 'GET',
          body: init?.body ? JSON.parse(String(init.body)) : undefined,
        })
        if (init?.method === 'DELETE') return new Response(null, { status: 204 })
        return new Response(JSON.stringify(task), {
          status: 200, headers: { 'content-type': 'application/json' },
        })
      },
    })
    const fields = {
      name: task.name, instructions: task.instructions, required_capabilities: [],
      default_target: task.default_target, config: {}, release: true,
    }
    await expect(client.createTask({ project_id: projectId, ...fields })).resolves.toEqual(task)
    await expect(client.createTaskVersion(taskId, { base_revision: 1, ...fields })).resolves.toEqual(task)
    await expect(client.releaseTaskVersion(taskId, 1)).resolves.toEqual(task)
    await client.deleteTask(taskId, 1)
    expect(requests.map(({ url, method }) => ({ url, method }))).toEqual([
      { url: 'https://cowork.example.test/api/v1/tasks', method: 'POST' },
      { url: `https://cowork.example.test/api/v1/tasks/${taskId}/versions`, method: 'POST' },
      { url: `https://cowork.example.test/api/v1/tasks/${taskId}/release`, method: 'POST' },
      { url: `https://cowork.example.test/api/v1/tasks/${taskId}?expected_revision=1`, method: 'DELETE' },
    ])
  })

  it('lists visible teams for scoped provider profiles', async () => {
    const team = {
      schema_version: 2,
      id: '10000000-0000-4000-8000-000000000060',
      revision: 1,
      etag: 'W/"10000000-0000-4000-8000-000000000060:1"',
      name: 'Example team',
      owner_user_id: '10000000-0000-4000-8000-000000000061',
      created_at: '2026-08-10T12:00:00Z',
      updated_at: '2026-08-10T12:00:00Z',
      deleted_at: null,
    }
    const client = new RemoteRuntimeClient({
      baseUrl: 'https://cowork.example.test', accessToken: () => 'access-token',
      fetch: async () => new Response(JSON.stringify([team]), {
        status: 200, headers: { 'content-type': 'application/json' },
      }),
    })
    await expect(client.listTeams()).resolves.toEqual([team])
  })

  it('resumes sync SSE from the last durable cursor', async () => {
    const entityId = '10000000-0000-4000-8000-000000000031'
    const event = {
      schema_version: 2,
      cursor: 9,
      entity_type: 'memory',
      entity_id: entityId,
      revision: 2,
      operation: 'delete',
      payload: null,
      created_at: '2026-08-10T12:00:00Z',
    }
    let lastEventId: string | null = null
    const client = new RemoteRuntimeClient({
      baseUrl: 'https://cowork.example.test',
      accessToken: () => 'access-token',
      reconnectDelayMs: 1,
      fetch: async (_input, init) => {
        lastEventId = new Headers(init?.headers).get('last-event-id')
        const encoded = new TextEncoder().encode(
          `id: 9\nevent: sync_change\ndata: ${JSON.stringify(event)}\n\n`,
        )
        return new Response(new ReadableStream({
          start(controller) {
            controller.enqueue(encoded)
            controller.close()
          },
        }), { status: 200, headers: { 'content-type': 'text/event-stream' } })
      },
    })
    let resolveEvent!: (value: unknown) => void
    const received = new Promise((resolve) => { resolveEvent = resolve })
    const unsubscribe = client.subscribeSyncEvents(4, resolveEvent)

    await expect(received).resolves.toEqual(event)
    unsubscribe()
    expect(lastEventId).toBe('4')
  })
})
