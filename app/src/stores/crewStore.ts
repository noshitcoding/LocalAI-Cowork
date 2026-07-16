import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import type { OllamaConfig } from './configStore'
import { useConfigStore } from './configStore'
import { safeInvoke } from '../utils/safeInvoke'
import { crewProviderLocator, deleteCredential, setCredential } from '../security/credentialVault'
import { sanitizeCrewsForPersistence } from '../security/credentialPersistence'
import { redactText } from '../security/redaction'

export type AgentRole = 'researcher' | 'writer' | 'reviewer' | 'planner' | 'executor' | 'analyst' | 'custom'
export type CrewProcess = 'sequential' | 'parallel' | 'hierarchical'
export type CrewProviderKind = 'ollama' | 'openai-compatible' | 'openrouter'
export type CrewOutputMode = 'standard' | 'bullet-report' | 'json'
export type CrewGovernanceMode = 'allow-all' | 'ask-risky' | 'ask-all' | 'read-only'

export type CrewAgent = {
  id: string
  name: string
  role: AgentRole
  goal: string
  backstory: string
  skillsMarkdown: string
  personalityId: string | null
  modelOverride: string | null
  providerKind: CrewProviderKind
  tools: string[]
  mcpServerNames: string[]
  enabled: boolean
  allowDelegation: boolean
  verbose: boolean
  maxIterations: number
}

export type CrewTask = {
  id: string
  description: string
  expectedOutput: string
  agentId: string
  context: string[]
  dependencies: string[]
  asyncExecution: boolean
  status: 'pending' | 'running' | 'completed' | 'failed'
  output: string | null
}

export type CrewRuntimeConfig = {
  enabled: boolean
  baseUrl: string
  model: string
  timeoutMs: number
}

export type CrewExternalProviderConfig = {
  enabled: boolean
  baseUrl: string
  model: string
  apiKey: string
  timeoutMs: number
  verifyTlsCertificates: boolean
}

export type CrewProviderProfiles = {
  openAICompatible: CrewExternalProviderConfig
  openRouter: CrewExternalProviderConfig
}

export type Crew = {
  id: string
  name: string
  description: string
  executionSubject: string
  executionGuidelines: string
  knowledgeFocus: string
  governanceMode: CrewGovernanceMode
  outputMode: CrewOutputMode
  stopOnFailure: boolean
  retryCount: number
  managerReviewEnabled: boolean
  managerReviewGuidelines: string
  shareAllTaskOutputs: boolean
  sharedOutputCharLimit: number
  defaultProvider?: CrewProviderKind
  defaultModel?: string
  providerProfiles: CrewProviderProfiles
  agents: CrewAgent[]
  tasks: CrewTask[]
  runtimeConfig: CrewRuntimeConfig
  process: CrewProcess
  managerAgentId: string | null
  verbose: boolean
  maxRpm: number
  maxParallelTasks: number
  status: 'idle' | 'running' | 'awaiting-approval' | 'completed' | 'failed' | 'canceled'
  createdAt: number
  updatedAt: number
}

export type CrewExecutionLog = {
  id: string
  crewId: string
  agentId: string
  taskId: string
  action: string
  result: string
  timestamp: number
  agentName?: string | null
  sourceAgent?: string | null
  targetAgent?: string | null
  provider?: string | null
  model?: string | null
  taskTitle?: string | null
  phase?: string | null
  summary?: string | null
  detail?: string | null
  severity?: 'info' | 'warning' | 'error' | null
  providerReasoning?: string | null
}

function redactCrewExecutionLog(log: CrewExecutionLog): CrewExecutionLog {
  const redactOptional = (value: string | null | undefined) => (
    typeof value === 'string' ? redactText(value) : value
  )
  return {
    ...log,
    action: redactText(log.action),
    result: redactText(log.result),
    summary: redactOptional(log.summary),
    detail: redactOptional(log.detail),
    providerReasoning: redactOptional(log.providerReasoning),
  }
}

export type CrewPersonalityProfile = {
  id: string
  name: string
  description: string
  role: AgentRole
  goal: string
  systemPrompt: string
  skillsMarkdown: string
  modelOverride: string | null
  temperature?: number | null
  icon?: string | null
  isDefault?: boolean
}

type CrewTaskExecutionResponse = {
  taskId: string
  agentId: string
  status: CrewTask['status'] | 'canceled'
  output: string | null
}

type CrewExecutionResponse = {
  crewId: string
  status: Crew['status']
  taskResults: CrewTaskExecutionResponse[]
  logs: CrewExecutionLog[]
  error: string | null
}

type CrewState = {
  crews: Crew[]
  agents: CrewAgent[]
  executionLogs: CrewExecutionLog[]
  activeCrewId: string | null
  loading: boolean

  createCrew: (id: string, name: string, agentIds: string[]) => void
  createStarterCrew: (name: string, goal: string) => string
  updateCrew: (id: string, patch: Partial<Omit<Crew, 'providerProfiles'>>) => void
  setCrewProviderProfiles: (id: string, profiles: CrewProviderProfiles) => Promise<void>
  deleteCrew: (id: string) => Promise<void>
  setActiveCrew: (id: string | null) => void

  addAgent: (agent: CrewAgent) => void
  updateAgent: (id: string, patch: Partial<CrewAgent>) => void
  updateCrewAgent: (crewId: string, agentId: string, patch: Partial<CrewAgent>) => void
  removeAgent: (id: string) => void
  loadAgents: () => void
  syncAgentsFromPersonalityProfiles: (profiles: CrewPersonalityProfile[]) => void
  migrateAgentsToPersonalityProfiles: (profiles: CrewPersonalityProfile[]) => Promise<boolean>
  unlinkPersonalityProfile: (profile: CrewPersonalityProfile) => void

  addTask: (crewId: string, task: CrewTask) => void
  updateTask: (crewId: string, taskId: string, patch: Partial<CrewTask>) => void
  removeTask: (crewId: string, taskId: string) => void

  runCrew: (crewId: string) => void
  stopCrew: (crewId: string) => void

  addLog: (log: CrewExecutionLog) => void
  installDefaultAgents: () => void
}

