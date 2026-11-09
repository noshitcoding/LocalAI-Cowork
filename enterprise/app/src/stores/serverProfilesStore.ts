import { create } from 'zustand'
import { z } from 'zod'
import { normalizeServerUrl } from '../runtime/runtimeClient'

const KEY = 'open-cowork-server-profiles-v1'
const profileSchema = z.object({ id: z.uuid(), name: z.string().max(100), serverUrl: z.string(), email: z.string().max(320) })
export type ServerProfile = z.infer<typeof profileSchema>

function readProfiles(): ServerProfile[] {
  try {
    return profileSchema.array().max(50).parse(JSON.parse(localStorage.getItem(KEY) ?? '[]'))
      .map(profile => ({ ...profile, serverUrl: normalizeServerUrl(profile.serverUrl) }))
  } catch { return [] }
}

function persist(profiles: ServerProfile[]) {
  try { localStorage.setItem(KEY, JSON.stringify(profiles)) } catch { /* Session-only profiles when storage is unavailable. */ }
}

// Addresses and account names only. Credentials remain in the OS vault.
export const useServerProfilesStore = create<{
  profiles: ServerProfile[]
  save: (serverUrl: string, email: string, name?: string) => ServerProfile
  remove: (id: string) => void
}>((set, get) => ({
  profiles: readProfiles(),
  save: (serverUrl, email, name) => {
    const url = normalizeServerUrl(serverUrl)
    const account = email.trim().toLowerCase().slice(0, 320)
    const current = get().profiles.find(profile => profile.serverUrl === url && profile.email === account)
    const profile = { id: current?.id ?? crypto.randomUUID(), name: (name?.trim() || current?.name || new URL(url).host).slice(0, 100), serverUrl: url, email: account }
    const profiles = [profile, ...get().profiles.filter(item => item.id !== profile.id)].slice(0, 50)
    persist(profiles)
    set({ profiles })
    return profile
  },
  remove: id => {
    const profiles = get().profiles.filter(profile => profile.id !== id)
    persist(profiles)
    set({ profiles })
  },
}))
