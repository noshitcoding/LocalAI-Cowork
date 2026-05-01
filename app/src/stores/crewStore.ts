import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import type { OllamaConfig } from './configStore'
import { safeInvoke } from '../utils/safeInvoke'

export type AgentRole = 'researcher' | 'writer' | 'reviewer' | 'planner' | 'executor' | 'analyst' | 'custom'
export type CrewProcess = 'sequential' | 'parallel' | 'hierarchical'
export type CrewProviderKind = 'ollama' | 'openai-compatible' | 'openrouter'
export type CrewOutputMode = 'standard' | 'bullet-report' | 'json'

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
}

export type CrewProviderProfiles = {
  openAICompatible: CrewExternalProviderConfig
  openRouter: CrewExternalProviderConfig
}

export type Crew = {
  id: string
  name: string
  description: string
  executionGuidelines: string
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
  status: 'idle' | 'running' | 'completed' | 'failed' | 'canceled'
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
  updateCrew: (id: string, patch: Partial<Crew>) => void
  deleteCrew: (id: string) => void
  setActiveCrew: (id: string | null) => void

  addAgent: (agent: CrewAgent) => void
  updateAgent: (id: string, patch: Partial<CrewAgent>) => void
  updateCrewAgent: (crewId: string, agentId: string, patch: Partial<CrewAgent>) => void
  removeAgent: (id: string) => void
  loadAgents: () => void

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
    backstory: 'Ein erfahrener Forscher mit Zugang zu vielfaeltigen Quellen. Analysiert Informationen kritisch und liefert fundierte Ergebnisse.',
    skillsMarkdown: '',
    personalityId: null,
    modelOverride: null,
    providerKind: 'ollama',
    tools: ['web_fetch', 'grep', 'glob', 'read_file'],
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
    goal: 'Hochwertige Texte, Dokumentation und Content erstellen',
    backstory: 'Ein versierter Autor der klare, praegnante und gut strukturierte Texte verfasst. Beherrscht verschiedene Schreibstile.',
    skillsMarkdown: '',
    personalityId: null,
    modelOverride: null,
    providerKind: 'ollama',
    tools: ['edit_file', 'read_file', 'glob'],
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
    goal: 'Code und Texte qualitativ pruefen und verbessern',
    backstory: 'Ein erfahrener Code-Reviewer mit Blick fuer Details, Best Practices und potenzielle Probleme.',
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
    goal: 'Komplexe Aufgaben in ausfuehrbare Schritte zerlegen',
    backstory: 'Ein strategischer Denker der komplexe Probleme analysiert und in klare, priorisierte Aktionsplaene uebersetzen kann.',
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
    goal: 'Aufgaben zuverlaessig und effizient ausfuehren',
    backstory: 'Ein zuverlaessiger Ausfuehrer der Plaene praezise umsetzt, Fehler erkennt und selbststaendig loest.',
    skillsMarkdown: '',
    personalityId: null,
    modelOverride: null,
    providerKind: 'ollama',
    tools: ['bash', 'edit_file', 'read_file', 'glob', 'grep'],
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
    goal: 'Daten analysieren, Muster erkennen und Empfehlungen ableiten',
    backstory: 'Ein Datenanalyst der Zusammenhaenge erkennt, Metriken auswertet und datengetriebene Empfehlungen ausspricht.',
    skillsMarkdown: '',
    personalityId: null,
    modelOverride: null,
    providerKind: 'ollama',
    tools: ['read_file', 'grep', 'glob', 'web_fetch'],
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
  fallbackConfig: { baseUrl?: string; model?: string; apiKey?: string } | undefined,
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
  }
}

function normalizeCrewStateEntry(crew: Crew): Crew {
  return {
    ...crew,
    executionGuidelines: crew.executionGuidelines ?? '',
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
          executionGuidelines: '',
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
          maxRpm: 10,
          maxParallelTasks: 3,
          status: 'idle',
          createdAt: Date.now(),
          updatedAt: Date.now(),
        }
        set(s => ({ crews: [crew, ...s.crews] }))
      },

      updateCrew: (id, patch) =>
        set(s => ({
          crews: s.crews.map(c => c.id === id ? { ...c, ...patch, updatedAt: Date.now() } : c),
        })),

      deleteCrew: (id) =>
        set(s => ({
          crews: s.crews.filter(c => c.id !== id),
          activeCrewId: s.activeCrewId === id ? null : s.activeCrewId,
        })),

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
          const configStore = await import('./configStore')
          config = configStore.useConfigStore.getState().ollama
          const { defaultLlmProfileIds, llmProfiles } = configStore.useConfigStore.getState()
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

        const enabledAgents = crew.agents.filter((agent) => agent.enabled)
        const enabledAgentIds = new Set(enabledAgents.map((agent) => agent.id))
        const runnableTasks = crew.tasks.filter((task) => enabledAgentIds.has(task.agentId))
        const blockedTasks = crew.tasks.filter((task) => !enabledAgentIds.has(task.agentId))
        const blockedTaskIds = new Set(blockedTasks.map((task) => task.id))

        if (enabledAgents.length === 0) {
          const message = 'Keine aktiven Crew-Mitglieder vorhanden.'
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
            'Crew-Ausfuehrung blockiert',
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
                      ? 'Zugewiesenes Crew-Mitglied ist deaktiviert.'
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
          'Crew gestartet',
          `${runnableTasks.length} aktive Task(s) werden ueber ${crew.process} ausgefuehrt.${blockedTasks.length > 0 ? ` ${blockedTasks.length} Task(s) sind durch deaktivierte Crew-Mitglieder blockiert.` : ''}`,
        ))

        try {
          const response = await safeInvoke<CrewExecutionResponse>('crew_execute', {
            request: {
              id: crew.id,
              name: crew.name,
              description: crew.description,
              executionGuidelines: crew.executionGuidelines,
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
                providerKind: agent.providerKind || crewDefaultProvider,
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
              'Crew-Ausfuehrung meldet Fehler',
              response.error,
            ))
          }
        } catch (error) {
          const message = error instanceof Error ? error.message : String(error)
          set(s => ({
            crews: s.crews.map(c =>
              c.id === crewId
                ? { ...c, status: 'failed' as const, updatedAt: Date.now() }
                : c
            ),
            executionLogs: [
              {
                id: `log-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
                crewId,
                agentId: crew.managerAgentId ?? crew.agents[0]?.id ?? 'unknown',
                taskId: crew.tasks[0]?.id ?? 'unknown',
                action: 'Crew-Ausfuehrung fehlgeschlagen',
                result: message,
                timestamp: Date.now(),
              },
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
          executionLogs: [log, ...s.executionLogs].slice(0, 500),
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
        }
      },
      partialize: (s) => ({
        crews: s.crews,
        agents: s.agents,
        executionLogs: s.executionLogs,
        activeCrewId: s.activeCrewId,
      }),
    }
  )
)
