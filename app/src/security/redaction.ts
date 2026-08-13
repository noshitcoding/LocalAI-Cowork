export const REDACTED_VALUE = '[REDACTED]'
const TRUNCATED_VALUE = '[TRUNCATED]'
const MAX_DEPTH = 16

function normalizeKey(key: string): string {
  return key.toLowerCase().replace(/[^a-z0-9]/g, '')
}

export function isSensitiveKey(key: string): boolean {
  const normalized = normalizeKey(key)
  return normalized === 'env'
    || normalized === 'environment'
    || normalized === 'headers'
    || normalized === 'authorization'
    || normalized === 'cookie'
    || normalized === 'setcookie'
    || normalized === 'auth'
    || normalized === 'token'
    || normalized === 'credential'
    || normalized === 'credentials'
    || normalized === 'configjson'
    || normalized.includes('apikey')
    || normalized.includes('accesstoken')
    || normalized.includes('refreshtoken')
    || normalized.includes('authtoken')
    || normalized.endsWith('token')
    || normalized.includes('password')
    || normalized.includes('passwd')
    || normalized.includes('clientsecret')
    || normalized.includes('privatekey')
    || normalized.includes('signingkey')
    || normalized.includes('webhookurl')
    || normalized.endsWith('secret')
}

export function redactText(value: string): string {
  return value
    .replace(/(bearer\s+)[^\s,;'"&]+/gi, `$1${REDACTED_VALUE}`)
    .replace(/((?:api_?key|access_token|refresh_token|token|password|passwd|client_secret|secret|signature|key)=)[^\s,;'"&#]+/gi, `$1${REDACTED_VALUE}`)
    .replace(/\b(?:sk-[A-Za-z0-9_-]{8,}|gh[pousr]_[A-Za-z0-9_-]{8,}|xox[baprs]-[A-Za-z0-9_-]{8,}|AIza[A-Za-z0-9_-]{8,})\b/g, REDACTED_VALUE)
}

function redactAtDepth(value: unknown, depth: number, seen: WeakSet<object>): unknown {
  if (depth > MAX_DEPTH) return TRUNCATED_VALUE
  if (typeof value === 'string') return redactText(value)
  if (value === null || typeof value !== 'object') return value
  if (seen.has(value)) return TRUNCATED_VALUE
  seen.add(value)

  if (Array.isArray(value)) {
    return value.map((entry) => redactAtDepth(entry, depth + 1, seen))
  }

  return Object.fromEntries(Object.entries(value).map(([key, entry]) => [
    key,
    isSensitiveKey(key) ? REDACTED_VALUE : redactAtDepth(entry, depth + 1, seen),
  ]))
}

export function redactSensitiveData(value: unknown): unknown {
  return redactAtDepth(value, 0, new WeakSet())
}

export function redactRecord(value: Record<string, unknown>): Record<string, unknown> {
  return redactSensitiveData(value) as Record<string, unknown>
}
