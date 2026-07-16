import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import type { PermissionMode } from '../engine/types/tool'

export type TaskStatus =
  | 'created'
  | 'planned'
  | 'waiting_approval'
  | 'running'
  | 'completed'
  | 'failed'
  | 'cancelled'

export type PlanApprovalStatus = TaskStatus

export type TaskStep = {
  id: string
  index: number
  title: string
  state: 'pending' | 'running' | 'completed' | 'failed' | 'skipped'
  requiresApproval: boolean
  riskLevel: 'low' | 'medium' | 'high'
  output: string | null
}

export type PlanApprovalStep = TaskStep

export type PermissionConfig = {
  mode: PermissionMode
  allowedDirectories: string[]
}

export type Task = {
  id: string
  title: string
  prompt: string
  status: TaskStatus
  steps: TaskStep[]
  threadId: string | null
  createdAt: number
  updatedAt: number
  error: string | null
  permissionConfig?: PermissionConfig
}

/**
 * Compatibility alias for the legacy planned-task store.
 * User-facing executable work lives in workTasksStore; this store now represents
 * approval/progress plans that still use the existing DB task tables.
 */
export type PlanApproval = Task

type TaskState = {
  tasks: Task[]
  activeTaskId: string | null
  loadFromDb: () => Promise<void>
  createTask: (prompt: string, title: string, threadId: string | null) => string
  updateTaskStatus: (taskId: string, status: TaskStatus) => void
  setTaskSteps: (taskId: string, steps: TaskStep[]) => void
  updateStep: (taskId: string, stepId: string, patch: Partial<TaskStep>) => void
  setActiveTask: (id: string | null) => void
  setTaskError: (taskId: string, error: string) => void
}

const TASK_STATUSES: TaskStatus[] = ['created', 'planned', 'waiting_approval', 'running', 'completed', 'failed', 'cancelled']
const STEP_STATES: TaskStep['state'][] = ['pending', 'running', 'completed', 'failed', 'skipped']
const RISK_LEVELS: TaskStep['riskLevel'][] = ['low', 'medium', 'high']

function generateId(): string {
  return `${Date.now()}-${Math.random().toString(36).slice(2, 9)}`
}

function normalizeTaskStatus(status: string): TaskStatus {
  if (TASK_STATUSES.includes(status as TaskStatus)) {
    return status === 'running' ? 'failed' : status as TaskStatus
  }
  return 'failed'
}

function normalizeStepState(state: string): TaskStep['state'] {
  return STEP_STATES.includes(state as TaskStep['state']) ? state as TaskStep['state'] : 'failed'
}

function normalizeRiskLevel(riskLevel: string): TaskStep['riskLevel'] {
  return RISK_LEVELS.includes(riskLevel as TaskStep['riskLevel']) ? riskLevel as TaskStep['riskLevel'] : 'medium'
}

