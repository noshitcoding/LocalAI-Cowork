import { describe, expect, it } from 'vitest'

import type {
  CapabilityCatalog,
  ExecutorRecord,
  ProjectRecord,
  ProviderProfile,
  SyncedEntity,
} from './contracts'
import {
  providerSupportsProject,
  providerSupportsTarget,
  remoteTargetChoices,
  remoteTargetSupports,
  selectedMcpServerNames,
} from './remoteExecutionOptions'

const profile = (binding: 'server' | 'per_device', teamId: string | null = null): ProviderProfile => ({
  schema_version: 2,
  id: crypto.randomUUID(),
  revision: 1,
  etag: 'W/"profile:1"',
  owner_user_id: teamId ? null : crypto.randomUUID(),
  team_id: teamId,
  name: 'Profile',
  provider_kind: 'openai_compatible',
  model_defaults: { endpoint_binding: binding, model: 'test' },
  has_secret: false,
  created_at: '2026-08-10T12:00:00Z',
  updated_at: '2026-08-10T12:00:00Z',
  deleted_at: null,
})

const project = (teamId: string | null): ProjectRecord => ({
  schema_version: 2,
  id: crypto.randomUUID(),
  revision: 1,
  etag: 'W/"project:1"',
  owner_user_id: crypto.randomUUID(),
  team_id: teamId,
  privacy: teamId ? 'team_managed' : 'private_local',
  name: 'Project',
  description: '',
  preferred_executor_target: null,
  current_version_id: null,
  policy: {},
  created_at: '2026-08-10T12:00:00Z',
  updated_at: '2026-08-10T12:00:00Z',
  deleted_at: null,
})

describe('remote provider routing', () => {
  it('keeps device endpoints on personal devices and server endpoints on managed executors', () => {
    expect(providerSupportsTarget(profile('per_device'), {
      kind: 'personal_device', device_id: crypto.randomUUID(),
    })).toBe(true)
    expect(providerSupportsTarget(profile('per_device'), { kind: 'server_linux', pool_id: null }))
      .toBe(false)
    expect(providerSupportsTarget(profile('server'), {
      kind: 'managed_windows_pool', pool_id: crypto.randomUUID(),
    })).toBe(true)
  })

  it('offers team profiles only inside their team while personal profiles remain reusable', () => {
    const teamId = crypto.randomUUID()
    expect(providerSupportsProject(profile('server', teamId), project(teamId))).toBe(true)
    expect(providerSupportsProject(profile('server', teamId), project(crypto.randomUUID()))).toBe(false)
    expect(providerSupportsProject(profile('server'), project(teamId))).toBe(true)
  })
})

const executor = (
  executorId: string,
  poolId: string,
  capabilities: Array<{ name: string; serverNames?: string[] }>,
  draining = false,
): ExecutorRecord => ({
  registration: {
    schema_version: 2,
    executor_id: executorId,
    kind: 'managed_windows',
    pool_id: poolId,
    owner_user_id: null,
    display_name: `Windows ${executorId.slice(-1)}`,
    protocol_version: 2,
    capabilities: capabilities.map(({ name, serverNames }) => ({
      schema_version: 2,
      name,
      version: 'test',
      attributes: serverNames ? { server_names: serverNames } : {},
    })),
    labels: {},
    max_concurrent_runs: 1,
  },
  online: true,
  draining,
  active_runs: 0,
  last_seen_at: '2026-08-11T12:00:00Z',
})

describe('remote executor routing', () => {
  it('requires one Windows executor to satisfy capabilities and every selected MCP binding', () => {
    const poolId = '10000000-0000-4000-8000-000000000010'
    const catalog: CapabilityCatalog = {
      schema_version: 2,
      server_linux: [],
      executors: [
        executor('10000000-0000-4000-8000-000000000011', poolId, [
          { name: 'office.microsoft' },
          { name: 'tool.mcp.invoke', serverNames: ['CRM'] },
        ]),
        executor('10000000-0000-4000-8000-000000000012', poolId, [
          { name: 'tool.mcp.invoke', serverNames: ['Project docs'] },
        ]),
        executor('10000000-0000-4000-8000-000000000013', poolId, [
          { name: 'office.microsoft' },
          { name: 'tool.mcp.invoke', serverNames: ['Project docs'] },
        ], true),
      ],
    }
    const pool = remoteTargetChoices(catalog).find((choice) => choice.key === `windows:${poolId}`)
    expect(pool).toBeDefined()
    expect(remoteTargetSupports(
      pool!,
      ['office.microsoft', 'tool.mcp.invoke'],
      ['Project docs'],
    )).toBe(false)

    catalog.executors.push(executor(
      '10000000-0000-4000-8000-000000000014',
      poolId,
      [
        { name: 'office.microsoft' },
        { name: 'tool.mcp.invoke', serverNames: ['Project docs', 'CRM'] },
      ],
    ))
    const updatedPool = remoteTargetChoices(catalog)
      .find((choice) => choice.key === `windows:${poolId}`)
    expect(remoteTargetSupports(
      updatedPool!,
      ['office.microsoft', 'tool.mcp.invoke'],
      ['Project docs'],
    )).toBe(true)
  })

  it('derives exact deduplicated MCP names from the selected synchronized metadata', () => {
    const selectedId = '10000000-0000-4000-8000-000000000021'
    const metadata = [
      {
        entity_id: selectedId,
        payload: { name: ' Project docs ' },
      },
      {
        entity_id: '10000000-0000-4000-8000-000000000022',
        payload: { name: 'CRM' },
      },
    ] as SyncedEntity[]
    expect(selectedMcpServerNames(metadata, [selectedId, selectedId])).toEqual(['Project docs'])
  })
})
