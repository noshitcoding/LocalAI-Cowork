import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { openUrl } from '@tauri-apps/plugin-opener'
import { create } from 'zustand'
import { hasTauriRuntime } from '../utils/safeInvoke'
import i18n from '../i18n'

export type CodexProfileStatus = 'signed_out' | 'login_pending' | 'ready' | 'limited' | 'requires_reauth' | 'unavailable'

export type CodexAuthProfile = {
  id: string
  name: string
  email: string | null
  accountId: string | null
  planType: string | null
  priority: number
  status: CodexProfileStatus
  cooldownUntil: string | null
  quotaJson: string | null
  quotaResetAt: string | null
  createdAt: string
  updatedAt: string
}

export type CodexModel = {
  id: string
  model: string
  displayName: string
  defaultReasoningEffort?: string
  supportedReasoningEfforts?: Array<{ reasoningEffort: string; description?: string }>
  inputModalities?: string[]
  isDefault?: boolean
}

export type CodexRuntimeStatus = {
  available: boolean
  version: string
  protocolSchema: string
  error: string | null
}

type CodexLoginResult = {
  type: 'chatgpt' | 'chatgptDeviceCode'
  loginId: string
  authUrl?: string
  verificationUrl?: string
  userCode?: string
}

type CodexRuntimeEvent = {
  profileId: string
  payload: {
    id?: number
    method?: string
    params?: Record<string, unknown>
  }
}

type CodexStoreState = {
  runtime: CodexRuntimeStatus
  profiles: CodexAuthProfile[]
  modelsByProfile: Record<string, CodexModel[]>
  deviceLogin: { profileId: string; verificationUrl: string; userCode: string } | null
  loading: boolean
  error: string | null
  load: () => Promise<void>
  createProfile: (name?: string) => Promise<string>
  renameProfile: (id: string, name: string) => Promise<void>
  reorderProfile: (id: string, direction: -1 | 1) => Promise<void>
  login: (id: string, flow?: 'browser' | 'device') => Promise<void>
  refreshProfile: (id: string, refreshToken?: boolean) => Promise<void>
  loadModels: (id: string) => Promise<CodexModel[]>
  logout: (id: string) => Promise<void>
  removeProfile: (id: string) => Promise<void>
  clearError: () => void
}

let runtimeUnlisten: UnlistenFn | null = null

function now(): string {
  return new Date().toISOString()
}

async function saveProfile(profile: CodexAuthProfile): Promise<void> {
  await invoke('codex_profile_upsert', { profile })
}

function sortProfiles(profiles: CodexAuthProfile[]): CodexAuthProfile[] {
  return [...profiles].sort((left, right) => left.priority - right.priority || left.createdAt.localeCompare(right.createdAt))
}

async function installRuntimeListener(): Promise<void> {
  if (!hasTauriRuntime() || runtimeUnlisten) return
  runtimeUnlisten = await listen<CodexRuntimeEvent>('codex-runtime-event', ({ payload: event }) => {
    const method = event.payload.method
    if (method === 'account/login/completed' || method === 'account/updated') {
      void useCodexStore.getState().refreshProfile(event.profileId)
      return
    }
    if (method === 'account/rateLimits/updated') {
      void useCodexStore.getState().refreshProfile(event.profileId)
      return
    }
    if (method === 'runtime/stopped' || method === 'runtime/protocolError') {
      useCodexStore.setState({
        error: method === 'runtime/protocolError'
          ? String(event.payload.params?.message ?? 'Codex protocol error')
          : i18n.t('Codex App Server stopped unexpectedly.'),
      })
    }
  })
}

