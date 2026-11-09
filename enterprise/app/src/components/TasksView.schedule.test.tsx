import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import TasksView from './TasksView'
import { useChatStore } from '../stores/chatStore'
import { useConfigStore } from '../stores/configStore'
import { useCoworkStore, type ScheduledTask } from '../stores/coworkStore'
import { useCrewStore } from '../stores/crewStore'
import { usePersonalityStore } from '../stores/personalityStore'
import { useProjectStore } from '../stores/projectStore'
import { useTaskTemplatesStore } from '../stores/taskTemplatesStore'
import { useUiStore } from '../stores/uiStore'
import { useWorkTasksStore, type WorkTask } from '../stores/workTasksStore'
import { hasTauriRuntime } from '../utils/safeInvoke'
import { deleteDurableLocalSchedule, upsertDurableLocalSchedule } from '../runtime/localDaemonExecution'

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(),
}))

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => vi.fn()),
}))

vi.mock('../utils/safeInvoke', () => ({
  hasTauriRuntime: vi.fn(() => false),
  safeInvoke: vi.fn(async (_command: string, _args?: unknown, fallback?: unknown) => fallback ?? null),
  safeInvokeVoid: vi.fn(async () => undefined),
}))

vi.mock('../runtime/localDaemonExecution', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../runtime/localDaemonExecution')>()
  return {
    ...actual,
    deleteDurableLocalSchedule: vi.fn(async () => undefined),
    upsertDurableCrewSchedule: vi.fn(),
    upsertDurableCodexSchedule: vi.fn(),
    upsertDurableLocalSchedule: vi.fn(async () => ({ next_run_at: null, last_triggered_at: null })),
  }
})

vi.mock('../utils/ollamaStreaming', () => ({
  streamChatTurn: vi.fn(),
}))

const scheduledTask: WorkTask = {
  id: 'task-scheduled',
  title: 'Scheduled Draft',
  prompt: 'Run the scheduled work',
  expectedOutput: '',
  workDir: '',
  threadId: null,
  runner: 'model',
  crewId: null,
  model: '',
  scheduleExpr: 'daily 09:00',
  scheduleEnabled: false,
  status: 'idle',
  output: null,
  error: null,
  lastRunAt: null,
  createdAt: 100,
  updatedAt: 100,
}

