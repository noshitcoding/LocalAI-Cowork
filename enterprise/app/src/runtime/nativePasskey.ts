import { deleteCredential, getCredential, setCredential } from '../security/credentialVault'
import { createPkcePair, RemoteAuthClient, type AuthTokens } from './authClient'
import { normalizeServerUrl } from './runtimeClient'

const CALLBACK_URI = 'open-cowork://auth/callback'
const AUTHORIZATION_LIFETIME_MS = 5 * 60 * 1_000
const PENDING_LOCATOR = {
  scope: 'remote_server' as const,
  ownerId: 'primary',
  field: 'native_passkey_authorization',
}

type PendingNativePasskey = {
  serverUrl: string
  email: string
  deviceId: string
  verifier: string
  state: string
  expiresAt: number
}

export type NativePasskeyCallback = { code: string; state: string }

function randomBase64Url(byteLength: number): string {
  const bytes = crypto.getRandomValues(new Uint8Array(byteLength))
  let binary = ''
  for (const byte of bytes) binary += String.fromCharCode(byte)
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/g, '')
}

export function nativePasskeyAvailable(): boolean {
  return typeof window !== 'undefined'
    && Boolean((window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__)
}

export function parseNativePasskeyCallback(value: string): NativePasskeyCallback | null {
  try {
    const url = new URL(value)
    const expected = new URL(CALLBACK_URI)
    if (url.protocol !== expected.protocol || url.hostname !== expected.hostname || url.pathname !== expected.pathname) {
      return null
    }
    const code = url.searchParams.get('code') ?? ''
    const state = url.searchParams.get('state') ?? ''
    if (!/^[A-Za-z0-9_-]{43,128}$/.test(code) || !/^[A-Za-z0-9_-]{43,128}$/.test(state)) {
      return null
    }
    return { code, state }
  } catch {
    return null
  }
}

export function nativePasskeyAuthorizationUrl(
  serverUrl: string,
  request: { email: string; deviceId: string; challenge: string; state: string },
): string {
  const url = new URL(`${normalizeServerUrl(serverUrl)}/api/v1/auth/native/passkey/authorize`)
  url.hash = new URLSearchParams({
    email: request.email,
    device_id: request.deviceId,
    code_challenge: request.challenge,
    state: request.state,
  }).toString()
  return url.toString()
}

function parsePending(value: string | null): PendingNativePasskey | null {
  if (!value) return null
  try {
    const pending = JSON.parse(value) as Partial<PendingNativePasskey>
    if (typeof pending.serverUrl !== 'string'
      || typeof pending.email !== 'string'
      || typeof pending.deviceId !== 'string'
      || typeof pending.verifier !== 'string'
      || typeof pending.state !== 'string'
      || typeof pending.expiresAt !== 'number') return null
    return pending as PendingNativePasskey
  } catch { return null }
}

function callbackFromUrls(urls: string[] | null, expectedState: string): NativePasskeyCallback | null {
  for (const value of urls ?? []) {
    const callback = parseNativePasskeyCallback(String(value))
    if (callback?.state === expectedState) return callback
  }
  return null
}

async function waitForCallback(
  expectedState: string,
  authorizeUrl: string,
): Promise<NativePasskeyCallback> {
  const [{ getCurrent, onOpenUrl }, { openUrl }] = await Promise.all([
    import('@tauri-apps/plugin-deep-link'),
    import('@tauri-apps/plugin-opener'),
  ])
  let accept: ((callback: NativePasskeyCallback) => void) | null = null
  const callbackPromise = new Promise<NativePasskeyCallback>((resolve) => { accept = resolve })
  const unlisten = await onOpenUrl((urls) => {
    const callback = callbackFromUrls(urls, expectedState)
    if (callback) accept?.(callback)
  })
  try {
    const current = callbackFromUrls(await getCurrent(), expectedState)
    if (current) return current
    await openUrl(authorizeUrl)
    return await Promise.race([
      callbackPromise,
      new Promise<never>((_resolve, reject) => {
        window.setTimeout(() => reject(new Error('Passkey authorization expired')), AUTHORIZATION_LIFETIME_MS)
      }),
    ])
  } finally { unlisten() }
}

async function exchangePending(
  pending: PendingNativePasskey,
  callback: NativePasskeyCallback,
): Promise<AuthTokens> {
  if (pending.expiresAt <= Date.now() || callback.state !== pending.state) {
    throw new Error('Passkey authorization is invalid or expired')
  }
  return new RemoteAuthClient(pending.serverUrl)
    .exchangeNativeCode(callback.code, pending.verifier, pending.deviceId)
}

export async function loginWithNativePasskey(
  serverUrl: string,
  email: string,
  deviceId: string,
): Promise<AuthTokens> {
  if (!nativePasskeyAvailable()) throw new Error('Native passkey authorization is unavailable')
  const normalizedUrl = normalizeServerUrl(serverUrl)
  const normalizedEmail = email.trim().toLocaleLowerCase()
  const { verifier, challenge } = await createPkcePair()
  const pending: PendingNativePasskey = {
    serverUrl: normalizedUrl,
    email: normalizedEmail,
    deviceId,
    verifier,
    state: randomBase64Url(32),
    expiresAt: Date.now() + AUTHORIZATION_LIFETIME_MS,
  }
  await setCredential(PENDING_LOCATOR, JSON.stringify(pending))
  try {
    const authorizeUrl = nativePasskeyAuthorizationUrl(normalizedUrl, {
      email: normalizedEmail,
      deviceId,
      challenge,
      state: pending.state,
    })
    return await exchangePending(pending, await waitForCallback(pending.state, authorizeUrl))
  } finally { await deleteCredential(PENDING_LOCATOR) }
}

export async function completePendingNativePasskey(): Promise<AuthTokens | null> {
  if (!nativePasskeyAvailable()) return null
  const pending = parsePending(await getCredential(PENDING_LOCATOR))
  if (!pending) return null
  if (pending.expiresAt <= Date.now()) {
    await deleteCredential(PENDING_LOCATOR)
    return null
  }
  const { getCurrent } = await import('@tauri-apps/plugin-deep-link')
  const callback = callbackFromUrls(await getCurrent(), pending.state)
  if (!callback) return null
  try { return await exchangePending(pending, callback) }
  finally { await deleteCredential(PENDING_LOCATOR) }
}
