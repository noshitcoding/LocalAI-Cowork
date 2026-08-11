import type {
  CapabilityCatalog,
  ExecutorRecord,
  ExecutorTarget,
  ProjectRecord,
  ProviderProfile,
  SyncedEntity,
} from './contracts'

type RemoteTargetCandidate = {
  capabilities: Set<string>
  mcpServerNames: Set<string>
}

export type RemoteTargetChoice = {
  key: string
  label: string
  target: ExecutorTarget
  capabilities: Set<string>
  candidates: RemoteTargetCandidate[]
}

export function remoteTargetKey(target: ExecutorTarget): string {
  if (target.kind === 'server_linux') return `server:${target.pool_id ?? ''}`
  if (target.kind === 'managed_windows_pool') return `windows:${target.pool_id}`
  return `device:${target.device_id}`
}

export function remoteTargetChoices(catalog: CapabilityCatalog): RemoteTargetChoice[] {
  const serverCapabilities = new Set(catalog.server_linux.map((capability) => capability.name))
  const choices: RemoteTargetChoice[] = [{
    key: 'server:',
    label: 'Linux server',
    target: { kind: 'server_linux', pool_id: null },
    capabilities: serverCapabilities,
    candidates: [{ capabilities: serverCapabilities, mcpServerNames: new Set() }],
  }]

  const windowsByPool = new Map<string, ExecutorRecord[]>()
  for (const executor of catalog.executors) {
    if (!executor.online || executor.draining) continue
    if (executor.registration.kind === 'managed_windows' && executor.registration.pool_id) {
      const pool = windowsByPool.get(executor.registration.pool_id) ?? []
      pool.push(executor)
      windowsByPool.set(executor.registration.pool_id, pool)
    } else if (executor.registration.kind === 'personal_device') {
      const candidate = targetCandidate(executor)
      choices.push({
        key: `device:${executor.registration.executor_id}`,
        label: `Personal device · ${executor.registration.display_name}`,
        target: { kind: 'personal_device', device_id: executor.registration.executor_id },
        capabilities: candidate.capabilities,
        candidates: [candidate],
      })
    }
  }
  for (const [poolId, executors] of windowsByPool) {
    const candidates = executors.map(targetCandidate)
    choices.push({
      key: `windows:${poolId}`,
      label: `Windows pool · ${executors[0].registration.display_name}`,
      target: { kind: 'managed_windows_pool', pool_id: poolId },
      capabilities: new Set(candidates.flatMap((candidate) => [...candidate.capabilities])),
      candidates,
    })
  }
  return choices
}

export function remoteTargetSupports(
  choice: RemoteTargetChoice,
  requiredCapabilities: readonly string[],
  requiredMcpServerNames: readonly string[] = [],
): boolean {
  const required = new Set(requiredCapabilities)
  const requiredMcp = new Set(requiredMcpServerNames.map((name) => name.trim()).filter(Boolean))
  return choice.candidates.some((candidate) => (
    [...required].every((capability) => candidate.capabilities.has(capability))
    && (choice.target.kind !== 'managed_windows_pool'
      || [...requiredMcp].every((name) => candidate.mcpServerNames.has(name)))
  ))
}

export function selectedMcpServerNames(
  metadata: readonly SyncedEntity[],
  selectedIds: readonly string[],
): string[] {
  const selected = new Set(selectedIds)
  return [...new Set(metadata.flatMap((entity) => {
    if (!selected.has(entity.entity_id) || !entity.payload
        || typeof entity.payload !== 'object' || Array.isArray(entity.payload)) return []
    const name = (entity.payload as Record<string, unknown>).name
    return typeof name === 'string' && name.trim() ? [name.trim()] : []
  }))]
}

function targetCandidate(executor: ExecutorRecord): RemoteTargetCandidate {
  const capabilities = new Set(
    executor.registration.capabilities.map((capability) => capability.name),
  )
  const mcpDescriptor = executor.registration.capabilities.find(
    (capability) => capability.name === 'tool.mcp.invoke',
  )
  const advertisedNames = mcpDescriptor?.attributes.server_names
  const mcpServerNames = new Set(
    Array.isArray(advertisedNames)
      ? advertisedNames.filter((name): name is string => typeof name === 'string' && Boolean(name.trim()))
        .map((name) => name.trim())
      : [],
  )
  return { capabilities, mcpServerNames }
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
