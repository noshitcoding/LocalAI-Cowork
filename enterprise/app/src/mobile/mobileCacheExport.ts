import {fromBase64, mobileSecureGet, toBase64} from './mobileSecure'
import type {MobileCacheBinding} from './mobileOfflineStore'

const LEGACY_KEY = 'open-cowork-mobile-cache-v1'
const PREFIX = 'open-cowork-mobile-cache-v2:'
const MAX_SOURCE = 8 * 1024 * 1024
const uuid = /^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/

export type MobileCacheExport = {
  format: 'localai-cowork-mobile-cache-export/1'
  payload_base64: string; payload_sha256: string; payload_size: number
  ciphertext_base64: string; ciphertext_sha256: string; ciphertext_size: number
  source: {storage_key: string; aad_base64: string; keystore_namespace: 'offline_cache';
    keystore_key: 'aes_gcm_key_v1'; key_never_exported: true}
}
async function sha(bytes: Uint8Array): Promise<string> {
  return Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', Uint8Array.from(bytes).buffer)))
    .map(value => value.toString(16).padStart(2, '0')).join('')
}
async function source(binding: MobileCacheBinding | null): Promise<{key: string; aad: Uint8Array}> {
  if (!binding) return {key: LEGACY_KEY, aad: new TextEncoder().encode(LEGACY_KEY)}
  const origin = new URL(binding.serverOrigin)
  if (origin.origin !== binding.serverOrigin || !['http:', 'https:'].includes(origin.protocol)
    || origin.username || origin.password || !uuid.test(binding.userId)) throw new Error('Invalid original account cache locator')
  const aad = new TextEncoder().encode(JSON.stringify({format: 'open-cowork-mobile-cache-v2',
    serverOrigin: binding.serverOrigin, userId: binding.userId}))
  const hash = toBase64(new Uint8Array(await crypto.subtle.digest('SHA-256', Uint8Array.from(aad).buffer)))
    .replaceAll('/', '_').replaceAll('+', '-').replaceAll('=', '')
  return {key: PREFIX + hash, aad}
}

/** Explicit read-only export: never calls the cache loader, saver or outbox.
 * A successful decrypt proves integrity under this existing device key. It is
 * not Android attestation or proof that every historical row belongs to an Actor.
 */
export async function exportOriginalMobileCache(binding: MobileCacheBinding | null): Promise<MobileCacheExport> {
  const locator = await source(binding)
  const encoded = localStorage.getItem(locator.key)
  if (!encoded) throw new Error('No original encrypted cache exists at the selected locator')
  if (encoded.length > Math.ceil((MAX_SOURCE + 28) / 3) * 4) throw new Error('Original encrypted cache exceeds 8 MiB')
  const packed = fromBase64(encoded)
  if (packed.length < 28 || packed.length > MAX_SOURCE + 28 || toBase64(packed) !== encoded)
    throw new Error('Original encrypted cache is truncated or has an unsupported encoding')
  // Retrieve only. Missing keys are never generated or replaced for an export.
  const keyText = await mobileSecureGet('offline_cache', 'aes_gcm_key_v1')
  if (!keyText) throw new Error('The original device cache key is unavailable; encrypted bytes were preserved')
  const rawKey = fromBase64(keyText)
  if (rawKey.length !== 32) { rawKey.fill(0); throw new Error('The original cache key has an unsupported size') }
  let key: CryptoKey
  try {
    key = await crypto.subtle.importKey('raw', Uint8Array.from(rawKey).buffer, 'AES-GCM', false, ['decrypt'])
  } finally { rawKey.fill(0) }
  let plaintext: Uint8Array | undefined
  try {
    plaintext = new Uint8Array(await crypto.subtle.decrypt({name: 'AES-GCM',
      iv: Uint8Array.from(packed.subarray(0, 12)).buffer,
      additionalData: Uint8Array.from(locator.aad).buffer}, key, Uint8Array.from(packed.subarray(12)).buffer))
    if (plaintext.length > MAX_SOURCE) throw new Error('Original cache plaintext exceeds 8 MiB')
    const value: unknown = JSON.parse(new TextDecoder('utf-8', {fatal: true}).decode(plaintext))
    if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Original cache JSON is invalid')
    const state = value as Record<string, unknown>
    if (![1, 2].includes(Number(state.schemaVersion)) || !Array.isArray(state.runs) || !Array.isArray(state.outbox))
      throw new Error('Original cache schema is unsupported')
    if (binding) {
      const actual = state.binding as MobileCacheBinding | null | undefined
      if (state.schemaVersion !== 2 || actual?.serverOrigin !== binding.serverOrigin || actual.userId !== binding.userId)
        throw new Error('Original account cache binding differs')
    }
    const result: MobileCacheExport = {format: 'localai-cowork-mobile-cache-export/1',
      payload_base64: toBase64(plaintext), payload_sha256: await sha(plaintext), payload_size: plaintext.length,
      ciphertext_base64: encoded, ciphertext_sha256: await sha(packed), ciphertext_size: packed.length,
      source: {storage_key: locator.key, aad_base64: toBase64(locator.aad), keystore_namespace: 'offline_cache',
        keystore_key: 'aes_gcm_key_v1', key_never_exported: true}}
    if (localStorage.getItem(locator.key) !== encoded) throw new Error('Original cache changed during export; retry a new snapshot')
    return result
  } finally { plaintext?.fill(0) }
}
