import { create } from 'zustand'

import { RemoteAuthClient, type AuthTokens } from '../runtime/authClient'
import { completePendingNativePasskey, loginWithNativePasskey } from '../runtime/nativePasskey'
import { completePendingOidc, loginWithOidc } from '../runtime/oidc'
import { normalizeServerUrl, RemoteRuntimeClient } from '../runtime/runtimeClient'
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
  setConnection: (serverUrl: string, email: string) => void
  restore: () => Promise<boolean>
  login: (serverUrl: string, email: string, password: string, secondFactor?: string) => Promise<void>
  bootstrap: (serverUrl: string, email: string, displayName: string, password: string, bootstrapToken: string) => Promise<void>
  acceptInvitation: (serverUrl: string, email: string, displayName: string, password: string, invitationToken: string) => Promise<void>
  loginPasskey: (serverUrl: string, email: string) => Promise<void>
  loginOidc: (serverUrl: string) => Promise<void>
  linkOidc: () => Promise<void>
  logout: () => Promise<void>
  clearError: () => void
}

let refreshInFlight: Promise<string> | null = null
let restoreInFlight: Promise<boolean> | null = null

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
    return { serverUrl: '', email: '' }
  }
}

function selectedServerUrl(serverUrl: string): string {
  if (IS_WEB_APP && typeof window !== 'undefined') return window.location.origin
  return normalizeServerUrl(serverUrl)
}

function storeConnection(serverUrl: string, email: string): void {
  if (typeof window === 'undefined') return
  window.localStorage.setItem(CONNECTION_KEY, JSON.stringify({ serverUrl, email }))
}

