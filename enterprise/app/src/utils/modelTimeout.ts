export const MIN_MODEL_TIMEOUT_MS = 1_000
export const MAX_MODEL_TIMEOUT_MS = 24 * 60 * 60 * 1_000
export const DEFAULT_MODEL_TIMEOUT_MS = 600_000

export function normalizeModelTimeoutMs(
  value: unknown,
  fallback = DEFAULT_MODEL_TIMEOUT_MS,
): number {
  const numeric = Number(value)
  const numericFallback = Number(fallback)
  const candidate = Number.isFinite(numeric)
    ? Math.trunc(numeric)
    : Number.isFinite(numericFallback)
      ? Math.trunc(numericFallback)
      : DEFAULT_MODEL_TIMEOUT_MS
  return Math.max(MIN_MODEL_TIMEOUT_MS, Math.min(MAX_MODEL_TIMEOUT_MS, candidate))
}