const DEFAULT_AGENTS: CrewAgent[] = [
  {
    id: 'agent-researcher',
    name: 'Forscher',
    role: 'researcher',
    goal: 'Gruendliche Recherche und Informationsbeschaffung zu jedem Thema',
    backstory: 'An experienced researcher with access to diverse sources. Analyzes information critically and delivers solid results.',
    skillsMarkdown: '',
    personalityId: null,
    modelOverride: null,
    providerKind: 'ollama',
    tools: ['web_search', 'web_fetch', 'grep', 'glob', 'read_file'],
    mcpServerNames: [],
    enabled: true,
    allowDelegation: true,
    verbose: true,
    maxIterations: 10,
  },
  {
    id: 'agent-writer',
    name: 'Autor',
    role: 'writer',
    goal: 'Hochwertige Texte, documentation und Content erstellen',
    backstory: 'An experienced writer who creates clear, concise, and well-structured text across multiple writing styles.',
    skillsMarkdown: '',
    personalityId: null,
    modelOverride: null,
    providerKind: 'ollama',
    tools: ['edit_file', 'read_file', 'glob', 'office_workflow'],
    mcpServerNames: [],
    enabled: true,
    allowDelegation: false,
    verbose: true,
    maxIterations: 5,
  },
  {
    id: 'agent-reviewer',
    name: 'Reviewer',
    role: 'reviewer',
    goal: 'Review and improve the quality of code and text',
    backstory: 'Ein erfahrener Code-Reviewer mit Blick for Details, best practices und potenzielle problems.',
    skillsMarkdown: '',
    personalityId: null,
    modelOverride: null,
    providerKind: 'ollama',
    tools: ['read_file', 'grep', 'glob'],
    mcpServerNames: [],
    enabled: true,
    allowDelegation: true,
    verbose: true,
    maxIterations: 8,
  },
  {
    id: 'agent-planner',
    name: 'Planer',
    role: 'planner',
    goal: 'Komplexe Tasks in ausfuehrbare Schritte zerlegen',
    backstory: 'A strategic thinker who analyzes complex problems and translates them into clear, prioritized action plans.',
    skillsMarkdown: '',
    personalityId: null,
    modelOverride: null,
    providerKind: 'ollama',
    tools: ['todo', 'read_file', 'glob', 'grep'],
    mcpServerNames: [],
    enabled: true,
    allowDelegation: true,
    verbose: true,
    maxIterations: 5,
  },
  {
    id: 'agent-executor',
    name: 'Ausfuehrer',
    role: 'executor',
    goal: 'Tasks zuverlaessig und effizient execute',
    backstory: 'A reliable executor who implements plans precisely, detects errors, and solves them independently.',
    skillsMarkdown: '',
    personalityId: null,
    modelOverride: null,
    providerKind: 'ollama',
    tools: ['bash', 'edit_file', 'read_file', 'glob', 'grep', 'office_workflow'],
    mcpServerNames: [],
    enabled: true,
    allowDelegation: false,
    verbose: true,
    maxIterations: 15,
  },
  {
    id: 'agent-analyst',
    name: 'Analyst',
    role: 'analyst',
    goal: 'Analyze data, detect patterns, and derive recommendations',
    backstory: 'A data analyst who recognizes relationships, evaluates metrics, and provides data-driven recommendations.',
    skillsMarkdown: '',
    personalityId: null,
    modelOverride: null,
    providerKind: 'ollama',
    tools: ['read_file', 'grep', 'glob', 'web_search', 'web_fetch'],
    mcpServerNames: [],
    enabled: true,
    allowDelegation: true,
    verbose: true,
    maxIterations: 8,
  },
]

const canceledCrewIds = new Set<string>()

const DEFAULT_CREW_RUNTIME_CONFIG: CrewRuntimeConfig = {
  enabled: false,
  baseUrl: '',
  model: '',
  timeoutMs: 600000,
}

const DEFAULT_EXTERNAL_PROVIDER_CONFIG: CrewExternalProviderConfig = {
  enabled: false,
  baseUrl: '',
  model: '',
  apiKey: '',
  timeoutMs: 600000,
  verifyTlsCertificates: true,
}

const DEFAULT_CREW_PROVIDER_PROFILES: CrewProviderProfiles = {
  openAICompatible: { ...DEFAULT_EXTERNAL_PROVIDER_CONFIG },
  openRouter: {
    ...DEFAULT_EXTERNAL_PROVIDER_CONFIG,
    baseUrl: 'https://openrouter.ai/api/v1',
  },
}

const DEFAULT_CREW_OUTPUT_MODE: CrewOutputMode = 'standard'
const DEFAULT_CREW_PROVIDER: CrewProviderKind = 'ollama'
const DEFAULT_CREW_GOVERNANCE_MODE: CrewGovernanceMode = 'allow-all'
const PERSONALITY_AGENT_ID_PREFIX = 'agent-personality-'

function isCrewAwaitingApproval(message: string): boolean {
  return message.trim().toLowerCase().startsWith('crew waiting for approval:')
}

function createPersonalityCrewAgentId(personalityId: string): string {
  return `${PERSONALITY_AGENT_ID_PREFIX}${personalityId}`
}

function isSyncablePersonalityProfile(profile: CrewPersonalityProfile): boolean {
  return profile.name.trim().length > 0
}

