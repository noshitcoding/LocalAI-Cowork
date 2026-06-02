import { create } from 'zustand'
import { persist } from 'zustand/middleware'

export type WorkTaskRunner = 'crew' | 'model'
export type WorkTaskStatus = 'idle' | 'waiting_approval' | 'running' | 'completed' | 'failed' | 'canceled'

export type WorkTask = {
  id: string
  title: string
  prompt: string
  expectedOutput: string
  workDir: string
  threadId: string | null
  runner: WorkTaskRunner
  crewId: string | null
  /** Model override for runner==='model'. Empty means "use current default". */
  model: string
  /** Cron-like expression used by the backend scheduler (e.g. "daily 09:00"). */
  scheduleExpr: string
  /** When true, a scheduler entry should exist and be active. */
  scheduleEnabled: boolean

  status: WorkTaskStatus
  output: string | null
  error: string | null
  lastRunAt: number | null
  createdAt: number
  updatedAt: number
}

type WorkTaskInput = {
  title?: string
  prompt: string
  expectedOutput?: string
  workDir?: string
  threadId?: string | null
  runner: WorkTaskRunner
  crewId?: string | null
  model?: string
  scheduleExpr?: string
  scheduleEnabled?: boolean
}

type WorkTasksState = {
  tasks: WorkTask[]
  addTask: (input: WorkTaskInput) => string
  updateTask: (id: string, patch: Partial<Omit<WorkTask, 'id' | 'createdAt'>>) => void
  removeTask: (id: string) => void
  upsertMany: (tasks: WorkTask[]) => void
}

