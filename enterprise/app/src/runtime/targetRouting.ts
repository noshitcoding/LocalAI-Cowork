import type {
  Capability,
  CapabilityCatalog,
  ExecutorRecord,
  ExecutorTarget,
} from './contracts'

export interface EligibleTarget {
  target: ExecutorTarget
  label: string
  capabilities: ReadonlySet<Capability>
  available: boolean
  unavailableReason?: string
}

export function eligibleTargets(
  catalog: CapabilityCatalog,
  requiredCapabilities: readonly Capability[],
): EligibleTarget[] {
  const required = new Set(requiredCapabilities)
  const targets: EligibleTarget[] = []
  const serverCapabilities = new Set(catalog.server_linux.map((item) => item.name))
  targets.push({
    target: { kind: 'server_linux' },
    label: 'Linux server',
    capabilities: serverCapabilities,
    available: containsAll(serverCapabilities, required),
    unavailableReason: containsAll(serverCapabilities, required)
      ? undefined
      : missingMessage(serverCapabilities, required),
  })

  const windowsByPool = new Map<string, ExecutorRecord[]>()
  for (const executor of catalog.executors) {
    if (executor.registration.kind !== 'managed_windows' || !executor.registration.pool_id) continue
    const pool = windowsByPool.get(executor.registration.pool_id) ?? []
    pool.push(executor)
    windowsByPool.set(executor.registration.pool_id, pool)
  }
  for (const [poolId, executors] of windowsByPool) {
    const matching = executors.filter(
      (executor) =>
        executor.online &&
        !executor.draining &&
        containsAll(capabilitySet(executor), required),
    )
    const capabilities = unionCapabilities(executors)
    targets.push({
      target: { kind: 'managed_windows_pool', pool_id: poolId },
      label: `Windows pool ${poolId.slice(0, 8)}`,
      capabilities,
      available: matching.length > 0,
      unavailableReason:
        matching.length > 0
          ? undefined
          : containsAll(capabilities, required)
            ? 'No matching Windows executor is online'
            : missingMessage(capabilities, required),
    })
  }

  for (const executor of catalog.executors) {
    if (executor.registration.kind !== 'personal_device') continue
    const capabilities = capabilitySet(executor)
    const capable = containsAll(capabilities, required)
    targets.push({
      target: { kind: 'personal_device', device_id: executor.registration.executor_id },
      label: executor.registration.display_name,
      capabilities,
      available: capable && executor.online && !executor.draining,
      unavailableReason: capable
        ? executor.online
          ? executor.draining
            ? 'Device is draining'
            : undefined
          : 'Device is offline'
        : missingMessage(capabilities, required),
    })
  }
  return targets
}

function capabilitySet(executor: ExecutorRecord): Set<Capability> {
  return new Set(executor.registration.capabilities.map((item) => item.name))
}

function unionCapabilities(executors: readonly ExecutorRecord[]): Set<Capability> {
  return new Set(executors.flatMap((executor) => [...capabilitySet(executor)]))
}

function containsAll(
  available: ReadonlySet<Capability>,
  required: ReadonlySet<Capability>,
): boolean {
  return [...required].every((capability) => available.has(capability))
}

function missingMessage(
  available: ReadonlySet<Capability>,
  required: ReadonlySet<Capability>,
): string {
  const missing = [...required].filter((capability) => !available.has(capability))
  return `Missing capabilities: ${missing.join(', ')}`
}