function isSamePersonalityAgent(agent: CrewAgent, personalityId: string): boolean {
  return agent.personalityId === personalityId || agent.id === createPersonalityCrewAgentId(personalityId)
}

function slugifyProfileName(value: string): string {
  return value.trim().toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '') || 'agent'
}

function buildAgentProfileSignature(agent: CrewAgent): string {
  return JSON.stringify({
    name: agent.name.trim(),
    role: agent.role,
    goal: agent.goal.trim(),
    systemPrompt: agent.backstory.trim(),
    skillsMarkdown: agent.skillsMarkdown.trim(),
    modelOverride: agent.modelOverride?.trim() || null,
  })
}

function createUniqueProfileName(baseName: string, existingNames: Set<string>): string {
  const base = baseName.trim() || 'Personality'
  let candidate = base
  let index = 2

  while (existingNames.has(candidate.toLowerCase())) {
    candidate = `${base} (${index})`
    index += 1
  }

  existingNames.add(candidate.toLowerCase())
  return candidate
}

function buildPersonalityProfileFromAgent(agent: CrewAgent, id: string, name: string): CrewPersonalityProfile {
  return {
    id,
    name,
    description: agent.goal,
    role: agent.role,
    goal: agent.goal,
    systemPrompt: agent.backstory,
    skillsMarkdown: agent.skillsMarkdown,
    modelOverride: agent.modelOverride?.trim() || null,
    temperature: null,
    icon: null,
    isDefault: false,
  }
}

function buildCrewAgentFromPersonality(profile: CrewPersonalityProfile): CrewAgent {
  const trimmedName = profile.name.trim()
  const trimmedGoal = (profile.goal || profile.description).trim()
  const trimmedModelOverride = profile.modelOverride?.trim() || null

  return {
    id: createPersonalityCrewAgentId(profile.id),
    name: trimmedName,
    role: profile.role ?? 'custom',
    goal: trimmedGoal || `Work in the style of ${trimmedName}.`,
    backstory: profile.systemPrompt,
    skillsMarkdown: profile.skillsMarkdown,
    personalityId: profile.id,
    modelOverride: trimmedModelOverride,
    providerKind: 'ollama',
    tools: [],
    mcpServerNames: [],
    enabled: true,
    allowDelegation: true,
    verbose: true,
    maxIterations: 8,
  }
}

export function resolveCrewAgentWithProfile(agent: CrewAgent, profiles: CrewPersonalityProfile[]): CrewAgent {
  if (!agent.personalityId) {
    return cloneCrewAgent(agent)
  }

  const profile = profiles.find((entry) => entry.id === agent.personalityId)
  if (!profile) {
    return cloneCrewAgent(agent)
  }

  return {
    ...agent,
    name: profile.name,
    role: profile.role ?? 'custom',
    goal: profile.goal || profile.description || `Work in the style of ${profile.name}.`,
    backstory: profile.systemPrompt,
    skillsMarkdown: profile.skillsMarkdown,
    modelOverride: profile.modelOverride?.trim() || null,
    tools: [...agent.tools],
    mcpServerNames: [...agent.mcpServerNames],
  }
}

export function resolveCrewAgentsWithProfiles(agents: CrewAgent[], profiles: CrewPersonalityProfile[]): CrewAgent[] {
  return agents.map((agent) => resolveCrewAgentWithProfile(agent, profiles))
}

function personalityRowsToCrewProfiles(rows: Array<{
  id: string
  name: string
  description?: string
  role?: AgentRole
  goal?: string
  system_prompt?: string
  skills_markdown?: string
  model_override?: string | null
  temperature?: number | null
  icon?: string | null
  is_default?: boolean
}>): CrewPersonalityProfile[] {
  return rows.map((row) => ({
    id: row.id,
    name: row.name,
    description: row.description ?? row.goal ?? '',
    role: row.role ?? 'custom',
    goal: row.goal ?? row.description ?? '',
    systemPrompt: row.system_prompt ?? '',
    skillsMarkdown: row.skills_markdown ?? '',
    modelOverride: row.model_override ?? null,
    temperature: row.temperature ?? null,
    icon: row.icon ?? null,
    isDefault: row.is_default ?? false,
  }))
}

async function loadPersonalityProfilesForRuntime(): Promise<CrewPersonalityProfile[]> {
  try {
    const { usePersonalityStore } = await import('./personalityStore')
    const store = usePersonalityStore.getState()
    if (store.personalities.length === 0) {
      await store.loadPersonalities()
    }
    return personalityRowsToCrewProfiles(usePersonalityStore.getState().personalities)
  } catch {
    return []
  }
}

function applyPersonalityProfileToAgent(agent: CrewAgent, profile: CrewPersonalityProfile): CrewAgent {
  return {
    ...resolveCrewAgentWithProfile(agent, [profile]),
    id: agent.id || createPersonalityCrewAgentId(profile.id),
    personalityId: profile.id,
    providerKind: agent.providerKind,
    enabled: agent.enabled,
    allowDelegation: agent.allowDelegation,
    verbose: agent.verbose,
    maxIterations: agent.maxIterations,
    tools: [...agent.tools],
    mcpServerNames: [...agent.mcpServerNames],
  }
}

function resolveCrewRuntimeConfig(crew: Crew, fallbackConfig?: OllamaConfig) {
  if (!crew.runtimeConfig.enabled) {
    return fallbackConfig
  }

  return {
    ...(fallbackConfig ?? { baseUrl: '', model: '', timeoutMs: 600000 }),
    baseUrl: crew.runtimeConfig.baseUrl.trim() || fallbackConfig?.baseUrl || '',
    model: crew.runtimeConfig.model.trim() || fallbackConfig?.model || '',
    timeoutMs: Math.max(1000, crew.runtimeConfig.timeoutMs || fallbackConfig?.timeoutMs || 600000),
  }
}

