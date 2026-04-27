import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import { safeInvoke } from '../utils/safeInvoke'

export type AgentRole = 'researcher' | 'writer' | 'reviewer' | 'planner' | 'executor' | 'analyst' | 'custom'

export type CrewAgent = {
  id: string
  name: string
  role: AgentRole
  goal: string
  backstory: string
  personalityId: string | null
  modelOverride: string | null
  tools: string[]
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

export type Crew = {
  id: string
  name: string
  description: string
  agents: CrewAgent[]
  tasks: CrewTask[]
  process: 'sequential' | 'hierarchical'
  managerAgentId: string | null
  verbose: boolean
  maxRpm: number
  status: 'idle' | 'running' | 'completed' | 'failed'
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

type ChatTurnResponse = {
  assistantMessage: string
  endpoint: string
  model: string
  requiresApproval: boolean
  proposedPlan: string[]
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
    personalityId: null,
    modelOverride: null,
    tools: ['web_fetch', 'grep', 'glob', 'read_file'],
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
    personalityId: null,
    modelOverride: null,
    tools: ['edit_file', 'read_file', 'glob'],
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
    personalityId: null,
    modelOverride: null,
    tools: ['read_file', 'grep', 'glob'],
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
    personalityId: null,
    modelOverride: null,
    tools: ['todo', 'read_file', 'glob', 'grep'],
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
    personalityId: null,
    modelOverride: null,
    tools: ['bash', 'edit_file', 'read_file', 'glob', 'grep'],
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
    personalityId: null,
    modelOverride: null,
    tools: ['read_file', 'grep', 'glob', 'web_fetch'],
    allowDelegation: true,
    verbose: true,
    maxIterations: 8,
  },
]

const canceledCrewIds = new Set<string>()

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

function buildAgentSystemPrompt(crew: Crew, agent: CrewAgent): string {
  return [
    `Du bist der Agent "${agent.name}" in der Crew "${crew.name}".`,
    `Rolle: ${agent.role}.`,
    `Ziel: ${agent.goal || 'kein Ziel angegeben'}.`,
    `Hintergrund: ${agent.backstory || 'kein Hintergrund angegeben'}.`,
    `Delegation erlaubt: ${agent.allowDelegation ? 'ja' : 'nein'}.`,
    `Maximale Iterationen: ${agent.maxIterations}.`,
    'Arbeite praezise, liefere nur das Ergebnis der aktuellen Aufgabe und nutze vorhandenen Kontext aus frueheren Tasks.',
  ].join('\n')
}

function buildTaskPrompt(crew: Crew, task: CrewTask, contextBlocks: string[]): string {
  const contextSection = contextBlocks.length > 0
    ? `\n\nKontext:\n${contextBlocks.map((entry, index) => `${index + 1}. ${entry}`).join('\n')}`
    : ''

  return [
    `Crew: ${crew.name}`,
    `Task-ID: ${task.id}`,
    `Beschreibung: ${task.description}`,
    `Erwartetes Ergebnis: ${task.expectedOutput || '(nicht angegeben)'}`,
    `Asynchrone Ausfuehrung: ${task.asyncExecution ? 'ja' : 'nein'}`,
    'Liefere direkt die finale Ausgabe fuer diese Aufgabe.',
  ].join('\n') + contextSection
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
          agents: selectedAgents.length > 0 ? selectedAgents : [...get().agents],
          tasks: [],
          process: 'sequential',
          managerAgentId: null,
          verbose: true,
          maxRpm: 10,
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
        try {
          const configStore = await import('./configStore')
          config = configStore.useConfigStore.getState().ollama
        } catch {
          // use backend defaults
        }

        set(s => ({
          crews: s.crews.map(c =>
            c.id === crewId
              ? {
                  ...c,
                  status: 'running' as const,
                  tasks: c.tasks.map((t, i) => ({
                    ...t,
                    status: i === 0 ? 'running' as const : 'pending' as const,
                    output: null,
                  })),
                  updatedAt: Date.now(),
                }
              : c
          ),
        }))

        get().addLog(createExecutionLog(
          crewId,
          crew.managerAgentId ?? crew.agents[0]?.id ?? 'crew-manager',
          crew.tasks[0]?.id ?? 'crew-start',
          'Crew gestartet',
          `${crew.tasks.length} Task(s) werden sequenziell ausgefuehrt.`,
        ))

        const taskOutputs = new Map<string, string>()
        let finalStatus: Crew['status'] = 'completed'

        try {
          for (const task of crew.tasks) {
            if (canceledCrewIds.has(crewId)) {
              finalStatus = 'idle'
              get().addLog(createExecutionLog(
                crewId,
                task.agentId,
                task.id,
                'Crew gestoppt',
                'Ausfuehrung vor dem naechsten Task abgebrochen.',
              ))
              break
            }

            const agent = crew.agents.find(entry => entry.id === task.agentId) ?? state.agents.find(entry => entry.id === task.agentId)
            if (!agent) {
              const errorMessage = `Agent ${task.agentId} fuer Task ${task.id} nicht gefunden.`
              finalStatus = 'failed'
              set(s => ({
                crews: s.crews.map(c =>
                  c.id === crewId
                    ? {
                        ...c,
                        status: 'failed',
                        tasks: c.tasks.map(entry => entry.id === task.id ? { ...entry, status: 'failed', output: errorMessage } : entry),
                        updatedAt: Date.now(),
                      }
                    : c
                ),
              }))
              get().addLog(createExecutionLog(crewId, task.agentId, task.id, 'Task fehlgeschlagen', errorMessage))
              break
            }

            set(s => ({
              crews: s.crews.map(c =>
                c.id === crewId
                  ? {
                      ...c,
                      tasks: c.tasks.map(entry =>
                        entry.id === task.id
                          ? { ...entry, status: 'running', output: null }
                          : entry.status === 'running'
                            ? { ...entry, status: 'pending' }
                            : entry
                      ),
                      updatedAt: Date.now(),
                    }
                  : c
              ),
            }))

            get().addLog(createExecutionLog(
              crewId,
              agent.id,
              task.id,
              'Task gestartet',
              `${agent.name} bearbeitet: ${task.description}`,
            ))

            const contextBlocks = [
              ...task.context.filter(entry => entry.trim().length > 0),
              ...task.dependencies
                .map((dependencyId) => taskOutputs.get(dependencyId))
                .filter((entry): entry is string => Boolean(entry))
                .map((entry) => `Abhaengigkeitsergebnis:\n${entry}`),
              ...crew.tasks
                .map((entry) => ({ entry, output: taskOutputs.get(entry.id) }))
                .filter(({ entry, output }) => entry.id !== task.id && Boolean(output))
                .map(({ entry, output }) => `Vorheriger Task ${entry.description}:\n${output}`),
            ]

            const response = await safeInvoke<ChatTurnResponse>('chat_turn', {
              request: {
                prompt: buildTaskPrompt(crew, task, contextBlocks),
                history: [
                  { role: 'system', content: buildAgentSystemPrompt(crew, agent) },
                  ...contextBlocks.map((entry) => ({ role: 'context', content: entry })),
                ],
                config: agent.modelOverride ? { ...(config ?? {}), model: agent.modelOverride } : config,
              },
            })

            const output = response.assistantMessage.trim()
            taskOutputs.set(task.id, output)

            set(s => ({
              crews: s.crews.map(c =>
                c.id === crewId
                  ? {
                      ...c,
                      tasks: c.tasks.map(entry => entry.id === task.id ? { ...entry, status: 'completed', output } : entry),
                      updatedAt: Date.now(),
                    }
                  : c
              ),
            }))

            get().addLog(createExecutionLog(
              crewId,
              agent.id,
              task.id,
              'Task abgeschlossen',
              output.slice(0, 1200),
            ))
          }
        } catch (error) {
          const message = error instanceof Error ? error.message : String(error)
          finalStatus = 'failed'
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

        canceledCrewIds.delete(crewId)

        set(s => ({
          crews: s.crews.map(c =>
            c.id === crewId
              ? {
                  ...c,
                  status: finalStatus,
                  tasks: c.tasks.map(task =>
                    finalStatus === 'idle' && task.status === 'running'
                      ? { ...task, status: 'failed', output: task.output ?? 'Abgebrochen' }
                      : task
                  ),
                  updatedAt: Date.now(),
                }
              : c
          ),
        }))
      },

      stopCrew: async (crewId) => {
        canceledCrewIds.add(crewId)
        const crew = get().crews.find(entry => entry.id === crewId)
        get().addLog(createExecutionLog(
          crewId,
          crew?.managerAgentId ?? crew?.agents[0]?.id ?? 'crew-manager',
          crew?.tasks.find(task => task.status === 'running')?.id ?? 'crew-stop',
          'Stop angefordert',
          'Die Crew wird nach dem laufenden Request beendet.',
        ))
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
      partialize: (s) => ({
        crews: s.crews,
        agents: s.agents,
        activeCrewId: s.activeCrewId,
      }),
    }
  )
)
