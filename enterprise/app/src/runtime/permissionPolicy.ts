export type DurableToolPolicy = 'read_only' | 'confirm_mutations' | 'autonomous'

/** Fail closed for unknown imported settings. Only an explicit bypass is autonomous. */
export function durableToolPolicy(permissionMode: string | null | undefined): DurableToolPolicy {
  if (permissionMode === 'bypass') return 'autonomous'
  if (permissionMode === 'default') return 'confirm_mutations'
  return 'read_only'
}
