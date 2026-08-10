import type { MessageRecord, RunEvent, RunRecord } from '../runtime/contracts'
import { fromBase64, mobileSecureGet, mobileSecureSet, toBase64 } from './mobileSecure'

const STORAGE_KEY = 'open-cowork-mobile-cache-v1'
const KEY_NAMESPACE = 'offline_cache'
const KEY_NAME = 'aes_gcm_key_v1'
const AAD = new TextEncoder().encode('open-cowork-mobile-cache-v1')

export type MobileOutboxOperation = {
  id: string
  kind: 'cancel_run'
  runId: string
  createdAt: string
  attempts: number
  lastError?: string
}

export type MobileOfflineState = {
  schemaVersion: 1
  runs: RunRecord[]
  events: Record<string, RunEvent[]>
  messages: Record<string, MessageRecord[]>
  outbox: MobileOutboxOperation[]
  updatedAt: string
}

export const EMPTY_MOBILE_OFFLINE_STATE: MobileOfflineState = {
  schemaVersion: 1,
  runs: [],
  events: {},
  messages: {},
  outbox: [],
  updatedAt: new Date(0).toISOString(),
}

export async function loadMobileOfflineState(): Promise<MobileOfflineState> {
  const encoded = localStorage.getItem(STORAGE_KEY)
  if (!encoded) return { ...EMPTY_MOBILE_OFFLINE_STATE, events: {}, messages: {}, outbox: [] }
  const key = await cacheKey(false)
  if (!key) return { ...EMPTY_MOBILE_OFFLINE_STATE, events: {}, messages: {}, outbox: [] }
  try {
    const packed = fromBase64(encoded)
    if (packed.length < 13) throw new Error('mobile cache is truncated')
    const plaintext = await crypto.subtle.decrypt(
      { name: 'AES-GCM', iv: packed.slice(0, 12), additionalData: AAD },
      key,
      packed.slice(12),
    )
    const parsed = JSON.parse(new TextDecoder().decode(plaintext)) as MobileOfflineState
    if (parsed.schemaVersion !== 1 || !Array.isArray(parsed.runs) || !Array.isArray(parsed.outbox)) {
      throw new Error('mobile cache schema is invalid')
    }
    return { ...parsed, messages: parsed.messages ?? {} }
  } catch (error) {
    localStorage.removeItem(STORAGE_KEY)
    throw new Error(
      `Encrypted mobile cache could not be opened: ${error instanceof Error ? error.message : String(error)}`,
      { cause: error },
    )
  }
}

export async function saveMobileOfflineState(state: MobileOfflineState): Promise<void> {
  const key = await cacheKey(true)
  if (!key) throw new Error('mobile cache key is unavailable')
  const iv = crypto.getRandomValues(new Uint8Array(12))
  const plaintext = new TextEncoder().encode(JSON.stringify({
    ...state,
    schemaVersion: 1,
    updatedAt: new Date().toISOString(),
  } satisfies MobileOfflineState))
  const ciphertext = await crypto.subtle.encrypt(
    { name: 'AES-GCM', iv, additionalData: AAD },
    key,
    plaintext,
  )
  const packed = new Uint8Array(iv.length + ciphertext.byteLength)
  packed.set(iv)
  packed.set(new Uint8Array(ciphertext), iv.length)
  localStorage.setItem(STORAGE_KEY, toBase64(packed))
}

async function cacheKey(create: boolean): Promise<CryptoKey | null> {
  let encoded = await mobileSecureGet(KEY_NAMESPACE, KEY_NAME)
  if (!encoded && create) {
    const generated = crypto.getRandomValues(new Uint8Array(32))
    encoded = toBase64(generated)
    await mobileSecureSet(KEY_NAMESPACE, KEY_NAME, encoded)
  }
  if (!encoded) return null
  return crypto.subtle.importKey(
    'raw',
    fromBase64(encoded) as BufferSource,
    'AES-GCM',
    false,
    ['encrypt', 'decrypt'],
  )
}