function isTauriRuntime(): boolean {
  return typeof window !== 'undefined' && Boolean((window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__)
}

async function persistInvoke(command: string, args: Record<string, unknown>, context: string): Promise<void> {
  if (!isTauriRuntime()) {
    return
  }
  try {
    await invoke(command, args)
  } catch (error) {
    console.error(`[taskStore] ${context} failed`, error)
  }
}

export const useTaskStore = create<TaskState>()((set, get) => ({
  tasks: [],
  activeTaskId: null,

  loadFromDb: async () => {
    try {
      type DbTask = { id: string; title: string; prompt: string; status: string; thread_id: string | null; created_at: string; updated_at: string; error: string | null }
      type DbStep = { id: string; idx: number; title: string; state: string; requires_approval: boolean; risk_level: string; output: string | null }
      const dbTasks = await invoke<DbTask[]>('db_list_tasks')
      const tasks: Task[] = []
      for (const dt of dbTasks) {
        const dbSteps = await invoke<DbStep[]>('db_list_steps', { taskId: dt.id })
        tasks.push({
          id: dt.id,
          title: dt.title,
          prompt: dt.prompt,
          status: normalizeTaskStatus(dt.status),
          threadId: dt.thread_id,
          createdAt: new Date(dt.created_at).getTime(),
          updatedAt: new Date(dt.updated_at).getTime(),
          error: dt.error,
          steps: dbSteps.map((s) => ({
            id: s.id,
            index: s.idx,
            title: s.title,
            state: normalizeStepState(s.state),
            requiresApproval: s.requires_approval,
            riskLevel: normalizeRiskLevel(s.risk_level),
            output: s.output,
          })),
        })
      }
      set({ tasks })
    } catch {
      // DB not available - keep in-memory state
    }
  },

  createTask: (prompt, title, threadId) => {
    const normalizedPrompt = prompt.trim()
    if (!normalizedPrompt) {
      throw new Error('Task prompt must not be empty.')
    }

    const id = generateId()
    const now = Date.now()
    const task: Task = {
      id,
      title,
      prompt: normalizedPrompt,
      status: 'created',
      steps: [],
      threadId,
      createdAt: now,
      updatedAt: now,
      error: null,
    }
    set((state) => ({
      tasks: [task, ...state.tasks],
      activeTaskId: id,
    }))
    const isoNow = new Date(now).toISOString()
    void persistInvoke('db_save_task', {
      id, title, prompt, status: 'created', threadId, createdAt: isoNow,
    }, 'db_save_task')
    return id
  },

  updateTaskStatus: (taskId, status) => {
    if (status === 'running') {
      const task = get().tasks.find((entry) => entry.id === taskId)
      if (task && task.steps.length === 0) {
        get().setTaskError(taskId, 'Task cannot be started without steps.')
        return
      }
    }

    set((state) => ({
      tasks: state.tasks.map((t) =>
        t.id === taskId ? { ...t, status, updatedAt: Date.now() } : t
      ),
    }))
    void persistInvoke('db_update_task_status', { id: taskId, status }, 'db_update_task_status')

    if (status === 'running') {
      void (async () => {
        if (!isTauriRuntime()) {
          return
        }
        try {
          await invoke('execute_task', { taskId })
          await get().loadFromDb()
        } catch (error) {
          const errorMessage = error instanceof Error ? error.message : String(error)
          set((state) => ({
            tasks: state.tasks.map((t) =>
              t.id === taskId
                ? { ...t, error: errorMessage, status: 'failed' as TaskStatus, updatedAt: Date.now() }
                : t
            ),
          }))
          void persistInvoke('db_update_task_status', { id: taskId, status: 'failed' }, 'db_update_task_status failed fallback')
          console.error('[taskStore] execute_task failed', error)
        }
      })()
    }
  },

  setTaskSteps: (taskId, steps) => {
    set((state) => ({
      tasks: state.tasks.map((t) =>
        t.id === taskId ? { ...t, steps, updatedAt: Date.now() } : t
      ),
    }))
    for (const step of steps) {
      void persistInvoke('db_save_step', {
        id: step.id, taskId, idx: step.index, title: step.title,
        stateVal: step.state, requiresApproval: step.requiresApproval, riskLevel: step.riskLevel,
      }, 'db_save_step')
    }
  },

  updateStep: (taskId, stepId, patch) => {
    set((state) => ({
      tasks: state.tasks.map((t) =>
        t.id === taskId
          ? {
              ...t,
              steps: t.steps.map((s) =>
                s.id === stepId ? { ...s, ...patch } : s
              ),
              updatedAt: Date.now(),
            }
          : t
      ),
    }))
    if (patch.state !== undefined) {
      void persistInvoke('db_update_step', { id: stepId, stateVal: patch.state, output: patch.output ?? null }, 'db_update_step')
    }
  },

  setActiveTask: (id) => set({ activeTaskId: id }),

  setTaskError: (taskId, error) => {
    set((state) => ({
      tasks: state.tasks.map((t) =>
        t.id === taskId
          ? { ...t, error, status: 'failed' as TaskStatus, updatedAt: Date.now() }
          : t
      ),
    }))
    void persistInvoke('db_update_task_status', { id: taskId, status: 'failed' }, 'db_update_task_status setTaskError')
  },
}))
