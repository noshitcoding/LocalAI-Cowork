import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useWorkTasksStore } from './workTasksStore'
import { hasTauriRuntime, safeInvoke, safeInvokeVoid } from '../utils/safeInvoke'

vi.mock('../utils/safeInvoke', () => ({
  hasTauriRuntime: vi.fn(() => false),
  safeInvoke: vi.fn(async (_cmd: string, _args?: unknown, fallback?: unknown) => fallback),
  safeInvokeVoid: vi.fn(async () => undefined),
}))

const hasTauriRuntimeMock = vi.mocked(hasTauriRuntime)
const safeInvokeMock = vi.mocked(safeInvoke)
const safeInvokeVoidMock = vi.mocked(safeInvokeVoid)

function resetStore() {
  window.localStorage.removeItem('open-cowork-work-tasks')
  window.localStorage.removeItem('open-cowork-work-tasks-sqlite-migrated')
  useWorkTasksStore.setState({ tasks: [] })
}

describe('workTasksStore', () => {
  beforeEach(() => {
    resetStore()
    hasTauriRuntimeMock.mockReturnValue(false)
    safeInvokeMock.mockReset()
    safeInvokeMock.mockImplementation(async (_cmd, _args, fallback) => fallback)
    safeInvokeVoidMock.mockReset()
    safeInvokeVoidMock.mockResolvedValue(undefined)
  })

  it('uses local state and localStorage fallback outside Tauri', async () => {
    window.localStorage.setItem('open-cowork-work-tasks', JSON.stringify({
      state: {
        tasks: [{
          id: 'legacy-1',
          title: 'Legacy',
          prompt: 'Do it',
          runner: 'model',
          status: 'running',
          createdAt: 100,
          updatedAt: 100,
        }],
      },
    }))

    await useWorkTasksStore.getState().loadFromDb()

    const task = useWorkTasksStore.getState().tasks[0]
    expect(task.id).toBe('legacy-1')
    expect(task.status).toBe('failed')
    expect(task.error).toBe('Task-Run was unterbrochen.')
  })

  it('persists create, update, and delete through Tauri commands', () => {
    hasTauriRuntimeMock.mockReturnValue(true)

    const id = useWorkTasksStore.getState().addTask({
      title: 'Report',
      prompt: 'Summarize',
      expectedOutput: 'Bullets',
      workDir: 'C:/work',
      runner: 'model',
      model: 'qwen3',
    })
    useWorkTasksStore.getState().updateTask(id, {
      status: 'completed',
      output: 'Done',
      lastRunAt: 123,
    })
    useWorkTasksStore.getState().removeTask(id)

    expect(safeInvokeVoidMock).toHaveBeenCalledWith('work_task_upsert', {
      request: expect.objectContaining({
        id,
        title: 'Report',
        prompt: 'Summarize',
        expectedOutput: 'Bullets',
        workDir: 'C:/work',
        runner: 'model',
        model: 'qwen3',
      }),
    })
    expect(safeInvokeVoidMock).toHaveBeenCalledWith('work_task_upsert', {
      request: expect.objectContaining({
        id,
        status: 'completed',
        output: 'Done',
      }),
    })
    expect(safeInvokeVoidMock).toHaveBeenCalledWith('work_task_delete', { id })
  })

  it('persists empty prompt edits so SQLite does not keep stale task text', () => {
    hasTauriRuntimeMock.mockReturnValue(true)

    const id = useWorkTasksStore.getState().addTask({
      title: 'Draft',
      prompt: 'Initial prompt',
      runner: 'model',
    })
    safeInvokeVoidMock.mockClear()

    useWorkTasksStore.getState().updateTask(id, { prompt: '' })

    expect(useWorkTasksStore.getState().tasks[0].prompt).toBe('')
    expect(safeInvokeVoidMock).toHaveBeenCalledWith('work_task_upsert', {
      request: expect.objectContaining({
        id,
        prompt: '',
      }),
    })
  })

  it('migrates legacy localStorage tasks to SQLite once and loads db rows', async () => {
    hasTauriRuntimeMock.mockReturnValue(true)
    window.localStorage.setItem('open-cowork-work-tasks', JSON.stringify({
      state: {
        tasks: [{
          id: 'legacy-1',
          title: 'Legacy',
          prompt: 'Do it',
          expectedOutput: 'Text',
          runner: 'crew',
          crewId: 'crew-1',
          scheduleExpr: 'daily 09:00',
          scheduleEnabled: true,
          status: 'idle',
          createdAt: 100,
          updatedAt: 200,
        }, {
          id: 'legacy-draft',
          title: 'Legacy Draft',
          prompt: '',
          expectedOutput: '',
          runner: 'model',
          status: 'idle',
          createdAt: 300,
          updatedAt: 400,
        }],
      },
    }))
    safeInvokeMock.mockImplementation(async (cmd, args, fallback) => {
      if (cmd === 'work_task_upsert') return (args as { request: unknown }).request
      if (cmd === 'work_task_list') {
        return [{
          id: 'db-1',
          title: 'From DB',
          prompt: 'Persisted',
          expectedOutput: '',
          workDir: '',
          threadId: null,
          runner: 'model',
          crewId: null,
          model: '',
          scheduleExpr: '',
          scheduleEnabled: false,
          status: 'idle',
          output: null,
          error: null,
          lastRunAt: null,
          createdAt: '2026-07-02T08:00:00Z',
          updatedAt: '2026-07-02T08:00:00Z',
        }]
      }
      return fallback
    })

    await useWorkTasksStore.getState().loadFromDb()

    expect(window.localStorage.getItem('open-cowork-work-tasks-sqlite-migrated')).toBe('true')
    expect(safeInvokeMock).toHaveBeenCalledWith('work_task_upsert', {
      request: expect.objectContaining({
        id: 'legacy-1',
        runner: 'crew',
        crewId: 'crew-1',
        scheduleEnabled: true,
      }),
    })
    expect(safeInvokeMock).toHaveBeenCalledWith('work_task_upsert', {
      request: expect.objectContaining({
        id: 'legacy-draft',
        title: 'Legacy Draft',
        prompt: '',
        runner: 'model',
      }),
    })
    expect(useWorkTasksStore.getState().tasks).toHaveLength(1)
    expect(useWorkTasksStore.getState().tasks[0].id).toBe('db-1')
  })
})
