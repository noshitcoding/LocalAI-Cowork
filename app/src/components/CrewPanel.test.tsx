import { act, fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import CrewPanel from './CrewPanel'
import { safeInvoke } from '../utils/safeInvoke'
import { useConfigStore } from '../stores/configStore'
import { useCrewStore, type CrewAgent } from '../stores/crewStore'

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
          executionGuidelines: '',
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
            },
            openRouter: {
              enabled: false,
              baseUrl: 'https://openrouter.ai/api/v1',
              model: '',
              apiKey: '',
              timeoutMs: 600000,
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
  })

  it('preserves custom member overrides when changing the crew provider', async () => {
    await act(async () => {
      render(<CrewPanel />)
    })

    await act(async () => {
      fireEvent.change(screen.getByLabelText(/Crew-Provider/), { target: { value: 'openai-compatible' } })
    })

    const crew = useCrewStore.getState().crews[0]
    const defaultAgent = crew.agents.find((agent) => agent.id === 'agent-default')
    const customAgent = crew.agents.find((agent) => agent.id === 'agent-custom')

    expect(crew.defaultProvider).toBe('openai-compatible')
    expect(crew.defaultModel).toBe('')
    expect(defaultAgent?.providerKind).toBe('openai-compatible')
    expect(customAgent?.providerKind).toBe('ollama')
    expect(customAgent?.modelOverride).toBe('llama3.1:70b')
    expect(screen.getByText('Mitglieder mit eigenem Provider oder Modell bleiben beim Umschalten unveraendert.')).toBeInTheDocument()
  })
})