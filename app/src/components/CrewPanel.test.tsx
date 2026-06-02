import { act, fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import CrewPanel from './CrewPanel'
import { safeInvoke } from '../utils/safeInvoke'
import { useConfigStore } from '../stores/configStore'
import { useCrewStore, type CrewAgent } from '../stores/crewStore'
import { usePersonalityStore } from '../stores/personalityStore'

vi.mock('../utils/safeInvoke', () => ({
  safeInvoke: vi.fn(),
}))

const safeInvokeMock = vi.mocked(safeInvoke)

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
    window.localStorage.removeItem('open-cowork-crew')
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

  it('syncs member providers to the crew provider when changing the crew provider', async () => {
    await act(async () => {
      render(<CrewPanel />)
    })

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Provider & Modell/i }))
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
    expect(screen.getByText('Der Crew-Provider gilt fuer alle Mitglieder. Pro Mitglied ist nur noch das Modell ueberschreibbar.')).toBeInTheDocument()
  })

  it('can grant a tool to all crew members from the crew-level access panel', async () => {
    await act(async () => {
      render(<CrewPanel />)
    })

    await act(async () => {
      fireEvent.click(screen.getByLabelText('Task delegieren'))
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
      render(<CrewPanel />)
    })

    await act(async () => {
      fireEvent.click(screen.getByLabelText('workspace-mcp'))
    })

    const crew = useCrewStore.getState().crews[0]
    expect(crew.agents.every((agent) => agent.mcpServerNames.includes('workspace-mcp'))).toBe(true)
  })

  it('adds newly created custom personalities to existing crew members automatically', async () => {
    usePersonalityStore.setState((state) => ({
      ...state,
      personalities: [
        {
          id: 'personality-product-owner',
          name: 'Product Owner',
          description: 'Priorisiert Anforderungen und strukturiert den Arbeitsfokus.',
          role: 'planner',
          goal: 'Priorisiert Anforderungen und strukturiert den Arbeitsfokus.',
          system_prompt: 'Arbeite wie ein Product Owner.',
          skills_markdown: '# Product Owner\n- Priorisierung\n- Anforderungen',
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
      render(<CrewPanel />)
    })

    const crew = useCrewStore.getState().crews[0]
    const syncedAgent = crew.agents.find((agent) => agent.personalityId === 'personality-product-owner')

    expect(crew.agents).toHaveLength(3)
    expect(syncedAgent).toMatchObject({
      id: 'agent-personality-personality-product-owner',
      name: 'Product Owner',
      role: 'planner',
      goal: 'Priorisiert Anforderungen und strukturiert den Arbeitsfokus.',
      backstory: 'Arbeite wie ein Product Owner.',
      skillsMarkdown: '# Product Owner\n- Priorisierung\n- Anforderungen',
      modelOverride: 'qwen3:14b',
    })
  })
})
