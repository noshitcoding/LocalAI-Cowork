import { act, fireEvent, render, screen } from '@testing-library/react'
import { MemoryRouter, useLocation } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import CrewPanel from './CrewPanel'
import { hasTauriRuntime, safeInvoke } from '../utils/safeInvoke'
import { useConfigStore } from '../stores/configStore'
import { useCrewStore, type CrewAgent } from '../stores/crewStore'
import { usePersonalityStore } from '../stores/personalityStore'

vi.mock('../utils/safeInvoke', () => ({
  hasTauriRuntime: vi.fn(() => false),
  safeInvoke: vi.fn(),
}))

const hasTauriRuntimeMock = vi.mocked(hasTauriRuntime)
const safeInvokeMock = vi.mocked(safeInvoke)

function LocationProbe() {
  const location = useLocation()
  return <output data-testid="location">{`${location.pathname}${location.search}`}</output>
}

function renderCrewPanel() {
  return render(
    <MemoryRouter>
      <CrewPanel />
      <LocationProbe />
    </MemoryRouter>,
  )
}

const baseAgent: CrewAgent = {
  id: 'agent-1',
  name: 'Agent 1',
  role: 'researcher',
  goal: 'Analyse',
  backstory: 'Testagent',
  skillsMarkdown: '',
  personalityId: null,
  modelOverride: null,
  providerKind: 'ollama',
  tools: [],
  mcpServerNames: [],
  enabled: true,
  allowDelegation: true,
  verbose: false,
  maxIterations: 5,
}

