import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Crew } from '../stores/crewStore'
import { useChatStore } from '../stores/chatStore'
import { useConfigStore } from '../stores/configStore'
import { useCoworkStore } from '../stores/coworkStore'
import { useCrewStore } from '../stores/crewStore'
import { usePersonalityStore } from '../stores/personalityStore'
import { useProjectStore } from '../stores/projectStore'
import { useTaskTemplatesStore } from '../stores/taskTemplatesStore'
import { useUiStore } from '../stores/uiStore'
import { useWorkTasksStore } from '../stores/workTasksStore'
import TasksView from './TasksView'

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

const freeModel = 'nvidia/nemotron-3-super-120b-a12b:free'

const crew = {
  id: 'crew-1',
  name: 'Build crew',
  description: 'Plan, create, and verify a complete working deliverable.',
  defaultProvider: 'openrouter',
  defaultModel: freeModel,
  providerProfiles: {
    openAICompatible: {
      enabled: false,
      baseUrl: 'https://api.openai.com/v1',
      model: '',
      apiKey: '',
      timeoutMs: 600000,
      verifyTlsCertificates: true,
    },
    openRouter: {
      enabled: true,
      baseUrl: 'https://openrouter.ai/api/v1',
      model: freeModel,
      apiKey: 'or-test',
      timeoutMs: 600000,
      verifyTlsCertificates: true,
    },
  },
  tasks: [
    {
      id: 'plan',
      description: 'Analyze the objective and create a plan.',
      expectedOutput: 'A concise plan.',
      agentId: 'planner',
      context: [],
      dependencies: [],
      asyncExecution: false,
      status: 'pending',
      output: null,
    },
    {
      id: 'execute',
      description: 'Produce the requested deliverable.',
      expectedOutput: 'A complete deliverable.',
      agentId: 'executor',
      context: ['plan'],
      dependencies: ['plan'],
      asyncExecution: false,
      status: 'pending',
      output: null,
    },
    {
      id: 'review',
      description: 'Review and finalize the result.',
      expectedOutput: 'A reviewed, user-ready deliverable.',
      agentId: 'reviewer',
      context: ['execute'],
      dependencies: ['execute'],
      asyncExecution: false,
      status: 'pending',
      output: null,
    },
  ],
  updatedAt: 200,
} as Crew

describe('TasksView crew mission flow', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    window.localStorage.clear()

    useWorkTasksStore.setState({ tasks: [] })
    useCrewStore.setState({
      crews: [crew],
      agents: [],
      executionLogs: [],
      activeCrewId: crew.id,
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
    useProjectStore.setState({ projects: [], activeProjectId: null })
    useUiStore.setState({ activeMode: 'work', workingFolder: null, workingPathKind: null })
    useCoworkStore.setState({
      scheduledTasks: [],
      scheduledRuns: [],
      loadScheduledTasks: vi.fn(async () => undefined),
      loadScheduledRuns: vi.fn(async () => undefined),
    })

    const config = useConfigStore.getState()
    useConfigStore.setState({
      ollama: { ...config.ollama, model: 'llama3.1:8b' },
      llmProfiles: [{
        id: 'openrouter-free',
        name: 'OpenRouter Free',
        provider: 'openrouter',
        baseUrl: 'https://openrouter.ai/api/v1',
        model: freeModel,
        apiKey: 'or-test',
        timeoutMs: 600000,
        verifyTlsCertificates: true,
        contextWindow: null,
        temperature: null,
      }],
      defaultLlmProfileIds: {
        ollama: 'default-ollama',
        'openai-compatible': 'default-openai-compatible',
        openrouter: 'openrouter-free',
      },
    })
  })

  it('prefills the task composer when Crew hands off a mission', async () => {
    const user = userEvent.setup()
    render(
      <MemoryRouter initialEntries={[`/tasks?crew=${crew.id}`]}>
        <TasksView />
      </MemoryRouter>,
    )

    expect(await screen.findByDisplayValue('Build crew · Mission')).toBeInTheDocument()
    expect(screen.getByDisplayValue(crew.description)).toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: /Advanced setup/i }))
    expect(screen.getByDisplayValue('A reviewed, user-ready deliverable.')).toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: 'Create task' }))
    await waitFor(() => {
      expect(useWorkTasksStore.getState().tasks[0]?.id).toBe('crew-mission-crew-1')
    })
    expect(screen.getByRole('button', { name: 'Mission created' })).toBeDisabled()
  })

  it('creates one mission and gives its chat the crew OpenRouter free model', async () => {
    const user = userEvent.setup()
    render(
      <MemoryRouter>
        <TasksView />
      </MemoryRouter>,
    )

    await user.click(await screen.findByRole('button', { name: 'Create crew mission' }))

    await waitFor(() => {
      expect(useWorkTasksStore.getState().tasks).toHaveLength(1)
      expect(useChatStore.getState().threads).toHaveLength(1)
    })

    const task = useWorkTasksStore.getState().tasks[0]
    const thread = useChatStore.getState().threads[0]
    expect(task.id).toBe('crew-mission-crew-1')
    expect(task.prompt).toBe(crew.description)
    expect(task.threadId).toBe(thread.id)
    expect(thread.providerSettings).toEqual({
      provider: 'openrouter',
      model: freeModel,
      profileId: 'openrouter-free',
    })
    expect(screen.getByRole('button', { name: 'Mission created' })).toBeDisabled()
  })

  it('selects a task from a chat deep link', async () => {
    const baseTask = {
      prompt: 'Complete the selected mission.',
      expectedOutput: 'A verified result.',
      workDir: '',
      threadId: null,
      runner: 'model' as const,
      crewId: null,
      model: freeModel,
      scheduleExpr: '',
      scheduleEnabled: false,
      status: 'idle' as const,
      output: null,
      error: null,
      lastRunAt: null,
      createdAt: 100,
      updatedAt: 100,
    }
    useWorkTasksStore.setState({
      tasks: [
        { ...baseTask, id: 'task-first', title: 'First mission' },
        { ...baseTask, id: 'task-linked', title: 'Linked mission', createdAt: 200, updatedAt: 200 },
      ],
    })

    render(
      <MemoryRouter initialEntries={['/tasks?task=task-linked']}>
        <TasksView />
      </MemoryRouter>,
    )

    const detail = await screen.findByRole('region', { name: 'Task detail' })
    await waitFor(() => {
      expect(within(detail).getByLabelText('Title')).toHaveValue('Linked mission')
    })
  })

  it('falls back to the first task when a deep link is stale', async () => {
    useWorkTasksStore.setState({
      tasks: [{
        id: 'task-current',
        title: 'Current mission',
        prompt: 'Continue the current mission.',
        expectedOutput: 'A verified result.',
        workDir: '',
        threadId: null,
        runner: 'model',
        crewId: null,
        model: freeModel,
        scheduleExpr: '',
        scheduleEnabled: false,
        status: 'idle',
        output: null,
        error: null,
        lastRunAt: null,
        createdAt: 100,
        updatedAt: 100,
      }],
    })

    render(
      <MemoryRouter initialEntries={['/tasks?task=deleted-task']}>
        <TasksView />
      </MemoryRouter>,
    )

    const detail = await screen.findByRole('region', { name: 'Task detail' })
    await waitFor(() => {
      expect(within(detail).getByLabelText('Title')).toHaveValue('Current mission')
    })
  })
})