function resolveExternalProviderConfig(
  config: CrewExternalProviderConfig,
  fallbackConfig: { baseUrl?: string; model?: string; apiKey?: string; verifyTlsCertificates?: boolean } | undefined,
  fallbackBaseUrl: string,
) {
  if (!config.enabled) {
    return undefined
  }

  return {
    baseUrl: config.baseUrl.trim() || fallbackConfig?.baseUrl?.trim() || fallbackBaseUrl,
    model: config.model.trim() || fallbackConfig?.model?.trim() || '',
    apiKey: config.apiKey.trim() || fallbackConfig?.apiKey?.trim() || '',
    timeoutMs: Math.max(1000, config.timeoutMs || DEFAULT_EXTERNAL_PROVIDER_CONFIG.timeoutMs),
    verifyTlsCertificates: (config.verifyTlsCertificates ?? true) && (fallbackConfig?.verifyTlsCertificates ?? true),
  }
}

function normalizeCrewStateEntry(crew: Crew): Crew {
  return {
    ...crew,
    executionSubject: crew.executionSubject ?? 'workspace-user',
    executionGuidelines: crew.executionGuidelines ?? '',
    knowledgeFocus: crew.knowledgeFocus ?? '',
    governanceMode: crew.governanceMode ?? DEFAULT_CREW_GOVERNANCE_MODE,
    outputMode: crew.outputMode ?? DEFAULT_CREW_OUTPUT_MODE,
    stopOnFailure: crew.stopOnFailure ?? false,
    retryCount: crew.retryCount ?? 0,
    managerReviewEnabled: crew.managerReviewEnabled ?? true,
    managerReviewGuidelines: crew.managerReviewGuidelines ?? '',
    shareAllTaskOutputs: crew.shareAllTaskOutputs ?? true,
    sharedOutputCharLimit: crew.sharedOutputCharLimit ?? 0,
    defaultProvider: crew.defaultProvider ?? DEFAULT_CREW_PROVIDER,
    defaultModel: crew.defaultModel ?? '',
    providerProfiles: {
      openAICompatible: {
        ...DEFAULT_CREW_PROVIDER_PROFILES.openAICompatible,
        ...crew.providerProfiles?.openAICompatible,
      },
      openRouter: {
        ...DEFAULT_CREW_PROVIDER_PROFILES.openRouter,
        ...crew.providerProfiles?.openRouter,
      },
    },
    agents: dedupeCrewAgents(crew.agents),
    runtimeConfig: {
      ...DEFAULT_CREW_RUNTIME_CONFIG,
      ...crew.runtimeConfig,
    },
  }
}

function dedupeCrewAgents(agents: CrewAgent[]): CrewAgent[] {
  const seen = new Set<string>()

  return agents.filter((agent) => {
    if (seen.has(agent.id)) {
      return false
    }

    seen.add(agent.id)
    return true
  }).map(cloneCrewAgent)
}

function cloneCrewAgent(agent: CrewAgent): CrewAgent {
  return {
    ...agent,
    skillsMarkdown: agent.skillsMarkdown ?? '',
    tools: [...agent.tools],
    mcpServerNames: [...agent.mcpServerNames],
  }
}

