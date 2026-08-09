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
})
