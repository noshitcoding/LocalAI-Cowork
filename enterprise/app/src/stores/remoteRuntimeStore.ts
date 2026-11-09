import { create } from 'zustand'
import { useServerProfilesStore } from './serverProfilesStore'

import { RemoteAuthClient, type AuthTokens } from '../runtime/authClient'
import { completePendingNativePasskey, loginWithNativePasskey } from '../runtime/nativePasskey'
import { completePendingOidc, loginWithOidc } from '../runtime/oidc'
import { normalizeServerUrl, RemoteRuntimeClient, RuntimeHttpError } from '../runtime/runtimeClient'
import { webauthnAvailableForOrigin } from '../runtime/webauthn'
import { deleteCredential, getCredential, setCredential } from '../security/credentialVault'

const CONNECTION_KEY = 'open-cowork-remote-connection-v1'
const DEVICE_KEY = 'open-cowork-remote-device-v1'
const BROWSER_SESSION_HINT_KEY = 'open-cowork-browser-session-v1'
const IS_WEB_APP = import.meta.env.MODE === 'web' || import.meta.env.VITE_COWORK_WEB === 'true'
const TOKEN_LOCATOR = {
  scope: 'remote_server' as const,
  ownerId: 'primary',
  field: 'refresh_token',
}

export type RemoteRuntimeStatus =
  | 'signed_out'
  | 'restoring'
  | 'authenticating'
  | 'second_factor_required'
  | 'authenticated'
  | 'error'

type StoredConnection = {
  serverUrl: string
  email: string
}

type RemoteRuntimeState = {
  serverUrl: string
  email: string
  status: RemoteRuntimeStatus
  userId: string | null
  accessToken: string | null
  accessExpiresAt: string | null
  error: string | null
  pendingPasswordLogin: { serverUrl: string; email: string; expiresAt: string } | null
  setConnection: (serverUrl: string, email: string) => void
  restore: () => Promise<boolean>
  login: (serverUrl: string, email: string, password: string, secondFactor?: string) => Promise<void>
  completePasswordLogin: (secondFactor: string) => Promise<void>
  cancelPasswordLogin: () => Promise<void>
  hasRecentPrimaryLogin: () => boolean
  loginFleet: (serverUrl: string, email: string, ticket: string) => Promise<void>
  bootstrap: (serverUrl: string, email: string, displayName: string, password: string, bootstrapToken: string) => Promise<void>
  acceptInvitation: (serverUrl: string, email: string, displayName: string, password: string, invitationToken: string) => Promise<void>
  registerOrganisationInvitation: (serverUrl: string, email: string, displayName: string, password: string, invitationToken: string) => Promise<void>
  loginPasskey: (serverUrl: string, email: string) => Promise<void>
  loginOidc: (serverUrl: string,organisation?:string) => Promise<void>
  linkOidc: (organisation?:string) => Promise<void>
  logout: () => Promise<void>
  clearError: () => void
}

type RefreshOperation = {
  generation: number
  serverUrl: string
  promise: Promise<string>
}

class StaleAuthOperationError extends Error {
  constructor() {
    super('Authentication changed while the request was in flight')
    this.name = 'StaleAuthOperationError'
  }
}

let authGeneration = 0
let refreshInFlight: RefreshOperation | null = null
let restoreInFlight: Promise<boolean> | null = null
let volatileDeviceId: string | null = null
const PASSWORD_CHALLENGE_MS = 5 * 60_000
const PRIMARY_LOGIN_MS = 10 * 60_000
type PendingPasswordLogin = { generation: number; serverUrl: string; email: string; password: string; expiresAt: number }
let pendingPasswordLogin: PendingPasswordLogin | null = null
let pendingPasswordTimer: ReturnType<typeof setTimeout> | null = null
let primaryLoginProof: { generation: number; serverUrl: string; userId: string; expiresAt: number } | null = null
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

function clearPendingPassword(): void {
  if (pendingPasswordTimer !== null) clearTimeout(pendingPasswordTimer)
  pendingPasswordTimer = null
  pendingPasswordLogin = null
}

function invalidateAuthOperations(): number {
  authGeneration += 1
  refreshInFlight = null
  restoreInFlight = null
  clearPendingPassword()
  primaryLoginProof = null
  return authGeneration
}

