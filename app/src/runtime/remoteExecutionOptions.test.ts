import { describe, expect, it } from 'vitest'

import type { ProjectRecord, ProviderProfile } from './contracts'
import { providerSupportsProject, providerSupportsTarget } from './remoteExecutionOptions'

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