describe('TasksView scheduling', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.mocked(hasTauriRuntime).mockReturnValue(false)
    window.localStorage.clear()

    useWorkTasksStore.setState({ tasks: [scheduledTask] })
    useCoworkStore.setState({
      scheduledTasks: [],
      scheduledRuns: [],
      loadScheduledTasks: vi.fn(async () => undefined),
      loadScheduledRuns: vi.fn(async () => undefined),
      upsertScheduledTask: vi.fn(async (task: ScheduledTask) => {
        useCoworkStore.setState({ scheduledTasks: [task] })
      }),
      toggleScheduledTask: vi.fn(async (taskId: string, active: boolean) => {
        useCoworkStore.setState((state) => ({
          scheduledTasks: state.scheduledTasks.map((task) => task.id === taskId ? { ...task, active } : task),
        }))
      }),
      removeScheduledTask: vi.fn(async (taskId: string) => {
        useCoworkStore.setState((state) => ({
          scheduledTasks: state.scheduledTasks.filter((task) => task.id !== taskId),
        }))
      }),
    })
    useCrewStore.setState({
      crews: [],
      agents: [],
      executionLogs: [],
      activeCrewId: null,
      loading: false,
    })
    usePersonalityStore.setState({
      personalities: [],
      activeId: null,
      loading: false,
      error: null,
      loadPersonalities: vi.fn(async () => undefined),
    })
    useTaskTemplatesStore.setState({ templates: [] })
    useChatStore.setState({
      threads: [],
      activeThreadId: null,
      pendingApproval: [],
      busy: false,
      error: null,
    })
    useProjectStore.setState({
      projects: [],
      activeProjectId: null,
    })
    useUiStore.setState({
      activeMode: 'work',
      workingFolder: null,
      workingPathKind: null,
    })

    const config = useConfigStore.getState()
    useConfigStore.setState({
      ollama: {
        ...config.ollama,
        model: 'llama3.1:8b',
      },
      llmProfiles: [],
      defaultLlmProfileIds: {
        ollama: 'default-ollama',
        'openai-compatible': 'default-openai-compatible',
        openrouter: 'default-openrouter',
      },
    })
  })

  it('creates an active scheduler row when enabling a WorkTask schedule for the first time', async () => {
    const user = userEvent.setup()
    const upsertScheduledTask = useCoworkStore.getState().upsertScheduledTask
    const toggleScheduledTask = useCoworkStore.getState().toggleScheduledTask

    render(
      <MemoryRouter>
        <TasksView />
      </MemoryRouter>,
    )

    expect(await screen.findByDisplayValue('daily 09:00')).toBeInTheDocument()

    await user.click(screen.getByRole('checkbox', { name: /Active/ }))

    await waitFor(() => {
      expect(upsertScheduledTask).toHaveBeenCalledWith(expect.objectContaining({
        id: scheduledTask.id,
        active: true,
        cronLike: scheduledTask.scheduleExpr,
        taskKind: 'prompt',
      }))
    })
    expect(toggleScheduledTask).not.toHaveBeenCalled()
    expect(useWorkTasksStore.getState().tasks[0].scheduleEnabled).toBe(true)
  })

  it('removes the matching scheduler row before deleting a WorkTask', async () => {
    const user = userEvent.setup()
    const removeScheduledTask = useCoworkStore.getState().removeScheduledTask
    useCoworkStore.setState({
      scheduledTasks: [{
        id: scheduledTask.id,
        name: scheduledTask.title,
        prompt: scheduledTask.prompt,
        cronLike: scheduledTask.scheduleExpr,
        taskKind: 'prompt',
        crewId: null,
        crewSnapshotJson: null,
        modelConfigJson: null,
        priority: 100,
        dependsOnTaskIds: [],
        active: true,
        lastRunAt: null,
        nextRunAt: null,
      }],
    })

    render(
      <MemoryRouter>
        <TasksView />
      </MemoryRouter>,
    )

    await user.click(await screen.findByRole('button', { name: 'Delete' }))

    await waitFor(() => {
      expect(removeScheduledTask).toHaveBeenCalledWith(scheduledTask.id)
      expect(useWorkTasksStore.getState().tasks).toHaveLength(0)
    })
  })

  it('preserves existing scheduler active state when saving schedule details', async () => {
    const user = userEvent.setup()
    const upsertScheduledTask = useCoworkStore.getState().upsertScheduledTask
    useWorkTasksStore.setState({
      tasks: [{
        ...scheduledTask,
        scheduleEnabled: true,
      }],
    })
    useCoworkStore.setState({
      scheduledTasks: [{
        id: scheduledTask.id,
        name: scheduledTask.title,
        prompt: scheduledTask.prompt,
        cronLike: scheduledTask.scheduleExpr,
        taskKind: 'prompt',
        crewId: null,
        crewSnapshotJson: null,
        modelConfigJson: null,
        priority: 100,
        dependsOnTaskIds: [],
        active: false,
        lastRunAt: null,
        nextRunAt: null,
      }],
    })

    render(
      <MemoryRouter>
        <TasksView />
      </MemoryRouter>,
    )

    await user.click(await screen.findByRole('button', { name: 'Save' }))

    await waitFor(() => {
      expect(upsertScheduledTask).toHaveBeenCalledWith(expect.objectContaining({
        id: scheduledTask.id,
        active: false,
      }))
    })
  })

  it('shows a failed create without committing the WorkTask and unlocks the controls', async () => {
    const user = userEvent.setup()
    useCoworkStore.setState({
      upsertScheduledTask: vi.fn(async () => { throw new Error('local scheduler unavailable') }),
    })

    render(
      <MemoryRouter>
        <TasksView />
      </MemoryRouter>,
    )
    const active = await screen.findByRole('checkbox', { name: /Active/ })
    await user.click(active)

    expect(await screen.findByRole('alert')).toHaveTextContent('local scheduler unavailable')
    expect(active).not.toBeDisabled()
    expect(useWorkTasksStore.getState().tasks[0].scheduleEnabled).toBe(false)
  })

  it('keeps controls busy and rolls a failed daemon toggle back to the prior scheduler row', async () => {
    const user = userEvent.setup()
    vi.mocked(hasTauriRuntime).mockReturnValue(true)
    let rejectDaemon!: (cause: Error) => void
    vi.mocked(upsertDurableLocalSchedule).mockImplementation(() => new Promise((_resolve, reject) => {
      rejectDaemon = reject
    }))
    const priorSchedule: ScheduledTask = {
      id: scheduledTask.id,
      name: scheduledTask.title,
      prompt: scheduledTask.prompt,
      cronLike: scheduledTask.scheduleExpr,
      taskKind: 'prompt',
      crewId: null,
      crewSnapshotJson: null,
      modelConfigJson: null,
      priority: 100,
      dependsOnTaskIds: [],
      active: false,
      lastRunAt: null,
      nextRunAt: null,
    }
    useWorkTasksStore.setState({ tasks: [{ ...scheduledTask, model: 'llama3.1:8b' }] })
    useCoworkStore.setState({ scheduledTasks: [priorSchedule] })

    render(
      <MemoryRouter>
        <TasksView />
      </MemoryRouter>,
    )
    const active = await screen.findByRole('checkbox', { name: /Active/ })
    await user.click(active)

    await waitFor(() => {
      const alert = screen.queryByRole('alert')
      if (alert) throw new Error(`Unexpected scheduler error before daemon mutation: ${alert.textContent}`)
      expect(upsertDurableLocalSchedule).toHaveBeenCalledTimes(1)
    })
    expect(active).toBeDisabled()
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled()
    expect(useWorkTasksStore.getState().tasks[0].scheduleEnabled).toBe(false)

    rejectDaemon(new Error('daemon rejected toggle'))
    expect(await screen.findByRole('alert')).toHaveTextContent('daemon rejected toggle')
    expect(useWorkTasksStore.getState().tasks[0].scheduleEnabled).toBe(false)
    expect(useCoworkStore.getState().scheduledTasks).toEqual([priorSchedule])
    expect(active).not.toBeDisabled()
  })

  it('restores a removed scheduler row when daemon deletion fails', async () => {
    const user = userEvent.setup()
    vi.mocked(hasTauriRuntime).mockReturnValue(true)
    vi.mocked(deleteDurableLocalSchedule).mockRejectedValue(new Error('daemon delete failed'))
    const priorSchedule: ScheduledTask = {
      id: scheduledTask.id,
      name: scheduledTask.title,
      prompt: scheduledTask.prompt,
      cronLike: scheduledTask.scheduleExpr,
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
    useWorkTasksStore.setState({ tasks: [{ ...scheduledTask, scheduleEnabled: true }] })
    useCoworkStore.setState({ scheduledTasks: [priorSchedule] })

    render(
      <MemoryRouter>
        <TasksView />
      </MemoryRouter>,
    )
    await user.click(await screen.findByRole('button', { name: 'Remove' }))

    expect(await screen.findByRole('alert')).toHaveTextContent('daemon delete failed')
    expect(useWorkTasksStore.getState().tasks[0]).toMatchObject({
      scheduleEnabled: true,
      scheduleExpr: scheduledTask.scheduleExpr,
    })
    expect(useCoworkStore.getState().scheduledTasks).toEqual([priorSchedule])
  })
})