export function remoteDeviceId(): string {
  if (typeof window === 'undefined') return crypto.randomUUID()
  const existing = window.localStorage.getItem(DEVICE_KEY)
  if (existing) return existing
  const created = crypto.randomUUID()
  window.localStorage.setItem(DEVICE_KEY, created)
  return created
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

async function applyTokens(tokens: AuthTokens): Promise<void> {
  if (IS_WEB_APP) {
    // A stale token from an older web build must not survive migration to the
    // HttpOnly-cookie session model.
    await deleteCredential(TOKEN_LOCATOR)
    window.localStorage.setItem(BROWSER_SESSION_HINT_KEY, 'active')
  } else {
    if (!tokens.refresh_token) throw new Error('The server did not return a native refresh token')
    await setCredential(TOKEN_LOCATOR, tokens.refresh_token)
  }
  useRemoteRuntimeStore.setState({
    status: 'authenticated',
    userId: tokens.user_id,
    accessToken: tokens.access_token,
    accessExpiresAt: tokens.access_expires_at,
    error: null,
  })
}

async function refreshAccessToken(): Promise<string> {
  if (refreshInFlight) return refreshInFlight
  refreshInFlight = (async () => {
    const state = useRemoteRuntimeStore.getState()
    if (!state.serverUrl) throw new Error('No Open Cowork server is configured')
    const refreshToken = IS_WEB_APP ? undefined : await getCredential(TOKEN_LOCATOR)
    if (!IS_WEB_APP && !refreshToken) {
      throw new Error('The server session has expired. Please sign in again.')
    }
    const tokens = await new RemoteAuthClient(state.serverUrl).refresh(refreshToken ?? undefined)
    await applyTokens(tokens)
    return tokens.access_token
  })()
  try {
    return await refreshInFlight
  } finally {
    refreshInFlight = null
  }
}

export async function remoteAccessToken(): Promise<string> {
  const state = useRemoteRuntimeStore.getState()
  const expiresAt = state.accessExpiresAt ? Date.parse(state.accessExpiresAt) : 0
  if (state.accessToken && expiresAt > Date.now() + 30_000) return state.accessToken
  try {
    return await refreshAccessToken()
  } catch (error) {
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

export function remoteRuntimeClient(): RemoteRuntimeClient {
  const serverUrl = useRemoteRuntimeStore.getState().serverUrl
  if (!serverUrl) throw new Error('No Open Cowork server is configured')
  return new RemoteRuntimeClient({ baseUrl: serverUrl, accessToken: remoteAccessToken })
}

const initialConnection = readStoredConnection()

export const useRemoteRuntimeStore = create<RemoteRuntimeState>((set, get) => ({
  ...initialConnection,
  status: 'signed_out',
  userId: null,
  accessToken: null,
  accessExpiresAt: null,
  error: null,

  setConnection: (serverUrl, email) => {
    set({ serverUrl: selectedServerUrl(serverUrl), email, error: null })
  },

  restore: async () => {
    if (get().status === 'authenticated') return true
    if (restoreInFlight) return restoreInFlight
    restoreInFlight = (async () => {
      set({ status: 'restoring', error: null })
      try {
        const callbackTokens = await completePendingOidc() ?? await completePendingNativePasskey()
        if (callbackTokens) {
          await applyTokens(callbackTokens)
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
    try {
      return await restoreInFlight
    } finally {
      restoreInFlight = null
    }
  },

  login: async (serverUrl, email, password, secondFactor) => {
    set({ status: 'authenticating', error: null })
    try {
      const normalizedUrl = selectedServerUrl(serverUrl)
      const normalizedEmail = email.trim().toLocaleLowerCase()
      const tokens = await new RemoteAuthClient(normalizedUrl).loginPkce({
        email: normalizedEmail,
        password,
        device_id: remoteDeviceId(),
        second_factor: secondFactor?.trim() || null,
      })
      storeConnection(normalizedUrl, normalizedEmail)
      set({ serverUrl: normalizedUrl, email: normalizedEmail })
      await applyTokens(tokens)
    } catch (error) {
      set({ status: 'error', error: errorMessage(error) })
      throw error
    }
  },

  bootstrap: async (serverUrl, email, displayName, password, bootstrapToken) => {
    set({ status: 'authenticating', error: null })
    try {
      const normalizedUrl = selectedServerUrl(serverUrl)
      const normalizedEmail = email.trim().toLocaleLowerCase()
      const tokens = await new RemoteAuthClient(normalizedUrl).bootstrap({
        email: normalizedEmail,
        display_name: displayName.trim(),
        password,
        bootstrap_token: bootstrapToken.trim(),
        device_id: remoteDeviceId(),
      })
      storeConnection(normalizedUrl, normalizedEmail)
      set({ serverUrl: normalizedUrl, email: normalizedEmail })
      await applyTokens(tokens)
    } catch (error) {
      set({ status: 'error', error: errorMessage(error) })
      throw error
    }
  },

  acceptInvitation: async (serverUrl, email, displayName, password, invitationToken) => {
    set({ status: 'authenticating', error: null })
    try {
      const normalizedUrl = selectedServerUrl(serverUrl)
      const normalizedEmail = email.trim().toLocaleLowerCase()
      const tokens = await new RemoteAuthClient(normalizedUrl).acceptInvitation({
        token: invitationToken.trim(),
        display_name: displayName.trim(),
        password,
        device_id: remoteDeviceId(),
      })
      storeConnection(normalizedUrl, normalizedEmail)
      set({ serverUrl: normalizedUrl, email: normalizedEmail })
      await applyTokens(tokens)
    } catch (error) {
      set({ status: 'error', error: errorMessage(error) })
      throw error
    }
  },

  loginPasskey: async (serverUrl, email) => {
    set({ status: 'authenticating', error: null })
    try {
      const normalizedUrl = selectedServerUrl(serverUrl)
      const normalizedEmail = email.trim().toLocaleLowerCase()
      storeConnection(normalizedUrl, normalizedEmail)
      set({ serverUrl: normalizedUrl, email: normalizedEmail })
      const deviceId = remoteDeviceId()
      const tokens = webauthnAvailableForOrigin(normalizedUrl)
        ? await new RemoteAuthClient(normalizedUrl).loginPasskey(normalizedEmail, deviceId)
        : await loginWithNativePasskey(normalizedUrl, normalizedEmail, deviceId)
      await applyTokens(tokens)
    } catch (error) {
      set({ status: 'error', error: errorMessage(error) })
      throw error
    }
  },

  loginOidc: async (serverUrl) => {
    set({ status: 'authenticating', error: null })
    try {
      const normalizedUrl = selectedServerUrl(serverUrl)
      const email = get().email
      storeConnection(normalizedUrl, email)
      set({ serverUrl: normalizedUrl })
      const tokens = await loginWithOidc(normalizedUrl, remoteDeviceId())
      if (tokens) await applyTokens(tokens)
    } catch (error) {
      set({ status: 'error', error: errorMessage(error) })
      throw error
    }
  },

  linkOidc: async () => {
    const state = get()
    if (state.status !== 'authenticated' || !state.serverUrl) {
      throw new Error('Sign in before linking an OIDC identity')
    }
    try {
      const tokens = await loginWithOidc(
        state.serverUrl,
        remoteDeviceId(),
        await remoteAccessToken(),
      )
      if (tokens) await applyTokens(tokens)
    } catch (error) {
      set({ error: errorMessage(error) })
      throw error
    }
  },

  logout: async () => {
    const { serverUrl, accessToken } = get()
    set({ status: 'signed_out', userId: null, accessToken: null, accessExpiresAt: null, error: null })
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

  clearError: () => set({ error: null, status: get().accessToken ? 'authenticated' : 'signed_out' }),
}))

export function resetRemoteRuntimeStoreForTests(): void {
  refreshInFlight = null
  restoreInFlight = null
  useRemoteRuntimeStore.setState({
    serverUrl: '',
    email: '',
    status: 'signed_out',
    userId: null,
    accessToken: null,
    accessExpiresAt: null,
    error: null,
  })
}
