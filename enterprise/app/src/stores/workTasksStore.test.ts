import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useWorkTasksStore, workTaskMetadataForDaemon } from './workTasksStore'
import { hasTauriRuntime, safeInvoke } from '../utils/safeInvoke'

vi.mock('../utils/safeInvoke', () => ({
  hasTauriRuntime: vi.fn(() => false),
  safeInvoke: vi.fn(async (_cmd: string, _args?: unknown, fallback?: unknown) => fallback),
}))

const hasTauriRuntimeMock = vi.mocked(hasTauriRuntime)
const safeInvokeMock = vi.mocked(safeInvoke)

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

  it('persists create, update, and delete through Tauri commands', async () => {
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

    await vi.waitFor(() => expect(safeInvokeMock).toHaveBeenCalledTimes(3))
    expect(safeInvokeMock).toHaveBeenCalledWith('work_task_upsert', {
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
    expect(safeInvokeMock).toHaveBeenCalledWith('work_task_upsert', {
      request: expect.objectContaining({
        id,
        status: 'completed',
        output: 'Done',
      }),
    })
    expect(safeInvokeMock).toHaveBeenCalledWith('work_task_delete', { id })
  })

  it('keeps a task running during an active in-memory execution', async () => {
    hasTauriRuntimeMock.mockReturnValue(true)
    const id = useWorkTasksStore.getState().addTask({
      title: 'Running report',
      prompt: 'Summarize',
      runner: 'model',
    })

    useWorkTasksStore.getState().updateTask(id, { status: 'running' })

    expect(useWorkTasksStore.getState().tasks[0].status).toBe('running')
    await vi.waitFor(() => expect(safeInvokeMock).toHaveBeenCalledTimes(2))
    expect(safeInvokeMock).toHaveBeenLastCalledWith('work_task_upsert', {
      request: expect.objectContaining({ id, status: 'running' }),
    })
  })

  it('syncs the reusable task definition without local paths or run output', () => {
    const id = useWorkTasksStore.getState().addTask({
      title: 'Scheduled report',
      prompt: 'Create the current report',
      expectedOutput: 'DOCX and PDF',
      workDir: 'C:/private/customer-workspace',
      runner: 'model',
      model: 'qwen3',
      scheduleExpr: 'daily 09:00',
      scheduleEnabled: true,
    })
    useWorkTasksStore.getState().updateTask(id, {
      status: 'completed',
      output: 'private result',
      error: 'private error',
    })

    const payload = workTaskMetadataForDaemon(useWorkTasksStore.getState().tasks[0])
    expect(payload).toMatchObject({
      task_kind: 'work',
      description: 'Create the current report',
      expected_output: 'DOCX and PDF',
      schedule_expression: 'daily 09:00',
    })
    const serialized = JSON.stringify(payload)
    expect(serialized).not.toContain('customer-workspace')
    expect(serialized).not.toContain('private result')
    expect(serialized).not.toContain('private error')
  })

  it('persists empty prompt edits so SQLite does not keep stale task text', async () => {
    hasTauriRuntimeMock.mockReturnValue(true)

    const id = useWorkTasksStore.getState().addTask({
      title: 'Draft',
      prompt: 'Initial prompt',
      runner: 'model',
    })
    await vi.waitFor(() => expect(safeInvokeMock).toHaveBeenCalledTimes(1))
    safeInvokeMock.mockClear()

    useWorkTasksStore.getState().updateTask(id, { prompt: '' })

    expect(useWorkTasksStore.getState().tasks[0].prompt).toBe('')
    await vi.waitFor(() => expect(safeInvokeMock).toHaveBeenCalledTimes(1))
    expect(safeInvokeMock).toHaveBeenCalledWith('work_task_upsert', {
      request: expect.objectContaining({
        id,
        prompt: '',
      }),
    })
  })

  it('serializes task snapshots and keeps delete as the final database mutation', async () => {
    hasTauriRuntimeMock.mockReturnValue(true)
    let releaseFirstUpsert = () => {}
    const firstUpsert = new Promise<unknown>((resolve) => {
      releaseFirstUpsert = () => resolve(undefined)
    })
    let upsertCount = 0
    safeInvokeMock.mockImplementation(async (command, _args, fallback) => {
      if (command === 'work_task_upsert' && ++upsertCount === 1) return firstUpsert
      return fallback
    })

    const id = useWorkTasksStore.getState().addTask({
      title: 'Ordered task',
      prompt: 'Initial',
      runner: 'model',
    })
    useWorkTasksStore.getState().updateTask(id, { prompt: 'Newest', status: 'completed' })
    useWorkTasksStore.getState().removeTask(id)

    expect(safeInvokeMock).toHaveBeenCalledTimes(1)
    releaseFirstUpsert()
    await vi.waitFor(() => expect(safeInvokeMock).toHaveBeenCalledTimes(3))

    expect(safeInvokeMock.mock.calls.map(([command]) => command)).toEqual([
      'work_task_upsert',
      'work_task_upsert',
      'work_task_delete',
    ])
    expect(safeInvokeMock.mock.calls[1]?.[1]).toEqual({
      request: expect.objectContaining({ id, prompt: 'Newest', status: 'completed' }),
    })
    expect(useWorkTasksStore.getState().tasks).toEqual([])
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
