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
      toggleScheduledTask: vi.fn(async () => undefined),
      removeScheduledTask: vi.fn(async () => undefined),
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

    await user.click(screen.getByRole('checkbox'))

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
})