describe('CrewPanel', () => {
  beforeEach(() => {
    Object.defineProperty(HTMLElement.prototype, 'scrollIntoView', {
      configurable: true,
      value: vi.fn(),
    })
    window.localStorage.removeItem('open-cowork-crew')
    hasTauriRuntimeMock.mockReset()
    hasTauriRuntimeMock.mockReturnValue(false)
    safeInvokeMock.mockReset()
    safeInvokeMock.mockResolvedValue({
      endpoint: 'https://api.openai.com/v1',
      models: ['gpt-4.1-mini'],
    })

    useConfigStore.setState({
      availableModels: ['llama3.2:latest', 'llama3.1:70b', 'qwen3:14b'],
      ollama: {
        baseUrl: 'http://localhost:11434',
        model: 'llama3.2:latest',
        timeoutMs: 200000,
        contextWindow: 128000,
        temperature: 0.1,
      },
      llmProfiles: [
        {
          id: 'openai-default',
          name: 'OpenAI kompatibel',
          provider: 'openai-compatible',
          baseUrl: 'https://api.openai.com/v1',
          model: 'gpt-4.1-mini',
          apiKey: 'sk-test',
          timeoutMs: 600000,
          verifyTlsCertificates: true,
          contextWindow: null,
          temperature: null,
        },
        {
          id: 'openrouter-default',
          name: 'OpenRouter',
          provider: 'openrouter',
          baseUrl: 'https://openrouter.ai/api/v1',
          model: '',
          apiKey: 'or-test',
          timeoutMs: 600000,
          verifyTlsCertificates: true,
          contextWindow: null,
          temperature: null,
        },
      ],
      defaultLlmProfileIds: {
        ollama: '',
        'openai-compatible': 'openai-default',
        openrouter: 'openrouter-default',
      },
      mcpServer: { name: '', command: '', args: '', env: {} },
      mcpServers: [],
    })

    useCrewStore.setState({
      crews: [
        {
          id: 'crew-1',
          name: 'Test Crew',
          description: '',
          executionSubject: 'workspace-user',
          executionGuidelines: '',
          knowledgeFocus: '',
          governanceMode: 'allow-all',
          outputMode: 'standard',
          stopOnFailure: false,
          retryCount: 0,
          managerReviewEnabled: true,
          managerReviewGuidelines: '',
          shareAllTaskOutputs: true,
          sharedOutputCharLimit: 0,
          defaultProvider: 'ollama',
          defaultModel: 'llama3.2:latest',
          providerProfiles: {
            openAICompatible: {
              enabled: true,
              baseUrl: 'https://api.openai.com/v1',
              model: 'gpt-4.1-mini',
              apiKey: 'sk-test',
              timeoutMs: 600000,
              verifyTlsCertificates: true,
            },
            openRouter: {
              enabled: false,
              baseUrl: 'https://openrouter.ai/api/v1',
              model: '',
              apiKey: '',
              timeoutMs: 600000,
              verifyTlsCertificates: true,
            },
          },
          agents: [
            { ...baseAgent, id: 'agent-default', name: 'Default Agent' },
            { ...baseAgent, id: 'agent-custom', name: 'Custom Agent', modelOverride: 'llama3.1:70b' },
          ],
          tasks: [],
          runtimeConfig: {
            enabled: false,
            baseUrl: '',
            model: '',
            timeoutMs: 600000,
          },
          process: 'sequential',
          managerAgentId: null,
          verbose: true,
          maxRpm: 10,
          maxParallelTasks: 3,
          status: 'idle',
          createdAt: 1,
          updatedAt: 1,
        },
      ],
      agents: [],
      executionLogs: [],
      activeCrewId: 'crew-1',
      loading: false,
    })

    usePersonalityStore.setState({
      personalities: [],
      activeId: null,
      loading: false,
      error: null,
      loadPersonalities: vi.fn().mockResolvedValue(undefined),
      upsertPersonality: vi.fn().mockResolvedValue(undefined),
      deletePersonality: vi.fn().mockResolvedValue(undefined),
      setActive: vi.fn(),
    })
  })

  it('does not run desktop personality migration in the browser preview', async () => {
    const migrateAgentsToPersonalityProfiles = vi.fn().mockResolvedValue(true)
    useCrewStore.setState({ migrateAgentsToPersonalityProfiles })

    await act(async () => {
      renderCrewPanel()
    })

    expect(migrateAgentsToPersonalityProfiles).not.toHaveBeenCalled()
    expect(usePersonalityStore.getState().loadPersonalities).toHaveBeenCalledTimes(1)
  })

  it('creates a complete starter workflow from an empty-state preset', async () => {
    useCrewStore.setState({
      crews: [],
      activeCrewId: null,
      agents: [
        { ...baseAgent, id: 'planner', name: 'Planner', role: 'planner' },
        { ...baseAgent, id: 'executor', name: 'Executor', role: 'executor' },
        { ...baseAgent, id: 'reviewer', name: 'Reviewer', role: 'reviewer' },
      ],
    })

    await act(async () => {
      renderCrewPanel()
    })

    fireEvent.click(screen.getByRole('button', { name: /Build crew/i }))

    const created = useCrewStore.getState().crews[0]
    expect(created.name).toBe('Build crew')
    expect(created.description).toContain('complete working deliverable')
    expect(created.tasks).toHaveLength(3)
    expect(created.agents.map((agent) => agent.role)).toEqual(['planner', 'executor', 'reviewer'])
  })

  it('hands the active crew to the task mission composer', async () => {
    renderCrewPanel()

    fireEvent.click(screen.getByRole('button', { name: 'Prepare mission in Tasks' }))

    expect(screen.getByTestId('location')).toHaveTextContent('/tasks?crew=crew-1')
  })

  it('syncs member providers to the crew provider when changing the crew provider', async () => {
    await act(async () => {
      renderCrewPanel()
    })

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Provider & Model/i }))
    })

    await act(async () => {
      fireEvent.change(screen.getByRole('combobox', { name: 'Crew-Provider' }), { target: { value: 'openai-compatible' } })
    })

    const crew = useCrewStore.getState().crews[0]
    const defaultAgent = crew.agents.find((agent) => agent.id === 'agent-default')
    const customAgent = crew.agents.find((agent) => agent.id === 'agent-custom')

    expect(crew.defaultProvider).toBe('openai-compatible')
    expect(crew.defaultModel).toBe('')
    expect(defaultAgent?.providerKind).toBe('openai-compatible')
    expect(customAgent?.providerKind).toBe('openai-compatible')
    expect(screen.getByText('The crew provider applies to all members. Only the model can still be overridden per member.')).toBeInTheDocument()
  })

  it('can grant a tool to all crew members from the crew-level access panel', async () => {
    await act(async () => {
      renderCrewPanel()
    })

    await act(async () => {
      fireEvent.click(screen.getByLabelText('Delegate task'))
    })

    const crew = useCrewStore.getState().crews[0]
    expect(crew.agents.every((agent) => agent.tools.includes('delegate_task'))).toBe(true)
    expect(crew.agents.every((agent) => agent.allowDelegation)).toBe(true)
  })

  it('can grant an MCP server to all crew members from the crew-level access panel', async () => {
    useConfigStore.setState({
      mcpServers: [
        { name: 'workspace-mcp', command: 'node', args: 'server.js', env: {} },
      ],
    })

    await act(async () => {
      renderCrewPanel()
    })

    await act(async () => {
      fireEvent.click(screen.getByLabelText('workspace-mcp'))
    })

    const crew = useCrewStore.getState().crews[0]
    expect(crew.agents.every((agent) => agent.mcpServerNames.includes('workspace-mcp'))).toBe(true)
  })

  it('starts a configured crew directly from the crew workspace', async () => {
    useCrewStore.setState((state) => ({
      crews: state.crews.map((crew) => ({
        ...crew,
        tasks: [{
          id: 'task-run',
          description: 'Run the configured crew',
          expectedOutput: 'Verified result',
          agentId: 'agent-default',
          context: [],
          dependencies: [],
          asyncExecution: false,
          status: 'pending' as const,
          output: null,
        }],
      })),
    }))
    safeInvokeMock.mockImplementation(async (command) => {
      if (command === 'crew_execute') {
        return {
          crewId: 'crew-1',
          status: 'completed',
          taskResults: [{ taskId: 'task-run', agentId: 'agent-default', status: 'completed', output: 'Done' }],
          logs: [],
          error: null,
        }
      }
      return { endpoint: 'https://api.openai.com/v1', models: ['gpt-4.1-mini'] }
    })

    await act(async () => {
      renderCrewPanel()
    })
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Run crew' }))
    })

    expect(safeInvokeMock).toHaveBeenCalledWith('crew_execute', expect.objectContaining({
      request: expect.objectContaining({ id: 'crew-1' }),
    }))
    expect(useCrewStore.getState().crews[0].status).toBe('completed')
  })

  it('adds newly created custom personalities to existing crew members automatically', async () => {
    usePersonalityStore.setState((state) => ({
      ...state,
      personalities: [
        {
          id: 'personality-product-owner',
          name: 'Product Owner',
          description: 'Prioritizes requirements and structures the work focus.',
          role: 'planner',
          goal: 'Prioritizes requirements and structures the work focus.',
          system_prompt: 'Work like a Product Owner.',
          skills_markdown: '# Product Owner\n- Prioritization\n- Requirements',
          temperature: null,
          model_override: 'qwen3:14b',
          icon: 'PO',
          is_default: false,
          created_at: '2026-05-04T00:00:00.000Z',
          updated_at: '2026-05-04T00:00:00.000Z',
        },
      ],
    }))

    await act(async () => {
      renderCrewPanel()
    })

    const crew = useCrewStore.getState().crews[0]
    const syncedAgent = crew.agents.find((agent) => agent.personalityId === 'personality-product-owner')

    expect(crew.agents).toHaveLength(3)
    expect(syncedAgent).toMatchObject({
      id: 'agent-personality-personality-product-owner',
      name: 'Product Owner',
      role: 'planner',
      goal: 'Prioritizes requirements and structures the work focus.',
      backstory: 'Work like a Product Owner.',
      skillsMarkdown: '# Product Owner\n- Prioritization\n- Requirements',
      modelOverride: 'qwen3:14b',
    })
  })
})
