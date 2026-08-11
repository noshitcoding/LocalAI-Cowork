import type {
  CapabilityCatalog,
  ExecutorTarget,
  ProjectRecord,
  ProviderProfile,
} from './contracts'

export type RemoteTargetChoice = {
  key: string
  label: string
  target: ExecutorTarget
  capabilities: Set<string>
}

export function remoteTargetKey(target: ExecutorTarget): string {
  if (target.kind === 'server_linux') return `server:${target.pool_id ?? ''}`
  if (target.kind === 'managed_windows_pool') return `windows:${target.pool_id}`
  return `device:${target.device_id}`
}

export function remoteTargetChoices(catalog: CapabilityCatalog): RemoteTargetChoice[] {
  const choices: RemoteTargetChoice[] = [{
    key: 'server:',
    label: 'Linux server',
    target: { kind: 'server_linux', pool_id: null },
    capabilities: new Set(catalog.server_linux.map((capability) => capability.name)),
  }]
  const seenPools = new Set<string>()
  for (const executor of catalog.executors) {
    if (!executor.online) continue
    const capabilities = new Set(
      executor.registration.capabilities.map((capability) => capability.name),
    )
    if (executor.registration.kind === 'managed_windows' && executor.registration.pool_id) {
      const key = `windows:${executor.registration.pool_id}`
      if (seenPools.has(key)) continue
      seenPools.add(key)
      choices.push({
        key,
        label: `Windows pool · ${executor.registration.display_name}`,
        target: { kind: 'managed_windows_pool', pool_id: executor.registration.pool_id },
        capabilities,
      })
    } else if (executor.registration.kind === 'personal_device') {
      choices.push({
        key: `device:${executor.registration.executor_id}`,
        label: `Personal device · ${executor.registration.display_name}`,
        target: { kind: 'personal_device', device_id: executor.registration.executor_id },
        capabilities,
      })
    }
  }
  return choices
}

export function providerEndpointBinding(profile: ProviderProfile): 'server' | 'per_device' {
  const defaults = profile.model_defaults
  if (typeof defaults === 'object' && defaults !== null
      && (defaults as Record<string, unknown>).endpoint_binding === 'per_device') {
    return 'per_device'
  }
  return 'server'
}

export function providerSupportsTarget(
  profile: ProviderProfile,
  target: ExecutorTarget,
): boolean {
  const required = target.kind === 'personal_device' ? 'per_device' : 'server'
  return providerEndpointBinding(profile) === required
}

export function providerSupportsProject(
  profile: ProviderProfile,
  project: ProjectRecord,
): boolean {
  return profile.team_id === null || profile.team_id === project.team_id
}

export function providerModelLabel(profile: ProviderProfile): string {
  const defaults = profile.model_defaults
  const model = typeof defaults === 'object' && defaults !== null
    ? (defaults as Record<string, unknown>).model
    : null
  return `${profile.name}${typeof model === 'string' && model ? ` · ${model}` : ''}`
}