export const useCodexStore = create<CodexStoreState>((set, get) => ({
  runtime: {
    available: false,
    version: '0.147.0',
    protocolSchema: 'app-server-0.147.0',
    error: null,
  },
  profiles: [],
  modelsByProfile: {},
  deviceLogin: null,
  loading: false,
  error: null,

  load: async () => {
    if (!hasTauriRuntime()) return
    set({ loading: true, error: null })
    try {
      await installRuntimeListener()
      const [runtime, profiles] = await Promise.all([
        invoke<CodexRuntimeStatus>('codex_runtime_status'),
        invoke<CodexAuthProfile[]>('codex_profile_list'),
      ])
      set({ runtime, profiles: sortProfiles(profiles), loading: false })
      for (const profile of profiles.filter((profile) => profile.status === 'ready' || profile.status === 'login_pending')) {
        void get().refreshProfile(profile.id)
      }
    } catch (error) {
      set({ loading: false, error: error instanceof Error ? error.message : String(error) })
    }
  },

  createProfile: async (name) => {
    const timestamp = now()
    const id = `codex-${crypto.randomUUID()}`
    const profile: CodexAuthProfile = {
      id,
      name: name?.trim() || `Codex-Konto ${get().profiles.length + 1}`,
      email: null,
      accountId: null,
      planType: null,
      priority: get().profiles.length,
      status: 'signed_out',
      cooldownUntil: null,
      quotaJson: null,
      quotaResetAt: null,
      createdAt: timestamp,
      updatedAt: timestamp,
    }
    if (hasTauriRuntime()) await saveProfile(profile)
    set((state) => ({ profiles: sortProfiles([...state.profiles, profile]) }))
    return id
  },

  renameProfile: async (id, name) => {
    const profile = get().profiles.find((profile) => profile.id === id)
    if (!profile || !name.trim()) return
    const updated = { ...profile, name: name.trim(), updatedAt: now() }
    if (hasTauriRuntime()) await saveProfile(updated)
    set((state) => ({ profiles: state.profiles.map((item) => item.id === id ? updated : item) }))
  },

  reorderProfile: async (id, direction) => {
    const profiles = sortProfiles(get().profiles)
    const index = profiles.findIndex((profile) => profile.id === id)
    const target = index + direction
    if (index < 0 || target < 0 || target >= profiles.length) return
    ;[profiles[index], profiles[target]] = [profiles[target], profiles[index]]
    const updated = profiles.map((profile, priority) => ({ ...profile, priority, updatedAt: now() }))
    if (hasTauriRuntime()) await Promise.all(updated.map(saveProfile))
    set({ profiles: updated })
  },

  login: async (id, flow = 'browser') => {
    set({ error: null, deviceLogin: null })
    try {
      const result = await invoke<CodexLoginResult>('codex_login_start', { profileId: id, flow })
      if (result.authUrl) await openUrl(result.authUrl)
      if (result.verificationUrl && result.userCode) {
        set({ deviceLogin: { profileId: id, verificationUrl: result.verificationUrl, userCode: result.userCode } })
      }
      set((state) => ({
        profiles: state.profiles.map((profile) => profile.id === id ? { ...profile, status: 'login_pending' } : profile),
      }))
    } catch (error) {
      set({ error: error instanceof Error ? error.message : String(error) })
    }
  },

  refreshProfile: async (id, refreshToken = false) => {
    if (!hasTauriRuntime()) return
    try {
      await invoke('codex_account_read', { profileId: id, refreshToken })
      try { await invoke('codex_rate_limits_read', { profileId: id }) } catch { /* account may be signed out */ }
      const profiles = await invoke<CodexAuthProfile[]>('codex_profile_list')
      set({ profiles: sortProfiles(profiles), error: null })
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      const requiresReauth = /invalid_grant|reauth|unauthori[sz]ed|401/i.test(message)
      set((state) => ({
        error: message,
        profiles: state.profiles.map((profile) => profile.id === id && requiresReauth
          ? { ...profile, status: 'requires_reauth' }
          : profile),
      }))
    }
  },

  loadModels: async (id) => {
    const result = await invoke<{ data?: CodexModel[] }>('codex_model_list', { profileId: id })
    const models = Array.isArray(result.data) ? result.data : []
    set((state) => ({ modelsByProfile: { ...state.modelsByProfile, [id]: models } }))
    return models
  },

  logout: async (id) => {
    await invoke('codex_logout', { profileId: id })
    set((state) => ({
      profiles: state.profiles.map((profile) => profile.id === id ? {
        ...profile,
        email: null,
        accountId: null,
        planType: null,
        status: 'signed_out',
        quotaJson: null,
        quotaResetAt: null,
      } : profile),
    }))
  },

  removeProfile: async (id) => {
    await invoke('codex_profile_delete', { profileId: id })
    set((state) => ({
      profiles: state.profiles.filter((profile) => profile.id !== id),
      deviceLogin: state.deviceLogin?.profileId === id ? null : state.deviceLogin,
    }))
  },

  clearError: () => set({ error: null }),
}))
