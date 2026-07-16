import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import { hasTauriRuntime, safeInvoke, safeInvokeVoid } from '../utils/safeInvoke'

const LEGACY_STORAGE_KEY = 'open-cowork-work-tasks'
const SQLITE_MIGRATION_FLAG = 'open-cowork-work-tasks-sqlite-migrated'

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

type BackendWorkTask = {
  id: string
  title: string
  prompt: string
  expectedOutput?: string
  expected_output?: string
  workDir?: string
  work_dir?: string
  threadId?: string | null
  thread_id?: string | null
  runner: string
  crewId?: string | null
  crew_id?: string | null
  model?: string
  scheduleExpr?: string
  schedule_expr?: string
  scheduleEnabled?: boolean
  schedule_enabled?: boolean
  status?: string
  output?: string | null
  error?: string | null
  lastRunAt?: string | number | null
  last_run_at?: string | number | null
  createdAt?: string | number
  created_at?: string | number
  updatedAt?: string | number
  updated_at?: string | number
}

type RawWorkTask = Partial<WorkTask> & {
  id?: unknown
  expected_output?: unknown
  work_dir?: unknown
  thread_id?: unknown
  crew_id?: unknown
  schedule_expr?: unknown
  schedule_enabled?: unknown
  last_run_at?: unknown
  created_at?: unknown
  updated_at?: unknown
}

type WorkTasksState = {
  tasks: WorkTask[]
  loadFromDb: () => Promise<void>
  addTask: (input: WorkTaskInput) => string
  updateTask: (id: string, patch: Partial<Omit<WorkTask, 'id' | 'createdAt'>>) => void
  removeTask: (id: string) => void
  upsertMany: (tasks: WorkTask[]) => void
}