function assertAuthOperationCurrent(generation: number, serverUrl?: string): void {
  const state = useRemoteRuntimeStore.getState()
  if (generation !== authGeneration || (serverUrl !== undefined && state.serverUrl !== serverUrl)) {
    throw new StaleAuthOperationError()
  }
}

function readStoredConnection(): StoredConnection {
  if (typeof window === 'undefined') return { serverUrl: '', email: '' }
  try {
    const value = JSON.parse(window.localStorage.getItem(CONNECTION_KEY) ?? '{}') as Partial<StoredConnection>
    return {
      serverUrl: IS_WEB_APP
        ? window.location.origin
        : typeof value.serverUrl === 'string' ? value.serverUrl : '',
      email: typeof value.email === 'string' ? value.email : '',
    }
  } catch {
    return { serverUrl: IS_WEB_APP ? window.location.origin : '', email: '' }
  }
}

function selectedServerUrl(serverUrl: string): string {
  if (IS_WEB_APP && typeof window !== 'undefined') return window.location.origin
  return normalizeServerUrl(serverUrl)
}

function storeConnection(serverUrl: string, email: string): void {
  if (typeof window === 'undefined') return
  try { window.localStorage.setItem(CONNECTION_KEY, JSON.stringify({ serverUrl, email })) } catch { /* Session-only connection. */ }
}