function generateId(): string {
  return `task-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}

function normalizeTask(raw: Partial<WorkTask> & { id?: unknown }): WorkTask | null {
  if (typeof raw.id !== 'string' || !raw.id.trim()) return null

  const createdAt = typeof raw.createdAt === 'number' ? raw.createdAt : Date.now()
  const updatedAt = typeof raw.updatedAt === 'number' ? raw.updatedAt : createdAt
  const runner: WorkTaskRunner = raw.runner === 'crew' || raw.runner === 'model' ? raw.runner : 'crew'
  const rawStatus = raw.status === 'idle' || raw.status === 'waiting_approval' || raw.status === 'running' || raw.status === 'completed' || raw.status === 'failed' || raw.status === 'canceled'
    ? raw.status
    : 'idle'
  const status: WorkTaskStatus = rawStatus === 'running' ? 'failed' : rawStatus
  const interrupted = rawStatus === 'running'
  const prompt = typeof raw.prompt === 'string' ? raw.prompt.trim() : ''
  const scheduleExpr = typeof raw.scheduleExpr === 'string' ? raw.scheduleExpr.trim() : ''

  return {
    id: raw.id.trim(),
    title: typeof raw.title === 'string' ? raw.title.trim() : '',
    prompt,
    expectedOutput: typeof raw.expectedOutput === 'string' ? raw.expectedOutput.trim() : '',
    workDir: typeof raw.workDir === 'string' ? raw.workDir.trim() : '',
    threadId: typeof raw.threadId === 'string' ? raw.threadId : null,
    runner,
    crewId: typeof raw.crewId === 'string' ? raw.crewId : null,
    model: typeof raw.model === 'string' ? raw.model.trim() : '',
    scheduleExpr,
    scheduleEnabled: Boolean(raw.scheduleEnabled) && Boolean(scheduleExpr),
    status,
    output: typeof raw.output === 'string' ? raw.output : null,
    error: interrupted ? 'Task-Lauf wurde unterbrochen.' : typeof raw.error === 'string' ? raw.error : null,
    lastRunAt: typeof raw.lastRunAt === 'number' ? raw.lastRunAt : null,
    createdAt,
    updatedAt: interrupted ? Date.now() : updatedAt,
  }
}

export const useWorkTasksStore = create<WorkTasksState>()(
  persist(
    (set) => ({
      tasks: [],

      addTask: (input) => {
        const prompt = input.prompt.trim()
        if (!prompt) {
          throw new Error('Task-Prompt darf nicht leer sein.')
        }

        const now = Date.now()
        const id = generateId()
        const task: WorkTask = {
          id,
          title: (input.title ?? '').trim(),
          prompt,
          expectedOutput: (input.expectedOutput ?? '').trim(),
          workDir: (input.workDir ?? '').trim(),
          threadId: typeof input.threadId === 'string' ? input.threadId : null,
          runner: input.runner,
          crewId: input.runner === 'crew' ? (input.crewId ?? null) : null,
          model: input.runner === 'model' ? (input.model ?? '').trim() : '',
          scheduleExpr: (input.scheduleExpr ?? '').trim(),
          scheduleEnabled: Boolean(input.scheduleEnabled) && Boolean((input.scheduleExpr ?? '').trim()),
          status: 'idle',
          output: null,
          error: null,
          lastRunAt: null,
          createdAt: now,
          updatedAt: now,
        }

        set((state) => ({
          tasks: [task, ...state.tasks],
        }))

        return id
      },

      updateTask: (id, patch) =>
        set((state) => ({
          tasks: state.tasks.map((task) =>
            task.id === id
              ? {
                  ...task,
                  ...patch,
                  title: patch.title !== undefined ? patch.title : task.title,
                  prompt: patch.prompt !== undefined ? patch.prompt : task.prompt,
                  expectedOutput: patch.expectedOutput !== undefined ? patch.expectedOutput : task.expectedOutput,
                  workDir: patch.workDir !== undefined ? patch.workDir : task.workDir,
                  threadId: patch.threadId !== undefined ? patch.threadId : task.threadId,
                  runner: patch.runner !== undefined ? patch.runner : task.runner,
                  crewId: patch.runner === 'crew'
                    ? (patch.crewId !== undefined ? patch.crewId : task.crewId)
                    : patch.runner === 'model'
                      ? null
                      : patch.crewId !== undefined
                        ? patch.crewId
                        : task.crewId,
                  model: patch.runner === 'model'
                    ? (patch.model !== undefined ? patch.model : task.model)
                    : patch.runner === 'crew'
                      ? ''
                      : patch.model !== undefined
                        ? patch.model
                        : task.model,
                  scheduleExpr: patch.scheduleExpr !== undefined ? patch.scheduleExpr : task.scheduleExpr,
                  scheduleEnabled: patch.scheduleEnabled !== undefined
                    ? Boolean(patch.scheduleEnabled) && Boolean((patch.scheduleExpr ?? task.scheduleExpr).trim())
                    : patch.scheduleExpr !== undefined && !patch.scheduleExpr.trim()
                      ? false
                      : task.scheduleEnabled,
                  updatedAt: Date.now(),
                }
              : task,
          ),
        })),

      removeTask: (id) =>
        set((state) => ({
          tasks: state.tasks.filter((task) => task.id !== id),
        })),

      upsertMany: (incoming) =>
        set((state) => {
          const existing = new Map(state.tasks.map((task) => [task.id, task]))
          for (const raw of incoming) {
            const normalized = normalizeTask(raw)
            if (!normalized) continue
            existing.set(normalized.id, normalized)
          }

          return {
            tasks: Array.from(existing.values()).sort((a, b) => b.createdAt - a.createdAt),
          }
        }),
    }),
    {
      name: 'open-cowork-work-tasks',
      merge: (persistedState, currentState) => {
        const typedState = persistedState as Partial<WorkTasksState>
        const tasks = Array.isArray((typedState as { tasks?: unknown }).tasks)
          ? (typedState as { tasks: unknown[] }).tasks
              .map((task) => normalizeTask(task as Partial<WorkTask>))
              .filter((task): task is WorkTask => Boolean(task))
          : currentState.tasks

        return {
          ...currentState,
          ...typedState,
          tasks,
        }
      },
      partialize: (state) => ({
        tasks: state.tasks,
      }),
    },
  ),
)