function generateId(): string {
  return `task-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}

function parseDate(value: unknown, fallback = Date.now()): number {
  if (typeof value === 'number' && Number.isFinite(value)) return value
  if (typeof value === 'string' && value.trim()) {
    const parsed = new Date(value).getTime()
    if (Number.isFinite(parsed)) return parsed
  }
  return fallback
}

function toIso(timestamp: number): string {
  return new Date(timestamp).toISOString()
}

function optionalString(value: unknown): string | null {
  return typeof value === 'string' ? value : null
}

function pickString(...values: unknown[]): string {
  for (const value of values) {
    if (typeof value === 'string') return value
  }
  return ''
}

function pickNullableString(...values: unknown[]): string | null {
  for (const value of values) {
    if (value === null) return null
    if (typeof value === 'string') return value
  }
  return null
}

function normalizeRunner(value: unknown): WorkTaskRunner {
  return value === 'model' ? 'model' : 'crew'
}

function normalizeStatus(value: unknown): WorkTaskStatus {
  const status = value === 'idle'
    || value === 'waiting_approval'
    || value === 'running'
    || value === 'completed'
    || value === 'failed'
    || value === 'canceled'
    ? value
    : 'idle'

  return status === 'running' ? 'failed' : status
}

export function normalizeTask(raw: RawWorkTask): WorkTask | null {
  if (typeof raw.id !== 'string' || !raw.id.trim()) return null

  const createdAt = parseDate(raw.createdAt ?? raw.created_at, Date.now())
  const updatedAt = parseDate(raw.updatedAt ?? raw.updated_at, createdAt)
  const rawStatus = raw.status === 'idle'
    || raw.status === 'waiting_approval'
    || raw.status === 'running'
    || raw.status === 'completed'
    || raw.status === 'failed'
    || raw.status === 'canceled'
    ? raw.status
    : 'idle'
  const status = normalizeStatus(raw.status)
  const interrupted = rawStatus === 'running'
  const runner = normalizeRunner(raw.runner)
  const scheduleExpr = pickString(raw.scheduleExpr, raw.schedule_expr).trim()
  const prompt = pickString(raw.prompt).trim()
  const threadId = pickNullableString(raw.threadId, raw.thread_id)
  const crewId = runner === 'crew' ? pickNullableString(raw.crewId, raw.crew_id) : null
  const model = runner === 'model' ? pickString(raw.model).trim() : ''

  return {
    id: raw.id.trim(),
    title: pickString(raw.title).trim(),
    prompt,
    expectedOutput: pickString(raw.expectedOutput, raw.expected_output).trim(),
    workDir: pickString(raw.workDir, raw.work_dir).trim(),
    threadId: threadId?.trim() ? threadId : null,
    runner,
    crewId: crewId?.trim() ? crewId : null,
    model,
    scheduleExpr,
    scheduleEnabled: Boolean(raw.scheduleEnabled ?? raw.schedule_enabled) && Boolean(scheduleExpr),
    status,
    output: optionalString(raw.output),
    error: interrupted ? 'Task-Run was unterbrochen.' : optionalString(raw.error),
    lastRunAt: raw.lastRunAt ?? raw.last_run_at ? parseDate(raw.lastRunAt ?? raw.last_run_at, Date.now()) : null,
    createdAt,
    updatedAt: interrupted ? Date.now() : updatedAt,
  }
}

function mapBackendWorkTask(row: BackendWorkTask): WorkTask | null {
  return normalizeTask(row as RawWorkTask)
}

function toBackendRequest(task: WorkTask): BackendWorkTask {
  return {
    id: task.id,
    title: task.title,
    prompt: task.prompt,
    expectedOutput: task.expectedOutput,
    workDir: task.workDir,
    threadId: task.threadId,
    runner: task.runner,
    crewId: task.runner === 'crew' ? task.crewId : null,
    model: task.runner === 'model' ? task.model : '',
    scheduleExpr: task.scheduleExpr,
    scheduleEnabled: task.scheduleEnabled,
    status: task.status,
    output: task.output,
    error: task.error,
    lastRunAt: task.lastRunAt ? toIso(task.lastRunAt) : null,
    createdAt: toIso(task.createdAt),
    updatedAt: toIso(task.updatedAt),
  }
}

function persistWorkTask(task: WorkTask): void {
  if (!task.id.trim()) return
  void safeInvokeVoid('work_task_upsert', {
    request: toBackendRequest(task),
  })
}

function parseLegacyStorage(): WorkTask[] {
  if (typeof window === 'undefined') return []
  const raw = window.localStorage.getItem(LEGACY_STORAGE_KEY)
  if (!raw) return []

  try {
    const parsed = JSON.parse(raw) as { state?: unknown; tasks?: unknown }
    const state = (parsed.state && typeof parsed.state === 'object' ? parsed.state : parsed) as { tasks?: unknown }
    return Array.isArray(state.tasks)
      ? state.tasks
          .map((task) => normalizeTask(task as RawWorkTask))
          .filter((task): task is WorkTask => Boolean(task))
      : []
  } catch {
    return []
  }
}

async function migrateLegacyStorageToSqlite(): Promise<void> {
  if (!hasTauriRuntime() || typeof window === 'undefined') return
  if (window.localStorage.getItem(SQLITE_MIGRATION_FLAG) === 'true') return

  const legacyTasks = parseLegacyStorage()
  for (const task of legacyTasks) {
    await safeInvoke<BackendWorkTask>('work_task_upsert', {
      request: toBackendRequest(task),
    })
  }

  window.localStorage.setItem(SQLITE_MIGRATION_FLAG, 'true')
}

function mergeTaskPatch(task: WorkTask, patch: Partial<Omit<WorkTask, 'id' | 'createdAt'>>): WorkTask {
  const runner = patch.runner ?? task.runner
  const scheduleExpr = patch.scheduleExpr !== undefined ? patch.scheduleExpr : task.scheduleExpr
  const next = normalizeTask({
    ...task,
    ...patch,
    runner,
    crewId: runner === 'crew'
      ? patch.crewId !== undefined
        ? patch.crewId
        : task.crewId
      : null,
    model: runner === 'model'
      ? patch.model !== undefined
        ? patch.model
        : task.model
      : '',
    scheduleExpr,
    scheduleEnabled: patch.scheduleEnabled !== undefined
      ? Boolean(patch.scheduleEnabled) && Boolean(scheduleExpr.trim())
      : scheduleExpr.trim()
        ? task.scheduleEnabled
        : false,
    updatedAt: Date.now(),
  })

  return next ?? task
}

export const useWorkTasksStore = create<WorkTasksState>()(
  persist(
    (set) => ({
      tasks: [],

      loadFromDb: async () => {
        if (!hasTauriRuntime()) {
          const legacyTasks = parseLegacyStorage()
          if (legacyTasks.length > 0) {
            set({ tasks: legacyTasks })
          }
          return
        }

        try {
          await migrateLegacyStorageToSqlite()
          const dbTasks = await safeInvoke<BackendWorkTask[]>('work_task_list', undefined, [])
          const tasks = Array.isArray(dbTasks)
            ? dbTasks
                .map(mapBackendWorkTask)
                .filter((task): task is WorkTask => Boolean(task))
            : []
          set({ tasks })
        } catch (error) {
          console.warn('[workTasksStore] loadFromDb failed', error)
        }
      },

      addTask: (input) => {
        const prompt = input.prompt.trim()
        if (!prompt) {
          throw new Error('Task prompt must not be empty.')
        }

        const now = Date.now()
        const task: WorkTask = {
          id: generateId(),
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
        persistWorkTask(task)

        return task.id
      },

      updateTask: (id, patch) => {
        let updatedTask: WorkTask | null = null
        set((state) => ({
          tasks: state.tasks.map((task) => {
            if (task.id !== id) return task
            updatedTask = mergeTaskPatch(task, patch)
            return updatedTask
          }),
        }))
        if (updatedTask) persistWorkTask(updatedTask)
      },

      removeTask: (id) => {
        set((state) => ({
          tasks: state.tasks.filter((task) => task.id !== id),
        }))
        void safeInvokeVoid('work_task_delete', { id })
      },

      upsertMany: (incoming) => {
        const normalizedTasks: WorkTask[] = []
        set((state) => {
          const existing = new Map(state.tasks.map((task) => [task.id, task]))
          for (const raw of incoming) {
            const normalized = normalizeTask(raw as RawWorkTask)
            if (!normalized) continue
            existing.set(normalized.id, normalized)
            normalizedTasks.push(normalized)
          }

          return {
            tasks: Array.from(existing.values()).sort((a, b) => b.createdAt - a.createdAt),
          }
        })
        normalizedTasks.forEach(persistWorkTask)
      },
    }),
    {
      name: LEGACY_STORAGE_KEY,
      merge: (persistedState, currentState) => {
        const typedState = persistedState as Partial<WorkTasksState>
        const tasks = Array.isArray((typedState as { tasks?: unknown }).tasks)
          ? (typedState as { tasks: unknown[] }).tasks
              .map((task) => normalizeTask(task as RawWorkTask))
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