export function remoteDeviceId(): string {
  if (typeof window === 'undefined') return volatileDeviceId ??= crypto.randomUUID()
  try {
    const existing = window.localStorage.getItem(DEVICE_KEY)?.trim()
    if (existing && UUID_PATTERN.test(existing)) {
      volatileDeviceId = existing
      return existing
    }
  } catch {
    return volatileDeviceId ??= crypto.randomUUID()
  }
  const created = crypto.randomUUID()
  volatileDeviceId = created
  try {
    window.localStorage.setItem(DEVICE_KEY, created)
  } catch {
    // Restricted storage still gets a stable identifier for this session.
  }
  return created
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

async function applyTokens(tokens: AuthTokens, context?: { generation: number; serverUrl: string }, interactivePrimary = false): Promise<void> {
  if (context) assertAuthOperationCurrent(context.generation, context.serverUrl)
  if (IS_WEB_APP) {
    // A stale token from an older web build must not survive migration to the
    // HttpOnly-cookie session model.
    await deleteCredential(TOKEN_LOCATOR)
    window.localStorage.setItem(BROWSER_SESSION_HINT_KEY, 'active')
  } else {
    if (!tokens.refresh_token) throw new Error('The server did not return a native refresh token')
    await setCredential(TOKEN_LOCATOR, tokens.refresh_token)
  }
  if (context) assertAuthOperationCurrent(context.generation, context.serverUrl)
  clearPendingPassword()
  if (interactivePrimary && context) primaryLoginProof = { ...context, userId: tokens.user_id, expiresAt: Date.now() + PRIMARY_LOGIN_MS }
  else if (primaryLoginProof && primaryLoginProof.userId !== tokens.user_id) primaryLoginProof = null
  useRemoteRuntimeStore.setState({
    status: 'authenticated',
    userId: tokens.user_id,
    accessToken: tokens.access_token,
    accessExpiresAt: tokens.access_expires_at,
    error: null,
    pendingPasswordLogin: null,
  })
}

async function refreshAccessToken(): Promise<string> {
  const state = useRemoteRuntimeStore.getState()
  if (!state.serverUrl) throw new Error('No Open Cowork server is configured')
  const generation = authGeneration
  const serverUrl = state.serverUrl
  if (refreshInFlight?.generation === generation && refreshInFlight.serverUrl === serverUrl) {
    return refreshInFlight.promise
  }
  const promise = (async () => {
    const refreshToken = IS_WEB_APP ? undefined : await getCredential(TOKEN_LOCATOR)
    assertAuthOperationCurrent(generation, serverUrl)
    if (!IS_WEB_APP && !refreshToken) {
      throw new Error('The server session has expired. Please sign in again.')
    }
    const tokens = await new RemoteAuthClient(serverUrl).refresh(refreshToken ?? undefined)
    await applyTokens(tokens, { generation, serverUrl })
    return tokens.access_token
  })().catch(error => {
    // A rejected request from an old server must not sign out a newer session.
    assertAuthOperationCurrent(generation, serverUrl)
    throw error
  })
  const operation = { generation, serverUrl, promise }
  refreshInFlight = operation
  try {
    return await promise
  } finally {
    if (refreshInFlight === operation) refreshInFlight = null
  }
}

export async function remoteAccessToken(): Promise<string> {
  const state = useRemoteRuntimeStore.getState()
  if (pendingPasswordLogin) throw new Error('Complete or cancel the pending password sign-in first')
  const expiresAt = state.accessExpiresAt ? Date.parse(state.accessExpiresAt) : 0
  if (state.accessToken && expiresAt > Date.now() + 30_000) return state.accessToken
  try {
    return await refreshAccessToken()
  } catch (error) {
    if (error instanceof StaleAuthOperationError) throw error
    useRemoteRuntimeStore.setState({
      status: 'signed_out',
      userId: null,
      accessToken: null,
      accessExpiresAt: null,
      error: errorMessage(error),
    })
    throw error
  }
}

export function remoteTokenProvider(): () => Promise<string> {
  const generation = authGeneration
  const serverUrl = useRemoteRuntimeStore.getState().serverUrl
  return async () => {
    assertAuthOperationCurrent(generation, serverUrl)
    const token = await remoteAccessToken()
    assertAuthOperationCurrent(generation, serverUrl)
    return token
  }
}

export function remoteRuntimeClient(): RemoteRuntimeClient {
  const serverUrl = useRemoteRuntimeStore.getState().serverUrl
  if (!serverUrl) throw new Error('No Open Cowork server is configured')
  return new RemoteRuntimeClient({ baseUrl: serverUrl, accessToken: remoteTokenProvider() })
}

async function authenticate(serverUrl: string, email: string, operation: (client: RemoteAuthClient, url: string) => Promise<AuthTokens | null>, options: { interactivePrimary?: boolean; password?: string } = { interactivePrimary: true }): Promise<void> {
  const url = selectedServerUrl(serverUrl)
  const account = email.trim().toLowerCase()
  const generation = invalidateAuthOperations()
  useRemoteRuntimeStore.setState({ serverUrl: url, email: account, status: 'authenticating', userId: null, accessToken: null, accessExpiresAt: null, error: null, pendingPasswordLogin: null })
  try {
    await deleteCredential(TOKEN_LOCATOR)
    assertAuthOperationCurrent(generation, url)
    // Persist the destination before a possible native OIDC/passkey redirect.
    storeConnection(url, account)
    const tokens = await operation(new RemoteAuthClient(url), url)
    assertAuthOperationCurrent(generation, url)
    if (tokens) {
      await applyTokens(tokens, { generation, serverUrl: url }, options.interactivePrimary !== false)
      if (!IS_WEB_APP) useServerProfilesStore.getState().save(url, account)
    }
  } catch (error) {
    if (generation === authGeneration && options.password !== undefined && error instanceof RuntimeHttpError && error.status === 401 && error.code === 'unauthorized' && error.message === 'TOTP or recovery code is required') {
      const expiresAt = Date.now() + PASSWORD_CHALLENGE_MS
      pendingPasswordLogin = { generation, serverUrl: url, email: account, password: options.password, expiresAt }
      pendingPasswordTimer = setTimeout(() => {
        if (pendingPasswordLogin?.generation !== generation) return
        invalidateAuthOperations()
        if (IS_WEB_APP) window.localStorage.removeItem(BROWSER_SESSION_HINT_KEY)
        useRemoteRuntimeStore.setState({ status: 'signed_out', userId: null, accessToken: null, accessExpiresAt: null, pendingPasswordLogin: null, error: 'Password confirmation expired. Sign in again.' })
      }, PASSWORD_CHALLENGE_MS)
      useRemoteRuntimeStore.setState({ status: 'second_factor_required', pendingPasswordLogin: { serverUrl: url, email: account, expiresAt: new Date(expiresAt).toISOString() }, error: null })
      return
    }
    if (generation === authGeneration) useRemoteRuntimeStore.setState({ status: 'error', error: errorMessage(error) })
    throw error
  }
}

const initialConnection = readStoredConnection()

export const useRemoteRuntimeStore = create<RemoteRuntimeState>((set, get) => ({
  ...initialConnection,
  status: 'signed_out',
  userId: null,
  accessToken: null,
  accessExpiresAt: null,
  error: null,
  pendingPasswordLogin: null,

  setConnection: (serverUrl, email) => {
    const normalizedUrl = selectedServerUrl(serverUrl)
    invalidateAuthOperations()
    void deleteCredential(TOKEN_LOCATOR)
    storeConnection(normalizedUrl, email)
    set({
      serverUrl: normalizedUrl,
      email,
      status: 'signed_out',
      userId: null,
      accessToken: null,
      accessExpiresAt: null,
      error: null,
      pendingPasswordLogin: null,
    })
  },

  restore: async () => {
    if (get().status === 'authenticated') return true
    if (pendingPasswordLogin) return false
    if (restoreInFlight) return restoreInFlight
    primaryLoginProof = null
    const context = { generation: authGeneration, serverUrl: get().serverUrl }
    const promise = (async () => {
      set({ status: 'restoring', error: null })
      try {
        const callbackTokens = await completePendingOidc(context.serverUrl) ?? await completePendingNativePasskey()
        assertAuthOperationCurrent(context.generation, context.serverUrl)
        if (callbackTokens) {
          // These are validated completions of an explicit interactive flow,
          // unlike restoring a cookie/refresh token below.
          await applyTokens(callbackTokens, context, true)
          return true
        }
        if (!get().serverUrl) {
          set({ status: 'signed_out' })
          return false
        }
        if (IS_WEB_APP && !window.localStorage.getItem(BROWSER_SESSION_HINT_KEY)) {
          set({ status: 'signed_out' })
          return false
        }
        await refreshAccessToken()
        return true
      } catch (error) {
        if (context.generation !== authGeneration || error instanceof StaleAuthOperationError) return false
        if (IS_WEB_APP) window.localStorage.removeItem(BROWSER_SESSION_HINT_KEY)
        set({
          status: 'signed_out',
          userId: null,
          accessToken: null,
          accessExpiresAt: null,
          error: errorMessage(error),
        })
        return false
      }
    })()
    restoreInFlight = promise
    try {
      return await promise
    } finally {
      if (restoreInFlight === promise) restoreInFlight = null
    }
  },

  login: (serverUrl, email, password, secondFactor) => authenticate(serverUrl, email, (client) => client.loginPkce({
    email: email.trim().toLowerCase(), password, device_id: remoteDeviceId(), second_factor: secondFactor?.trim() || null,
  }), { interactivePrimary: true, password: secondFactor?.trim() ? undefined : password }),

  completePasswordLogin: async secondFactor => {
    const pending = pendingPasswordLogin
    if (!pending) throw new Error('Password confirmation expired. Sign in again.')
    if (pending.expiresAt <= Date.now()) {
      await get().cancelPasswordLogin()
      throw new Error('Password confirmation expired. Sign in again.')
    }
    assertAuthOperationCurrent(pending.generation, pending.serverUrl)
    if (get().status !== 'second_factor_required' || get().email !== pending.email) throw new Error('Password sign-in is no longer awaiting a second factor')
    const code = secondFactor.trim()
    if (!code || code.length > 128) throw new Error('Enter an authenticator or recovery code')
    set({ status: 'authenticating', error: null })
    try {
      // The existing endpoint rechecks the password and factor together. The
      // first-step response is a UI signal, never an authentication grant.
      const tokens = await new RemoteAuthClient(pending.serverUrl).loginPkce({ email: pending.email, password: pending.password, device_id: remoteDeviceId(), second_factor: code })
      assertAuthOperationCurrent(pending.generation, pending.serverUrl)
      await applyTokens(tokens, { generation: pending.generation, serverUrl: pending.serverUrl }, true)
      if (!IS_WEB_APP) useServerProfilesStore.getState().save(pending.serverUrl, pending.email)
    } catch (error) {
      if (pending.generation === authGeneration) {
        if (error instanceof RuntimeHttpError && error.status === 401 && error.message === 'invalid email or password') {
          clearPendingPassword()
          set({ status: 'error', pendingPasswordLogin: null, error: errorMessage(error) })
        } else set({ status: 'second_factor_required', error: errorMessage(error) })
      }
      throw error
    }
  },

  cancelPasswordLogin: () => get().logout(),

  hasRecentPrimaryLogin: () => {
    const state = get(), proof = primaryLoginProof
    return !!proof && proof.generation === authGeneration && proof.serverUrl === state.serverUrl && proof.userId === state.userId && proof.expiresAt > Date.now() && state.status === 'authenticated'
  },

  loginFleet: (serverUrl, email, ticket) => authenticate(serverUrl, email, client => client.loginFleet(ticket, remoteDeviceId()), { interactivePrimary: false }),

  bootstrap: (serverUrl, email, displayName, password, bootstrapToken) => authenticate(serverUrl, email, (client) => client.bootstrap({
    email: email.trim().toLowerCase(), display_name: displayName.trim(), password, bootstrap_token: bootstrapToken.trim(), device_id: remoteDeviceId(),
  })),

  acceptInvitation: (serverUrl, email, displayName, password, invitationToken) => authenticate(serverUrl, email, (client) => client.acceptInvitation({
    token: invitationToken.trim(), display_name: displayName.trim(), password, device_id: remoteDeviceId(),
  })),
  registerOrganisationInvitation: (serverUrl, email, displayName, password, invitationToken) => authenticate(serverUrl, email, client => client.registerOrganisationInvitation({
    token: invitationToken.trim(), display_name: displayName.trim(), password, device_id: remoteDeviceId(),
  })),

  loginPasskey: (serverUrl, email) => authenticate(serverUrl, email, (client, url) => webauthnAvailableForOrigin(url)
    ? client.loginPasskey(email.trim().toLowerCase(), remoteDeviceId())
    : loginWithNativePasskey(url, email.trim().toLowerCase(), remoteDeviceId())),

  loginOidc: (serverUrl,organisation) => authenticate(serverUrl, get().email, (_client, url) => loginWithOidc(url, remoteDeviceId(),undefined,organisation)),

  linkOidc: async (organisation) => {
    const state = get()
    if (state.status !== 'authenticated' || !state.serverUrl) {
      throw new Error('Sign in before linking an OIDC identity')
    }
    const context = { generation: authGeneration, serverUrl: state.serverUrl }
    try {
      const tokens = await loginWithOidc(
        state.serverUrl,
        remoteDeviceId(),
        await remoteAccessToken(),
        organisation,
      )
      if (tokens) await applyTokens(tokens, context, true)
    } catch (error) {
      if (context.generation === authGeneration) set({ error: errorMessage(error) })
      throw error
    }
  },

  logout: async () => {
    const { serverUrl, accessToken } = get()
    invalidateAuthOperations()
    set({ status: 'signed_out', userId: null, accessToken: null, accessExpiresAt: null, error: null, pendingPasswordLogin: null })
    if (IS_WEB_APP) window.localStorage.removeItem(BROWSER_SESSION_HINT_KEY)
    await deleteCredential(TOKEN_LOCATOR)
    if (serverUrl && accessToken) {
      try {
        await new RemoteAuthClient(serverUrl).logout(accessToken)
      } catch {
        // The local credential is already removed. A stale or offline server session
        // will expire or can be revoked from the device management screen.
      }
    }
  },

  clearError: () => {
    if (pendingPasswordLogin) { set({ error: null }); return }
    if (get().status === 'restoring') {
      get().setConnection(get().serverUrl, get().email)
      return
    }
    set({ error: null, status: get().accessToken ? 'authenticated' : 'signed_out' })
  },
}))

export function resetRemoteRuntimeStoreForTests(): void {
  clearPendingPassword()
  primaryLoginProof = null
  authGeneration = 0
  refreshInFlight = null
  restoreInFlight = null
  volatileDeviceId = null
  useRemoteRuntimeStore.setState({
    serverUrl: '',
    email: '',
    status: 'signed_out',
    userId: null,
    accessToken: null,
    accessExpiresAt: null,
    error: null,
    pendingPasswordLogin: null,
  })
}
