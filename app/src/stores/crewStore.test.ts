import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useCrewStore, type CrewAgent } from './crewStore'
import { usePersonalityStore } from './personalityStore'
import { safeInvoke } from '../utils/safeInvoke'

vi.mock('../utils/safeInvoke', () => ({
  safeInvoke: vi.fn(),
}))

const safeInvokeMock = vi.mocked(safeInvoke)

const duplicateAgent: CrewAgent = {
  id: 'agent-researcher',
  name: 'Forscher',
  role: 'researcher',
  goal: 'Recherche',
  backstory: 'Testagent',
  skillsMarkdown: '',
  personalityId: null,
  modelOverride: null,
  providerKind: 'ollama',
  tools: ['read_file'],
  mcpServerNames: [],
  enabled: true,
  allowDelegation: true,
  verbose: true,
  maxIterations: 10,
}

describe('crewStore', () => {
  beforeEach(() => {
    window.localStorage.removeItem('open-cowork-crew')
    safeInvokeMock.mockReset()
    safeInvokeMock.mockImplementation(async (_cmd, _args, fallback) => fallback ?? null)
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
    })
  })

  it('deduplicates agents when creating a crew', () => {
    useCrewStore.setState({
      agents: [duplicateAgent, { ...duplicateAgent }],
    })

    useCrewStore.getState().createCrew('crew-1', 'Test Crew', [])

    expect(useCrewStore.getState().crews).toHaveLength(1)
    expect(useCrewStore.getState().crews[0].agents).toHaveLength(1)
    expect(useCrewStore.getState().crews[0].agents[0].id).toBe('agent-researcher')
  })

  it('syncs global profile fields while preserving crew-specific permissions', () => {
    const localA = { ...duplicateAgent, id: 'agent-a', personalityId: 'pers-shared', tools: ['read_file'], enabled: true }
    const localB = { ...duplicateAgent, id: 'agent-b', personalityId: 'pers-shared', tools: ['edit_file'], enabled: false }
    useCrewStore.setState({
      agents: [localA],
      crews: [
        {
          id: 'crew-a',
          name: 'Crew A',
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
          defaultModel: '',
          providerProfiles: {
            openAICompatible: { enabled: false, baseUrl: '', model: '', apiKey: '', timeoutMs: 600000, verifyTlsCertificates: true },
            openRouter: { enabled: false, baseUrl: 'https://openrouter.ai/api/v1', model: '', apiKey: '', timeoutMs: 600000, verifyTlsCertificates: true },
          },
          agents: [localA],
          tasks: [],
          runtimeConfig: { enabled: false, baseUrl: '', model: '', timeoutMs: 600000 },
          process: 'sequential',
          managerAgentId: null,
          verbose: true,
          maxRpm: 10,
          maxParallelTasks: 3,
          status: 'idle',
          createdAt: 1,
          updatedAt: 1,
        },
        {
          id: 'crew-b',
          name: 'Crew B',
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
          defaultModel: '',
          providerProfiles: {
            openAICompatible: { enabled: false, baseUrl: '', model: '', apiKey: '', timeoutMs: 600000, verifyTlsCertificates: true },
            openRouter: { enabled: false, baseUrl: 'https://openrouter.ai/api/v1', model: '', apiKey: '', timeoutMs: 600000, verifyTlsCertificates: true },
          },
          agents: [localB],
          tasks: [],
          runtimeConfig: { enabled: false, baseUrl: '', model: '', timeoutMs: 600000 },
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
    })

    useCrewStore.getState().syncAgentsFromPersonalityProfiles([{
      id: 'pers-shared',
      name: 'Shared Analyst',
      description: 'Analyse',
      role: 'analyst',
      goal: 'Globale Analyse',
      systemPrompt: 'Globaler Hintergrund',
      skillsMarkdown: '# Skills',
      modelOverride: 'qwen3:14b',
    }])

    const [crewA, crewB] = useCrewStore.getState().crews
    expect(crewA.agents[0]).toMatchObject({ name: 'Shared Analyst', role: 'analyst', goal: 'Globale Analyse', tools: ['read_file'], enabled: true })
    expect(crewB.agents[0]).toMatchObject({ name: 'Shared Analyst', role: 'analyst', goal: 'Globale Analyse', tools: ['edit_file'], enabled: false })
  })

  it('keeps a local snapshot when a used profile is deleted', () => {
    useCrewStore.setState({
      agents: [],
      crews: [{
        id: 'crew-a',
        name: 'Crew A',
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
        defaultModel: '',
        providerProfiles: {
          openAICompatible: { enabled: false, baseUrl: '', model: '', apiKey: '', timeoutMs: 600000, verifyTlsCertificates: true },
          openRouter: { enabled: false, baseUrl: 'https://openrouter.ai/api/v1', model: '', apiKey: '', timeoutMs: 600000, verifyTlsCertificates: true },
        },
        agents: [{ ...duplicateAgent, id: 'agent-a', personalityId: 'pers-delete', tools: ['read_file'] }],
        tasks: [],
        runtimeConfig: { enabled: false, baseUrl: '', model: '', timeoutMs: 600000 },
        process: 'sequential',
        managerAgentId: null,
        verbose: true,
        maxRpm: 10,
        maxParallelTasks: 3,
        status: 'idle',
        createdAt: 1,
        updatedAt: 1,
      }],
    })

    useCrewStore.getState().unlinkPersonalityProfile({
      id: 'pers-delete',
      name: 'Snapshot Agent',
      description: 'Snapshot',
      role: 'writer',
      goal: 'Snapshot Ziel',
      systemPrompt: 'Snapshot Hintergrund',
      skillsMarkdown: '# Snapshot Skills',
      modelOverride: 'llama3',
    })

    const agent = useCrewStore.getState().crews[0].agents[0]
    expect(agent.personalityId).toBeNull()
    expect(agent).toMatchObject({
      name: 'Snapshot Agent',
      role: 'writer',
      goal: 'Snapshot Ziel',
      backstory: 'Snapshot Hintergrund',
      skillsMarkdown: '# Snapshot Skills',
      tools: ['read_file'],
    })
  })

  it('migrates unlinked crew agents to unique global profiles', async () => {
    useCrewStore.setState({
      agents: [],
      crews: [{
        id: 'crew-a',
        name: 'Crew A',
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
        defaultModel: '',
        providerProfiles: {
          openAICompatible: { enabled: false, baseUrl: '', model: '', apiKey: '', timeoutMs: 600000, verifyTlsCertificates: true },
          openRouter: { enabled: false, baseUrl: 'https://openrouter.ai/api/v1', model: '', apiKey: '', timeoutMs: 600000, verifyTlsCertificates: true },
        },
        agents: [
          { ...duplicateAgent, id: 'agent-a', name: 'Analyst', goal: 'Analyse A' },
          { ...duplicateAgent, id: 'agent-b', name: 'Analyst', goal: 'Analyse B' },
        ],
        tasks: [],
        runtimeConfig: { enabled: false, baseUrl: '', model: '', timeoutMs: 600000 },
        process: 'sequential',
        managerAgentId: null,
        verbose: true,
        maxRpm: 10,
        maxParallelTasks: 3,
        status: 'idle',
        createdAt: 1,
        updatedAt: 1,
      }],
    })

    const changed = await useCrewStore.getState().migrateAgentsToPersonalityProfiles([])

    expect(changed).toBe(true)
    expect(safeInvokeMock).toHaveBeenCalledWith('personality_upsert', expect.objectContaining({
      name: 'Analyst',
    }), undefined)
    expect(safeInvokeMock).toHaveBeenCalledWith('personality_upsert', expect.objectContaining({
      name: 'Analyst (2)',
    }), undefined)
    const agents = useCrewStore.getState().crews[0].agents
    expect(agents[0].personalityId).toBeTruthy()
    expect(agents[1].personalityId).toBeTruthy()
    expect(agents[0].personalityId).not.toBe(agents[1].personalityId)
  })

  it('uses current personality profile fields in the CrewAI runtime payload', async () => {
    safeInvokeMock.mockImplementation(async (cmd, _args, fallback) => {
      if (cmd === 'crew_execute') {
        return { crewId: 'crew-a', status: 'completed', taskResults: [], logs: [], error: null }
      }
      return fallback ?? null
    })
    usePersonalityStore.setState({
      personalities: [{
        id: 'pers-runtime',
        name: 'Runtime Profile',
        description: 'Runtime',
        role: 'planner',
        goal: 'Aktueller Profil-Fokus',
        system_prompt: 'Aktueller Hintergrund',
        skills_markdown: '# Runtime Skills',
        temperature: null,
        model_override: 'qwen3:14b',
        icon: null,
        is_default: false,
        created_at: '2026-05-09T00:00:00.000Z',
        updated_at: '2026-05-09T00:00:00.000Z',
      }],
    })
    useCrewStore.setState({
      agents: [],
      crews: [{
        id: 'crew-a',
        name: 'Crew A',
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
        defaultModel: '',
        providerProfiles: {
          openAICompatible: { enabled: false, baseUrl: '', model: '', apiKey: '', timeoutMs: 600000, verifyTlsCertificates: true },
          openRouter: { enabled: false, baseUrl: 'https://openrouter.ai/api/v1', model: '', apiKey: '', timeoutMs: 600000, verifyTlsCertificates: true },
        },
        agents: [{ ...duplicateAgent, id: 'agent-runtime', name: 'Stale', goal: 'Alt', backstory: 'Alt', personalityId: 'pers-runtime' }],
        tasks: [{
          id: 'task-1',
          description: 'Do it',
          expectedOutput: 'Done',
          agentId: 'agent-runtime',
          context: [],
          dependencies: [],
          asyncExecution: false,
          status: 'pending',
          output: null,
        }],
        runtimeConfig: { enabled: false, baseUrl: '', model: '', timeoutMs: 600000 },
        process: 'sequential',
        managerAgentId: null,
        verbose: true,
        maxRpm: 10,
        maxParallelTasks: 3,
        status: 'idle',
        createdAt: 1,
        updatedAt: 1,
      }],
    })

    await useCrewStore.getState().runCrew('crew-a')

    const crewExecuteCall = safeInvokeMock.mock.calls.find(([cmd]) => cmd === 'crew_execute')
    expect(crewExecuteCall?.[1]).toMatchObject({
      request: {
        agents: [{
          id: 'agent-runtime',
          name: 'Runtime Profile',
          role: 'planner',
          goal: 'Aktueller Profil-Fokus',
          backstory: 'Aktueller Hintergrund',
          skillsMarkdown: '# Runtime Skills',
          modelOverride: 'qwen3:14b',
          tools: ['read_file'],
        }],
      },
    })
  })
})