function createExecutionLog(crewId: string, agentId: string, taskId: string, action: string, result: string): CrewExecutionLog {
  return {
    id: `log-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
    crewId,
    agentId,
    taskId,
    action,
    result,
    timestamp: Date.now(),
  }
}

export const useCrewStore = create<CrewState>()(
  persist(
    (set, get) => ({
      crews: [],
      agents: [],
      executionLogs: [],
      activeCrewId: null,
      loading: false,

      createCrew: (id, name, agentIds) => {
        const selectedAgents = get().agents.filter(a => agentIds.includes(a.id))
        const crew: Crew = {
          id,
          name,
          description: '',
          executionSubject: 'workspace-user',
          executionGuidelines: '',
          knowledgeFocus: '',
          governanceMode: DEFAULT_CREW_GOVERNANCE_MODE,
          outputMode: DEFAULT_CREW_OUTPUT_MODE,
          stopOnFailure: false,
          retryCount: 0,
          managerReviewEnabled: true,
          managerReviewGuidelines: '',
          shareAllTaskOutputs: true,
          sharedOutputCharLimit: 0,
          defaultProvider: DEFAULT_CREW_PROVIDER,
          defaultModel: '',
          providerProfiles: {
            openAICompatible: { ...DEFAULT_CREW_PROVIDER_PROFILES.openAICompatible },
            openRouter: { ...DEFAULT_CREW_PROVIDER_PROFILES.openRouter },
          },
          agents: dedupeCrewAgents(selectedAgents.length > 0 ? selectedAgents : get().agents),
          tasks: [],
          runtimeConfig: { ...DEFAULT_CREW_RUNTIME_CONFIG },
          process: 'sequential',
          managerAgentId: null,
          verbose: true,
          maxRpm: 3,
          maxParallelTasks: 1,
          status: 'idle',
          createdAt: Date.now(),
          updatedAt: Date.now(),
        }
        set(s => ({ crews: [crew, ...s.crews] }))
      },

      createStarterCrew: (name, goal) => {
        if (get().agents.length === 0) {
          get().loadAgents()
        }
        const availableAgents = get().agents
        const preferredRoles: AgentRole[] = ['planner', 'executor', 'reviewer']
        const selectedAgents = preferredRoles
          .map((role) => availableAgents.find((agent) => agent.role === role))
          .filter((agent): agent is CrewAgent => Boolean(agent))
        for (const agent of availableAgents) {
          if (selectedAgents.length >= 3) break
          if (!selectedAgents.some((selected) => selected.id === agent.id)) selectedAgents.push(agent)
        }
        if (selectedAgents.length === 0) {
          throw new Error('No crew agents are available.')
        }

        const crewId = crypto.randomUUID()
        get().createCrew(crewId, name.trim() || 'New crew', selectedAgents.map((agent) => agent.id))
        const normalizedGoal = goal.trim() || `Complete ${name.trim() || 'the requested task'}`
        const planTaskId = crypto.randomUUID()
        const executeTaskId = crypto.randomUUID()
        const reviewTaskId = crypto.randomUUID()
        const planner = selectedAgents[0]
        const executor = selectedAgents[1] ?? planner
        const reviewer = selectedAgents[2] ?? executor

        get().updateCrew(crewId, {
          description: normalizedGoal,
          knowledgeFocus: normalizedGoal,
          managerAgentId: planner.id,
          tasks: [
            {
              id: planTaskId,
              description: `Analyze the objective, retrieve relevant knowledge, and create an executable plan: ${normalizedGoal}`,
              expectedOutput: 'A concise plan with assumptions, evidence needs, and acceptance criteria.',
              agentId: planner.id,
              context: [],
              dependencies: [],
              asyncExecution: false,
              status: 'pending',
              output: null,
            },
            {
              id: executeTaskId,
              description: `Execute the approved plan and produce the requested result: ${normalizedGoal}`,
              expectedOutput: 'A complete result with concrete evidence and any produced artifacts.',
              agentId: executor.id,
              context: [planTaskId],
              dependencies: [planTaskId],
              asyncExecution: false,
              status: 'pending',
              output: null,
            },
            {
              id: reviewTaskId,
              description: `Review the result against the objective, correct gaps, and provide the final synthesis: ${normalizedGoal}`,
              expectedOutput: 'A verified final answer including remaining risks and acceptance status.',
              agentId: reviewer.id,
              context: [planTaskId, executeTaskId],
              dependencies: [executeTaskId],
              asyncExecution: false,
              status: 'pending',
              output: null,
            },
          ],
        })
        get().setActiveCrew(crewId)
        return crewId
      },

      updateCrew: (id, patch) =>
        set(s => ({
          crews: s.crews.map(c => c.id === id ? { ...c, ...patch, updatedAt: Date.now() } : c),
        })),

      setCrewProviderProfiles: async (id, profiles) => {
        await Promise.all([
          setCredential(
            crewProviderLocator(id, 'openai_compatible'),
            profiles.openAICompatible.apiKey,
          ),
          setCredential(
            crewProviderLocator(id, 'openrouter'),
            profiles.openRouter.apiKey,
          ),
        ])
        set((state) => ({
          crews: state.crews.map((crew) => (
            crew.id === id
              ? { ...crew, providerProfiles: profiles, updatedAt: Date.now() }
              : crew
          )),
        }))
      },

      deleteCrew: async (id) => {
        await Promise.all([
          deleteCredential(crewProviderLocator(id, 'openai_compatible')),
          deleteCredential(crewProviderLocator(id, 'openrouter')),
        ])
        set(s => ({
          crews: s.crews.filter(c => c.id !== id),
          activeCrewId: s.activeCrewId === id ? null : s.activeCrewId,
        }))
      },

      setActiveCrew: (id) => set({ activeCrewId: id }),

      addAgent: (agent) =>
        set(s => ({
          agents: s.agents.some(a => a.id === agent.id)
            ? s.agents.map(a => a.id === agent.id ? agent : a)
            : [agent, ...s.agents],
        })),

      updateAgent: (id, patch) =>
        set(s => ({
          agents: s.agents.map(a => a.id === id ? { ...a, ...patch } : a),
        })),

      updateCrewAgent: (crewId, agentId, patch) =>
        set(s => ({
          crews: s.crews.map(c =>
            c.id === crewId
              ? {
                  ...c,
                  agents: c.agents.map(agent =>
                    agent.id === agentId
                      ? {
                          ...agent,
                          ...patch,
                          tools: patch.tools ? [...patch.tools] : agent.tools,
                          mcpServerNames: patch.mcpServerNames ? [...patch.mcpServerNames] : agent.mcpServerNames,
                        }
                      : agent
                  ),
                  updatedAt: Date.now(),
                }
              : c
          ),
        })),

      removeAgent: (id) =>
        set(s => ({ agents: s.agents.filter(a => a.id !== id) })),

      loadAgents: () => {
        const current = get().agents
        if (current.length === 0) {
          set({ agents: [...DEFAULT_AGENTS] })
        }
      },

      syncAgentsFromPersonalityProfiles: (profiles) =>
        set((state) => {
          const syncableProfiles = Array.from(
            new Map(
              profiles
                .filter(isSyncablePersonalityProfile)
                .map((profile) => [profile.id, profile]),
            ).values(),
          )

          if (syncableProfiles.length === 0) {
            return state
          }

          const syncedAgents = syncableProfiles.map(buildCrewAgentFromPersonality)
          const nextAgents = state.agents.length > 0
            ? state.agents.map(cloneCrewAgent)
            : DEFAULT_AGENTS.map(cloneCrewAgent)
          let agentsChanged = state.agents.length === 0

          for (const agent of syncedAgents) {
            const existingIndex = nextAgents.findIndex((existingAgent) => isSamePersonalityAgent(existingAgent, agent.personalityId ?? ''))
            if (existingIndex >= 0) {
              nextAgents[existingIndex] = applyPersonalityProfileToAgent(nextAgents[existingIndex], syncableProfiles.find((profile) => profile.id === agent.personalityId)!)
              agentsChanged = true
              continue
            }

            nextAgents.push(cloneCrewAgent(agent))
            agentsChanged = true
          }

          let crewsChanged = false
          const nextCrews = state.crews.map((crew) => {
            let crewAgentsChanged = false
            let nextCrewAgents = crew.agents.map((agent) => {
              if (!agent.personalityId) {
                return cloneCrewAgent(agent)
              }

              const profile = syncableProfiles.find((entry) => entry.id === agent.personalityId)
              if (!profile) {
                return cloneCrewAgent(agent)
              }

              crewAgentsChanged = true
              return applyPersonalityProfileToAgent(agent, profile)
            })
            const missingAgents = syncedAgents.filter(
              (agent) => !nextCrewAgents.some((existingAgent) => isSamePersonalityAgent(existingAgent, agent.personalityId ?? '')),
            )

            if (missingAgents.length > 0) {
              crewAgentsChanged = true
              nextCrewAgents = [...nextCrewAgents, ...missingAgents.map(cloneCrewAgent)]
            }

            if (!crewAgentsChanged) {
              return crew
            }

            crewsChanged = true
            return {
              ...crew,
              agents: dedupeCrewAgents(nextCrewAgents),
              updatedAt: Date.now(),
            }
          })

          if (!agentsChanged && !crewsChanged) {
            return state
          }

          return {
            agents: dedupeCrewAgents(nextAgents),
            crews: nextCrews,
          }
        }),

      migrateAgentsToPersonalityProfiles: async (profiles) => {
        const state = get()
        const profileById = new Map(profiles.map((profile) => [profile.id, profile]))
        const existingNames = new Set(profiles.map((profile) => profile.name.trim().toLowerCase()))
        const usedProfileIds = new Set(profiles.map((profile) => profile.id))
        const draftsBySignature = new Map<string, CrewPersonalityProfile>()
        const profileIdBySignature = new Map<string, string>()
        const allAgents = [
          ...state.agents,
          ...state.crews.flatMap((crew) => crew.agents),
        ]

        for (const agent of allAgents) {
          if (agent.personalityId && profileById.has(agent.personalityId)) {
            continue
          }

          const signature = buildAgentProfileSignature(agent)
          if (profileIdBySignature.has(signature)) {
            continue
          }

          const baseId = agent.personalityId
            ?? `pers-migrated-${slugifyProfileName(agent.name)}-${draftsBySignature.size + 1}`
          let id = baseId
          let suffix = 2
          while (usedProfileIds.has(id)) {
            id = `${baseId}-${suffix}`
            suffix += 1
          }
          usedProfileIds.add(id)
          const name = createUniqueProfileName(agent.name, existingNames)
          const profile = buildPersonalityProfileFromAgent(agent, id, name)
          draftsBySignature.set(signature, profile)
          profileIdBySignature.set(signature, profile.id)
        }

        if (draftsBySignature.size === 0) {
          return false
        }

        const drafts = Array.from(draftsBySignature.values())
        try {
          for (const profile of drafts) {
            await safeInvoke('personality_upsert', {
              id: profile.id,
              name: profile.name,
              description: profile.description,
              role: profile.role,
              goal: profile.goal,
              systemPrompt: profile.systemPrompt,
              skillsMarkdown: profile.skillsMarkdown,
              temperature: profile.temperature ?? null,
              modelOverride: profile.modelOverride,
              icon: profile.icon ?? null,
              isDefault: profile.isDefault ?? false,
            }, undefined)
          }
        } catch {
          return false
        }

        const allProfiles = [...profiles, ...drafts]
        set((current) => {
          const migrateAgent = (agent: CrewAgent): CrewAgent => {
            if (agent.personalityId && profileById.has(agent.personalityId)) {
              return cloneCrewAgent(agent)
            }

            const profileId = profileIdBySignature.get(buildAgentProfileSignature(agent)) ?? agent.personalityId
            const profile = profileId ? allProfiles.find((entry) => entry.id === profileId) : undefined
            if (!profile) {
              return cloneCrewAgent(agent)
            }

            return applyPersonalityProfileToAgent(
              {
                ...agent,
                personalityId: profile.id,
              },
              profile,
            )
          }

          return {
            agents: dedupeCrewAgents(current.agents.map(migrateAgent)),
            crews: current.crews.map((crew) => ({
              ...crew,
              agents: dedupeCrewAgents(crew.agents.map(migrateAgent)),
              updatedAt: Date.now(),
            })),
          }
        })

        return true
      },

      unlinkPersonalityProfile: (profile) =>
        set((state) => {
          const toSnapshot = (agent: CrewAgent): CrewAgent => {
            if (!isSamePersonalityAgent(agent, profile.id)) {
              return cloneCrewAgent(agent)
            }

            const snapshot = applyPersonalityProfileToAgent(agent, profile)
            return {
              ...snapshot,
              personalityId: null,
            }
          }

          return {
            agents: state.agents.map(toSnapshot),
            crews: state.crews.map((crew) => ({
              ...crew,
              agents: crew.agents.map(toSnapshot),
              updatedAt: Date.now(),
            })),
          }
        }),

      addTask: (crewId, task) =>
        set(s => ({
          crews: s.crews.map(c =>
            c.id === crewId ? { ...c, tasks: [...c.tasks, task], updatedAt: Date.now() } : c
          ),
        })),

      updateTask: (crewId, taskId, patch) =>
        set(s => ({
          crews: s.crews.map(c =>
            c.id === crewId
              ? {
                  ...c,
                  tasks: c.tasks.map(t => t.id === taskId ? { ...t, ...patch } : t),
                  updatedAt: Date.now(),
                }
              : c
          ),
        })),

      removeTask: (crewId, taskId) =>
        set(s => ({
          crews: s.crews.map(c =>
            c.id === crewId
              ? { ...c, tasks: c.tasks.filter(t => t.id !== taskId), updatedAt: Date.now() }
              : c
          ),
        })),

      runCrew: async (crewId) => {
        const state = get()
        const crew = state.crews.find(c => c.id === crewId)
        if (!crew || crew.tasks.length === 0) return

        canceledCrewIds.delete(crewId)

        let config = undefined
        let providerConfigs = undefined
        try {
          config = useConfigStore.getState().ollama
          const { defaultLlmProfileIds, llmProfiles } = useConfigStore.getState()
          const defaultOpenAICompatibleProfile = llmProfiles.find((profile) => profile.id === defaultLlmProfileIds['openai-compatible'] && profile.provider === 'openai-compatible')
            ?? llmProfiles.find((profile) => profile.provider === 'openai-compatible')
          const defaultOpenRouterProfile = llmProfiles.find((profile) => profile.id === defaultLlmProfileIds.openrouter && profile.provider === 'openrouter')
            ?? llmProfiles.find((profile) => profile.provider === 'openrouter')
          providerConfigs = {
            openAICompatible: resolveExternalProviderConfig(
              crew.providerProfiles.openAICompatible,
              defaultOpenAICompatibleProfile,
              defaultOpenAICompatibleProfile?.baseUrl || crew.providerProfiles.openAICompatible.baseUrl || 'https://api.openai.com/v1',
            ),
            openRouter: resolveExternalProviderConfig(
              crew.providerProfiles.openRouter,
              defaultOpenRouterProfile,
              defaultOpenRouterProfile?.baseUrl || crew.providerProfiles.openRouter.baseUrl || 'https://openrouter.ai/api/v1',
            ),
          }
        } catch {
          // use backend defaults
        }

        config = resolveCrewRuntimeConfig(crew, config)
        const crewDefaultProvider = crew.defaultProvider ?? DEFAULT_CREW_PROVIDER
        const crewDefaultModel = crew.defaultModel?.trim() ?? ''

        if (crewDefaultModel) {
          if (crewDefaultProvider === 'ollama') {
            config = {
              ...(config ?? {}),
              model: crewDefaultModel,
            }
          } else if (crewDefaultProvider === 'openai-compatible' && providerConfigs?.openAICompatible) {
            providerConfigs = {
              ...providerConfigs,
              openAICompatible: {
                ...providerConfigs.openAICompatible,
                model: crewDefaultModel,
              },
            }
          } else if (crewDefaultProvider === 'openrouter' && providerConfigs?.openRouter) {
            providerConfigs = {
              ...providerConfigs,
              openRouter: {
                ...providerConfigs.openRouter,
                model: crewDefaultModel,
              },
            }
          }
        }

        const personalityProfiles = await loadPersonalityProfilesForRuntime()
        const resolvedCrewAgents = resolveCrewAgentsWithProfiles(crew.agents, personalityProfiles)
        const enabledAgents = resolvedCrewAgents.filter((agent) => agent.enabled)
        const enabledAgentIds = new Set(enabledAgents.map((agent) => agent.id))
        const runnableTasks = crew.tasks.filter((task) => enabledAgentIds.has(task.agentId))
        const blockedTasks = crew.tasks.filter((task) => !enabledAgentIds.has(task.agentId))
        const blockedTaskIds = new Set(blockedTasks.map((task) => task.id))

        if (enabledAgents.length === 0) {
          const message = 'No active Crew-members available.'
          set((s) => ({
            crews: s.crews.map((entry) =>
              entry.id === crewId
                ? {
                    ...entry,
                    status: 'failed',
                    tasks: entry.tasks.map((task) => ({
                      ...task,
                      status: 'failed',
                      output: task.output ?? message,
                    })),
                    updatedAt: Date.now(),
                  }
                : entry
            ),
          }))
          get().addLog(createExecutionLog(
            crewId,
            crew.managerAgentId ?? crew.agents[0]?.id ?? 'crew-manager',
            crew.tasks[0]?.id ?? 'crew-start',
            'Crew-Execution blockiert',
            message,
          ))
          return
        }

        set(s => ({
          crews: s.crews.map(c =>
            c.id === crewId
              ? {
                  ...c,
                  status: 'running' as const,
                  tasks: c.tasks.map((t, i) => ({
                    ...t,
                    status: blockedTaskIds.has(t.id)
                      ? 'failed'
                      : i === 0 && runnableTasks.length > 0
                        ? 'running'
                        : 'pending',
                    output: blockedTaskIds.has(t.id)
                      ? 'Zugewiesenes Crew-Mitglied ist disabled.'
                      : null,
                  })),
                  updatedAt: Date.now(),
                }
              : c
          ),
        }))

        get().addLog(createExecutionLog(
          crewId,
          crew.managerAgentId ?? crew.agents[0]?.id ?? 'crew-manager',
          runnableTasks[0]?.id ?? crew.tasks[0]?.id ?? 'crew-start',
          'Crew started',
          `${runnableTasks.length} active task(s) are executed through ${crew.process} executed.${blockedTasks.length > 0 ? ` ${blockedTasks.length} task(s) are blocked by disabled crew members.` : ''}`,
        ))

        try {
          const response = await safeInvoke<CrewExecutionResponse>('crew_execute', {
            request: {
              id: crew.id,
              name: crew.name,
              description: crew.description,
              executionSubject: crew.executionSubject,
              executionGuidelines: crew.executionGuidelines,
              knowledgeFocus: crew.knowledgeFocus,
              governanceMode: crew.governanceMode,
              outputMode: crew.outputMode,
              stopOnFailure: crew.stopOnFailure,
              retryCount: crew.retryCount,
              managerReviewEnabled: crew.managerReviewEnabled,
              managerReviewGuidelines: crew.managerReviewGuidelines,
              shareAllTaskOutputs: crew.shareAllTaskOutputs,
              sharedOutputCharLimit: crew.sharedOutputCharLimit,
              providerConfigs,
              process: crew.process,
              managerAgentId: crew.managerAgentId,
              verbose: crew.verbose,
              maxRpm: crew.maxRpm,
              maxParallelTasks: crew.maxParallelTasks,
              agents: enabledAgents.map((agent) => ({
                id: agent.id,
                name: agent.name,
                role: agent.role,
                goal: agent.goal,
                backstory: agent.backstory,
                skillsMarkdown: agent.skillsMarkdown,
                personalityId: agent.personalityId,
                modelOverride: agent.modelOverride?.trim() ? agent.modelOverride : null,
                providerKind: crewDefaultProvider,
                tools: agent.tools,
                mcpServerNames: agent.mcpServerNames,
                enabled: agent.enabled,
                allowDelegation: agent.allowDelegation,
                verbose: agent.verbose,
                maxIterations: agent.maxIterations,
              })),
              tasks: runnableTasks.map((task) => ({
                id: task.id,
                description: task.description,
                expectedOutput: task.expectedOutput,
                agentId: task.agentId,
                context: task.context,
                dependencies: task.dependencies.filter((dependencyId) =>
                  runnableTasks.some((candidate) => candidate.id === dependencyId)
                ),
                asyncExecution: crew.process === 'parallel' ? true : task.asyncExecution,
              })),
              config,
            },
          })

          set((s) => ({
            crews: s.crews.map((entry) =>
              entry.id === crewId
                ? {
                    ...entry,
                    status: response.status,
                    tasks: entry.tasks.map((task) => {
                      const result = response.taskResults.find((taskResult) => taskResult.taskId === task.id)
                      if (!result) {
                        return task
                      }
                      return {
                        ...task,
                        status: result.status === 'canceled' ? 'failed' : result.status,
                        output: result.output,
                      }
                    }),
                    updatedAt: Date.now(),
                  }
                : entry
            ),
          }))

          response.logs.forEach((log) => get().addLog(log))

          if (response.error) {
            get().addLog(createExecutionLog(
              crewId,
              crew.managerAgentId ?? crew.agents[0]?.id ?? 'crew-manager',
              runnableTasks[0]?.id ?? 'crew-start',
              'Crew-Execution meldet Error',
              response.error,
            ))
          }
        } catch (error) {
          const message = error instanceof Error ? error.message : String(error)
          const awaitingApproval = isCrewAwaitingApproval(message)
          set(s => ({
            crews: s.crews.map(c =>
              c.id === crewId
                ? {
                    ...c,
                    status: awaitingApproval ? 'awaiting-approval' as const : 'failed' as const,
                    tasks: awaitingApproval
                      ? c.tasks.map((task) => {
                          if (blockedTaskIds.has(task.id)) {
                            return task
                          }

                          return {
                            ...task,
                            status: 'pending',
                            output: null,
                          }
                        })
                      : c.tasks,
                    updatedAt: Date.now(),
                  }
                : c
            ),
            executionLogs: [
              redactCrewExecutionLog({
                id: `log-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
                crewId,
                agentId: crew.managerAgentId ?? crew.agents[0]?.id ?? 'unknown',
                taskId: crew.tasks[0]?.id ?? 'unknown',
                action: awaitingApproval ? 'Crew is waiting for approval' : 'Crew execution failed',
                result: message,
                timestamp: Date.now(),
              }),
              ...s.executionLogs,
            ].slice(0, 500),
          }))
        }
      },

      stopCrew: async (crewId) => {
        const crew = get().crews.find(entry => entry.id === crewId)
        canceledCrewIds.add(crewId)
        try {
          await safeInvoke('crew_stop', {
            request: {
              crewId,
            },
          })
        } catch {
          // Keep optimistic stop request in UI even if backend stop failed.
        }
        get().addLog(createExecutionLog(
          crewId,
          crew?.managerAgentId ?? crew?.agents[0]?.id ?? 'crew-manager',
          crew?.tasks.find(task => task.status === 'running')?.id ?? 'crew-stop',
          'Stop angefordert',
          'Die Crew wird nach dem laufenden Request beendet.',
        ))
        set((s) => ({
          crews: s.crews.map((entry) =>
            entry.id === crewId
              ? { ...entry, status: 'canceled', updatedAt: Date.now() }
              : entry
          ),
        }))
      },

      addLog: (log) =>
        set(s => ({
          executionLogs: [redactCrewExecutionLog(log), ...s.executionLogs].slice(0, 500),
        })),

      installDefaultAgents: () => {
        set(s => {
          const existing = new Map(s.agents.map(a => [a.id, a]))
          for (const agent of DEFAULT_AGENTS) {
            if (!existing.has(agent.id)) {
              existing.set(agent.id, agent)
            }
          }
          return { agents: Array.from(existing.values()) }
        })
      },
    }),
    {
      name: 'open-cowork-crew',
      merge: (persistedState, currentState) => {
        const typedState = persistedState as Partial<CrewState>
        return {
          ...currentState,
          ...typedState,
          crews: (typedState.crews ?? currentState.crews).map(normalizeCrewStateEntry),
          agents: dedupeCrewAgents(typedState.agents ?? currentState.agents),
          executionLogs: (typedState.executionLogs ?? currentState.executionLogs)
            .map(redactCrewExecutionLog)
            .slice(0, 500),
        }
      },
      partialize: (s) => ({
        crews: sanitizeCrewsForPersistence(s.crews),
        agents: s.agents,
        executionLogs: s.executionLogs.map(redactCrewExecutionLog),
        activeCrewId: s.activeCrewId,
      }),
    }
  )
)
