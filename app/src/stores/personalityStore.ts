import { create } from 'zustand'
import { safeInvoke } from '../utils/safeInvoke'
import { DEFAULT_PERSONALITIES } from '../utils/defaultSeeds'
import type { AgentRole, CrewPersonalityProfile } from './crewStore'

export type Personality = {
  id: string
  name: string
  description: string
  role: AgentRole
  goal: string
  system_prompt: string
  skills_markdown: string
  temperature: number | null
  model_override: string | null
  icon: string | null
  is_default: boolean
  created_at: string
  updated_at: string
}

type PersonalityState = {
  personalities: Personality[]
  activeId: string | null
  loading: boolean
  error: string | null

  loadPersonalities: () => Promise<void>
  upsertPersonality: (p: {
    id: string; name: string; description?: string; role?: AgentRole; goal?: string; systemPrompt: string; skillsMarkdown?: string
    temperature?: number | null; modelOverride?: string | null; icon?: string | null; isDefault?: boolean
  }) => Promise<void>
  deletePersonality: (id: string) => Promise<void>
  setActive: (id: string | null) => void
}

const AGENT_ROLES: AgentRole[] = ['researcher', 'writer', 'reviewer', 'planner', 'executor', 'analyst', 'custom']

function normalizeRole(value: unknown): AgentRole {
  return AGENT_ROLES.includes(value as AgentRole) ? value as AgentRole : 'custom'
}

function normalizePersonality(raw: Partial<Personality> & { systemPrompt?: string; skillsMarkdown?: string }): Personality {
  const timestamp = new Date().toISOString()
  const goal = typeof raw.goal === 'string' ? raw.goal : typeof raw.description === 'string' ? raw.description : ''
  const systemPrompt = typeof raw.system_prompt === 'string' ? raw.system_prompt : raw.systemPrompt ?? ''

  return {
    id: raw.id ?? `pers-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
    name: raw.name?.trim() || 'Personality',
    description: typeof raw.description === 'string' ? raw.description : goal,
    role: normalizeRole(raw.role),
    goal,
    system_prompt: systemPrompt,
    skills_markdown: typeof raw.skills_markdown === 'string' ? raw.skills_markdown : raw.skillsMarkdown ?? '',
    temperature: typeof raw.temperature === 'number' ? raw.temperature : null,
    model_override: raw.model_override?.trim() || null,
    icon: raw.icon || null,
    is_default: Boolean(raw.is_default),
    created_at: raw.created_at ?? timestamp,
    updated_at: raw.updated_at ?? timestamp,
  }
}

function buildUniqueName(name: string, id: string, personalities: Personality[]): string {
  const baseName = name.trim() || 'Personality'
  let candidate = baseName
  let index = 2

  while (personalities.some((personality) => (
    personality.id !== id && personality.name.trim().toLowerCase() === candidate.toLowerCase()
  ))) {
    candidate = `${baseName} (${index})`
    index += 1
  }

  return candidate
}

function toCrewPersonalityProfile(personality: Personality): CrewPersonalityProfile {
  return {
    id: personality.id,
    name: personality.name,
    description: personality.description,
    role: personality.role,
    goal: personality.goal,
    systemPrompt: personality.system_prompt,
    skillsMarkdown: personality.skills_markdown,
    modelOverride: personality.model_override,
    temperature: personality.temperature,
    icon: personality.icon,
    isDefault: personality.is_default,
  }
}

function buildFallbackPersonalities(): Personality[] {
  const timestamp = new Date().toISOString()

  return DEFAULT_PERSONALITIES.map((entry) => normalizePersonality({
    id: entry.id,
    name: entry.name,
    description: entry.goal,
    role: entry.role,
    goal: entry.goal,
    system_prompt: entry.systemPrompt,
    skills_markdown: entry.skillsMarkdown,
    temperature: entry.temperature,
    model_override: null,
    icon: entry.icon,
    is_default: entry.isDefault,
    created_at: timestamp,
    updated_at: timestamp,
  }))
}

export const usePersonalityStore = create<PersonalityState>()((set, get) => ({
  personalities: [],
  activeId: null,
  loading: false,
  error: null,

  loadPersonalities: async () => {
    set({ loading: true, error: null })
    try {
      const personalities = await safeInvoke<Personality[]>('personality_list', undefined, [])
      const resolvedPersonalities = personalities.length > 0 ? personalities.map(normalizePersonality) : buildFallbackPersonalities()
      set({ personalities: resolvedPersonalities, loading: false })
      const def = resolvedPersonalities.find((p) => p.is_default)
      if (def) set({ activeId: def.id })
    } catch (e) {
      const fallbackPersonalities = buildFallbackPersonalities()
      set({
        personalities: fallbackPersonalities,
        error: String(e),
        loading: false,
      })
      const def = fallbackPersonalities.find((p) => p.is_default)
      if (def) set({ activeId: def.id })
    }
  },

  upsertPersonality: async (p) => {
    const current = get().personalities
    const existing = current.find((personality) => personality.id === p.id)
    const now = new Date().toISOString()
    const name = buildUniqueName(p.name, p.id, current)
    const goal = p.goal ?? p.description ?? existing?.goal ?? ''
    const next = normalizePersonality({
      ...existing,
      id: p.id,
      name,
      description: p.description ?? goal,
      role: p.role ?? existing?.role ?? 'custom',
      goal,
      system_prompt: p.systemPrompt,
      skills_markdown: p.skillsMarkdown ?? existing?.skills_markdown ?? '',
      temperature: p.temperature ?? null,
      model_override: p.modelOverride ?? null,
      icon: p.icon ?? null,
      is_default: p.isDefault ?? false,
      created_at: existing?.created_at ?? now,
      updated_at: now,
    })

    set((state) => ({
      personalities: [
        next,
        ...state.personalities
          .filter((personality) => personality.id !== next.id)
          .map((personality) => next.is_default ? { ...personality, is_default: false } : personality),
      ].sort((a, b) => a.name.localeCompare(b.name)),
      activeId: next.is_default ? next.id : state.activeId,
      error: null,
    }))

    try {
      await safeInvoke('personality_upsert', {
        id: next.id,
        name: next.name,
        description: next.description,
        role: next.role,
        goal: next.goal,
        systemPrompt: next.system_prompt,
        skillsMarkdown: next.skills_markdown,
        temperature: next.temperature,
        modelOverride: next.model_override,
        icon: next.icon,
        isDefault: next.is_default,
      }, undefined)
    } catch (e) {
      set({ error: String(e) })
    }
  },

  deletePersonality: async (id) => {
    try {
      const personality = get().personalities.find((entry) => entry.id === id)
      await safeInvoke('personality_delete', { id }, undefined)
      if (personality) {
        const { useCrewStore } = await import('./crewStore')
        useCrewStore.getState().unlinkPersonalityProfile(toCrewPersonalityProfile(personality))
      }
      set((s) => ({
        personalities: s.personalities.filter((p) => p.id !== id),
        activeId: s.activeId === id ? null : s.activeId,
      }))
    } catch (e) {
      set({ error: String(e) })
    }
  },

  setActive: (id) => set({ activeId: id }),
}))
