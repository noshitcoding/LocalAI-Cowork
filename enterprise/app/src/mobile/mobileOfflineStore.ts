import type { CreateThreadMessageRequest, MessageRecord, RunEvent, RunRecord } from '../runtime/contracts'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import { fromBase64, mobileSecureGet, mobileSecureSet, toBase64 } from './mobileSecure'

const STORAGE_KEY = 'open-cowork-mobile-cache-v1'
const V2_STORAGE_PREFIX = 'open-cowork-mobile-cache-v2:'
const KEY_NAMESPACE = 'offline_cache'
const KEY_NAME = 'aes_gcm_key_v1'
const AAD = new TextEncoder().encode('open-cowork-mobile-cache-v1')
let mobileOfflineSaveQueue: Promise<void> = Promise.resolve()

type MobileOutboxBase = {
  id: string
  createdAt: string
  attempts: number
  lastError?: string
}

export type MobileOutboxOperation = MobileOutboxBase & ({
  kind: 'cancel_run'
  runId: string
} | {
  kind: 'thread_message'
  threadId: string
  request: CreateThreadMessageRequest
})

export type MobileOfflineState = {
  schemaVersion: 1 | 2
  binding?: MobileCacheBinding | null
  legacy?: { runs: RunRecord[]; events: Record<string, RunEvent[]>; messages: Record<string, MessageRecord[]>; outbox: MobileOutboxOperation[] }
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
export type MobileCacheBinding = { serverOrigin: string; userId: string }
export function mobileCacheBinding(server: string, userId: string): MobileCacheBinding {
  const origin = new URL(server)
  if (!['https:', 'http:'].includes(origin.protocol) || origin.username || origin.password || origin.search || origin.hash || !userId) throw new Error('Invalid mobile cache identity')
  return { serverOrigin: origin.origin, userId }
}
export function mobileCacheMatches(state: MobileOfflineState, binding: MobileCacheBinding): boolean {
  return state.schemaVersion === 2 && state.binding?.serverOrigin === binding.serverOrigin && state.binding.userId === binding.userId
}
async function storageContext(binding?: MobileCacheBinding | null): Promise<{ key: string; aad: Uint8Array }> {
  if (!binding) return { key: STORAGE_KEY, aad: AAD }
  const aad = new TextEncoder().encode(JSON.stringify({ format: 'open-cowork-mobile-cache-v2', ...binding }))
  const hash = toBase64(new Uint8Array(await crypto.subtle.digest('SHA-256', aad))).replaceAll('/', '_').replaceAll('+', '-').replaceAll('=', '')
  return { key: V2_STORAGE_PREFIX + hash, aad }
}

export function createOfflineThreadMessageOperation(
  run: RunRecord,
  prompt: string,
  id = crypto.randomUUID(),
  createdAt = new Date().toISOString(),
): MobileOutboxOperation {
  const content = prompt.trim()
  if (!content) throw new Error('Offline replies must not be empty')
  return {
    id,
    kind: 'thread_message',
    threadId: run.spec.thread_id,
    request: {
      content: { text: content },
      run: {
        thread_id: run.spec.thread_id,
        project_id: run.spec.project_id,
        project_revision: run.spec.project.revision,
        project_privacy: run.spec.project_privacy,
        task: null,
        executor_target: run.spec.executor_target,
        required_capabilities: run.spec.required_capabilities,
        input: { prompt: content },
        model_profile_id: run.spec.model_profile_id ?? null,
        snapshot_id: null,
        idempotency_key: id,
      },
    },
    createdAt,
    attempts: 0,
  }
}

export async function flushMobileOutbox(
  client: Pick<RemoteRuntimeClient, 'cancelRun' | 'createThreadMessage'>,
  operations: MobileOutboxOperation[],
): Promise<{ remaining: MobileOutboxOperation[]; createdRuns: RunRecord[] }> {
  const remaining: MobileOutboxOperation[] = []
  const createdRuns: RunRecord[] = []
  for (const [index, operation] of operations.entries()) {
    try {
      if (operation.kind === 'cancel_run') await client.cancelRun(operation.runId)
      else createdRuns.push((await client.createThreadMessage(operation.threadId, operation.request)).run)
    } catch (error) {
      remaining.push({
        ...operation,
        attempts: operation.attempts + 1,
        lastError: error instanceof Error ? error.message : String(error),
      })
      remaining.push(...operations.slice(index + 1))
      break
    }
  }
  return { remaining, createdRuns }
}

export function mergeMobileOutboxAfterFlush(
  current: MobileOutboxOperation[],
  flushedSnapshot: MobileOutboxOperation[],
  remaining: MobileOutboxOperation[],
): MobileOutboxOperation[] {
  const flushedIds = new Set(flushedSnapshot.map((operation) => operation.id))
  const remainingById = new Map(remaining.map((operation) => [operation.id, operation]))
  return current.flatMap((operation) => {
    if (!flushedIds.has(operation.id)) return [operation]
    const retained = remainingById.get(operation.id)
    return retained ? [retained] : []
  })
}

export async function loadMobileOfflineState(binding?: MobileCacheBinding): Promise<MobileOfflineState> {
  const context = await storageContext(binding)
  const own = localStorage.getItem(context.key)
  const encoded = own ?? localStorage.getItem(STORAGE_KEY)
  if (!encoded) return { ...EMPTY_MOBILE_OFFLINE_STATE, schemaVersion: 2, binding: binding ?? null, events: {}, messages: {}, outbox: [] }
  const key = await cacheKey(false)
  if (!key) throw new Error('The encrypted cache key is unavailable. Existing encrypted bytes were preserved.')
  try {
    const packed = fromBase64(encoded)
    if (packed.length < 13) throw new Error('mobile cache is truncated')
    const plaintext = await crypto.subtle.decrypt(
      { name: 'AES-GCM', iv: packed.slice(0, 12), additionalData: (own ? context.aad : AAD) as BufferSource },
      key,
      packed.slice(12),
    )
    const parsed = JSON.parse(new TextDecoder().decode(plaintext)) as MobileOfflineState
    if (![1, 2].includes(parsed.schemaVersion) || !Array.isArray(parsed.runs) || !Array.isArray(parsed.outbox)) {
      throw new Error('mobile cache schema is invalid')
    }
    if (binding && own && mobileCacheMatches(parsed, binding)) return { ...parsed, messages: parsed.messages ?? {} }
    const migrated: MobileOfflineState = { ...EMPTY_MOBILE_OFFLINE_STATE, schemaVersion: 2, binding: binding ?? null, runs: [], events: {}, messages: {}, outbox: [], legacy: parsed.legacy ?? { runs: parsed.runs, events: parsed.events ?? {}, messages: parsed.messages ?? {}, outbox: parsed.outbox } }
    if (binding) await saveMobileOfflineState(migrated)
    return migrated
  } catch (error) {
    // Keep encrypted source bytes after failed migration or missing secure keys.
    throw new Error(
      `Encrypted mobile cache could not be opened: ${error instanceof Error ? error.message : String(error)}`,
      { cause: error },
    )
  }
}

async function writeMobileOfflineState(plaintext: Uint8Array, binding?: MobileCacheBinding | null): Promise<void> {
  const key = await cacheKey(true)
  if (!key) throw new Error('mobile cache key is unavailable')
  const iv = crypto.getRandomValues(new Uint8Array(12))
  const context = await storageContext(binding)
  const ciphertext = await crypto.subtle.encrypt(
    { name: 'AES-GCM', iv, additionalData: context.aad as BufferSource },
    key,
    plaintext as BufferSource,
  )
  const packed = new Uint8Array(iv.length + ciphertext.byteLength)
  packed.set(iv)
  packed.set(new Uint8Array(ciphertext), iv.length)
  localStorage.setItem(context.key, toBase64(packed))
}

export function saveMobileOfflineState(state: MobileOfflineState): Promise<void> {
  if (!state.binding) return Promise.reject(new Error('Open a verified account before writing or replaying the mobile cache'))
  const plaintext = new TextEncoder().encode(JSON.stringify({
    ...state,
    schemaVersion: 2,
    updatedAt: new Date().toISOString(),
  } satisfies MobileOfflineState))
  const save = mobileOfflineSaveQueue
    .catch(() => undefined)
    .then(() => writeMobileOfflineState(plaintext, state.binding))
  mobileOfflineSaveQueue = save.then(() => undefined, () => undefined)
  return save
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
