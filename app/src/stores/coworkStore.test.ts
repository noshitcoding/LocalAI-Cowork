import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useCoworkStore, type ScheduledTask, type ScheduledTaskRun } from './coworkStore'

const safeInvokeMock = vi.hoisted(() => vi.fn(async (_command: string, _args?: unknown, fallback?: unknown) => fallback))

vi.mock('../utils/safeInvoke', () => ({
  safeInvoke: safeInvokeMock,
}))

const scheduledTask: ScheduledTask = {
  id: 'work-1',
  name: 'Scheduled work',
  prompt: 'Run work',
  cronLike: 'daily 09:00',
  taskKind: 'prompt',
  crewId: null,
  crewSnapshotJson: null,
  modelConfigJson: null,
  priority: 100,
  dependsOnTaskIds: [],
  active: true,
  lastRunAt: null,
  nextRunAt: null,
}

const scheduledRun: ScheduledTaskRun = {
  id: 'run-1',
  taskId: 'work-1',
  status: 'completed',
  startedAt: 100,
  finishedAt: 200,
  result: 'done',
  error: null,
}

describe('coworkStore scheduler state', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    useCoworkStore.setState({
      scheduledTasks: [scheduledTask],
      scheduledRuns: [scheduledRun, { ...scheduledRun, id: 'run-other', taskId: 'other' }],
    })
  })

  it('removes scheduled task and runs locally when scheduler delete uses browser fallback', async () => {
    await useCoworkStore.getState().removeScheduledTask('work-1')

    expect(safeInvokeMock).toHaveBeenCalledWith('scheduler_delete_task', { id: 'work-1' }, null)
    expect(useCoworkStore.getState().scheduledTasks).toHaveLength(0)
    expect(useCoworkStore.getState().scheduledRuns).toEqual([
      expect.objectContaining({ id: 'run-other', taskId: 'other' }),
    ])
  })
})
