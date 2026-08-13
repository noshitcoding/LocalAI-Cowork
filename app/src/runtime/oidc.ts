import { deleteCredential, getCredential, setCredential } from '../security/credentialVault'
import { createPkcePair, RemoteAuthClient, type AuthTokens } from './authClient'
import { nativePasskeyAvailable, parseNativePasskeyCallback } from './nativePasskey'
import { normalizeServerUrl } from './runtimeClient'

const NATIVE_CALLBACK = 'open-cowork://auth/callback'
const WEB_CALLBACK_PATH = '/auth/callback'
const SESSION_KEY = 'open-cowork-oidc-pending-v1'
const LIFETIME_MS = 5 * 60 * 1_000
const PENDING_LOCATOR = {
  scope: 'remote_server' as const,
  ownerId: 'primary',
  field: 'oidc_authorization',
}

type PendingOidc = {
  serverUrl: string
  deviceId: string
  verifier: string
  state: string
  expiresAt: number
  native: boolean
}

export type OidcCallback = { code: string; state: string }

function randomBase64Url(byteLength: number): string {
  const bytes = crypto.getRandomValues(new Uint8Array(byteLength))
  let binary = ''
  for (const byte of bytes) binary += String.fromCharCode(byte)
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/g, '')
}

function parsePending(value: string | null): PendingOidc | null {
  if (!value) return null
  try {
    const parsed = JSON.parse(value) as Partial<PendingOidc>
    if (typeof parsed.serverUrl !== 'string'
      || typeof parsed.deviceId !== 'string'
      || typeof parsed.verifier !== 'string'
      || typeof parsed.state !== 'string'
      || typeof parsed.expiresAt !== 'number'
      || typeof parsed.native !== 'boolean') return null
    return parsed as PendingOidc
  } catch { return null }
}

async function storePending(pending: PendingOidc): Promise<void> {
  const serialized = JSON.stringify(pending)
  if (pending.native) await setCredential(PENDING_LOCATOR, serialized)
  else window.sessionStorage.setItem(SESSION_KEY, serialized)
}

async function loadPending(): Promise<PendingOidc | null> {
  const native = nativePasskeyAvailable()
  return parsePending(native ? await getCredential(PENDING_LOCATOR) : window.sessionStorage.getItem(SESSION_KEY))
}

async function clearPending(native: boolean): Promise<void> {
  if (native) await deleteCredential(PENDING_LOCATOR)
  else window.sessionStorage.removeItem(SESSION_KEY)
}

export function parseOidcWebCallback(value: string, expectedOrigin: string): OidcCallback | null {
  try {
    const url = new URL(value)
    if (url.origin !== expectedOrigin || url.pathname !== WEB_CALLBACK_PATH) return null
    const code = url.searchParams.get('code') ?? ''
    const state = url.searchParams.get('state') ?? ''
    if (!/^[A-Za-z0-9_-]{43,128}$/.test(code) || !/^[A-Za-z0-9_-]{43,128}$/.test(state)) return null
    return { code, state }
  } catch { return null }
}

function currentWebCallback(): OidcCallback | null {
  if (typeof window === 'undefined') return null
  return parseOidcWebCallback(window.location.href, window.location.origin)
}

function nativeCallbackFromUrls(urls: string[] | null, state: string): OidcCallback | null {
  for (const value of urls ?? []) {
    const callback = parseNativePasskeyCallback(String(value))
    if (callback?.state === state) return callback
  }
  return null
}

async function waitForNativeCallback(state: string, authorizationUrl: string): Promise<OidcCallback> {
  const [{ getCurrent, onOpenUrl }, { openUrl }] = await Promise.all([
    import('@tauri-apps/plugin-deep-link'),
    import('@tauri-apps/plugin-opener'),
  ])
  let accept: ((callback: OidcCallback) => void) | null = null
  const callbackPromise = new Promise<OidcCallback>((resolve) => { accept = resolve })
  const unlisten = await onOpenUrl((urls) => {
    const callback = nativeCallbackFromUrls(urls, state)
    if (callback) accept?.(callback)
  })
  try {
    const current = nativeCallbackFromUrls(await getCurrent(), state)
    if (current) return current
    await openUrl(authorizationUrl)
    return await Promise.race([
      callbackPromise,
      new Promise<never>((_resolve, reject) => {
        window.setTimeout(() => reject(new Error('OIDC authorization expired')), LIFETIME_MS)
      }),
    ])
  } finally { unlisten() }
}

async function exchange(pending: PendingOidc, callback: OidcCallback): Promise<AuthTokens> {
  if (pending.expiresAt <= Date.now() || callback.state !== pending.state) {
    throw new Error('OIDC authorization is invalid or expired')
  }
  return new RemoteAuthClient(pending.serverUrl)
    .exchangeNativeCode(callback.code, pending.verifier, pending.deviceId)
}

export async function oidcEnabled(serverUrl: string): Promise<boolean> {
  if (!serverUrl.trim()) return false
  try { return await new RemoteAuthClient(serverUrl).oidcEnabled() }
  catch { return false }
}

export async function loginWithOidc(
  serverUrl: string,
  deviceId: string,
  linkAccessToken?: string,
): Promise<AuthTokens | null> {
  const normalizedUrl = normalizeServerUrl(serverUrl)
  const native = nativePasskeyAvailable()
  const { verifier, challenge } = await createPkcePair()
  const pending: PendingOidc = {
    serverUrl: normalizedUrl,
    deviceId,
    verifier,
    state: randomBase64Url(32),
    expiresAt: Date.now() + LIFETIME_MS,
    native,
  }
  await storePending(pending)
  try {
    const authorization = await new RemoteAuthClient(normalizedUrl).startOidcAuthorization({
      device_id: deviceId,
      code_challenge: challenge,
      client_state: pending.state,
      redirect_uri: native ? NATIVE_CALLBACK : `${window.location.origin}${WEB_CALLBACK_PATH}`,
    }, linkAccessToken)
    if (!native) {
      window.location.assign(authorization.authorization_url)
      return null
    }
    const tokens = await exchange(
      pending,
      await waitForNativeCallback(pending.state, authorization.authorization_url),
    )
    await clearPending(true)
    return tokens
  } catch (error) {
    await clearPending(native)
    throw error
  }
}

export async function completePendingOidc(): Promise<AuthTokens | null> {
  const pending = await loadPending()
  if (!pending) return null
  if (pending.expiresAt <= Date.now()) {
    await clearPending(pending.native)
    return null
  }
  const callback = pending.native
    ? nativeCallbackFromUrls(
        await import('@tauri-apps/plugin-deep-link').then(({ getCurrent }) => getCurrent()),
        pending.state,
      )
    : currentWebCallback()
  if (!callback) return null
  try { return await exchange(pending, callback) }
  finally { await clearPending(pending.native) }
}
