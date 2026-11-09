/** Catalog evidence is session-local and bound to the exact provider configuration. */
export const MODEL_CATALOG_MAX_AGE_MS = 5 * 60_000
export type ProviderCatalogTarget = { baseUrl: string; authMode?: string; verifyTlsCertificates?: boolean }
export type ProviderModelCatalogStamp = { target: string; checkedAt: number }
export type ProviderModelCatalogs = Record<string, ProviderModelCatalogStamp | undefined>

export function providerCatalogTarget(profile: ProviderCatalogTarget): string {
  return JSON.stringify([profile.baseUrl.trim().replace(/\/$/, ''), profile.authMode ?? 'bearer', profile.verifyTlsCertificates !== false])
}

export function isModelCatalogTimestampFresh(checkedAt: number | undefined, now = Date.now()): boolean {
  return typeof checkedAt === 'number' && Number.isFinite(checkedAt)
    && checkedAt <= now && now - checkedAt < MODEL_CATALOG_MAX_AGE_MS
}

export function isProviderModelCatalogFresh(profile: ProviderCatalogTarget, stamp: ProviderModelCatalogStamp | undefined, now = Date.now()): boolean {
  return Boolean(stamp && stamp.target === providerCatalogTarget(profile) && isModelCatalogTimestampFresh(stamp.checkedAt, now))
}
