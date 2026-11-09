import type { ProviderProfile } from './contracts'

export function isManagedModelProfile(profile: ProviderProfile): boolean {
  const defaults = profile.model_defaults
  return typeof defaults === 'object' && defaults !== null && 'source' in defaults && defaults.source === 'management_catalog' && 'read_only' in defaults && defaults.read_only === true
}
