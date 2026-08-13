import { describe, expect, it } from 'vitest'
import type { Crew } from '../../stores/crewStore'
import type { WorkTask } from '../../stores/workTasksStore'
import {
  augmentCrewToolsForTask,
  buildWorkTaskCrewGuidelines,
  buildCrewRuntimeTasks,
  isCodingTask,
  isPresentationTask,
  isResearchTask,
  resolveWorkTaskResponseLanguage,
  resolveEffectiveCrewProvider,
  resolveExternalProviderConfig,
} from './workTaskCrewRuntime'

function createWorkTask(patch: Partial<WorkTask> = {}): WorkTask {
  const now = Date.now()
  return {
    id: 'work-task',
    title: 'Task',
    prompt: 'Complete the task.',
    expectedOutput: '',
    workDir: '',
    threadId: 'thread',
    runner: 'crew',
    crewId: 'crew',
    model: '',
    scheduleExpr: '',
    scheduleEnabled: false,
    status: 'idle',
    output: null,
    error: null,
    lastRunAt: null,
    createdAt: now,
    updatedAt: now,
    ...patch,
  }
}

describe('task-specific CrewAI tools', () => {
  it('adds discovery and fetch tools to research tasks', () => {
    const task = createWorkTask({ prompt: 'Recherchiere aktuelle Quellen zum Thema.' })

    expect(isResearchTask(task)).toBe(true)
    expect(augmentCrewToolsForTask(['read_file'], task)).toEqual([
      'read_file',
      'web_search',
      'web_fetch',
    ])
  })

  it('makes the live runtime clock authoritative for fresh news', () => {
    const task = createWorkTask({
      prompt: 'Create a verified Daily News Report for the last 24 hours.',
    })
    const crew = {
      executionGuidelines: 'Verify every source.',
    } as Crew

    const guidelines = buildWorkTaskCrewGuidelines(crew, task)

    expect(guidelines).toContain('Authoritative runtime clock')
    expect(guidelines).toContain('never "the future"')
    expect(guidelines).toContain('Do not stop after one poor search')
    expect(guidelines).toContain('newer than model training is never')
  })

  it('uses the prompt language ahead of fixed Crew language defaults', () => {
    const englishTask = createWorkTask({
      title: 'Daily briefing',
      prompt: 'Create a verified report with current sources.',
    })
    const germanTask = createWorkTask({
      title: 'Tagesbericht',
      prompt: 'Erstelle bitte einen Bericht mit aktuellen Quellen.',
    })
    const ambiguousTask = createWorkTask({
      title: 'Q3',
      prompt: '2026',
    })

    expect(resolveWorkTaskResponseLanguage(englishTask, 'de')).toBe('English')
    expect(resolveWorkTaskResponseLanguage(germanTask, 'en')).toBe('German')
    expect(resolveWorkTaskResponseLanguage(ambiguousTask, 'de')).toBe('German')

    const guidelines = buildWorkTaskCrewGuidelines({
      executionGuidelines: 'Final answer language: German.',
    } as Crew, englishTask)
    expect(guidelines).toContain('Required final-output language: English')
    expect(guidelines).toContain('overrides fixed language defaults')
  })

  it('adds file editing and verification tools to coding tasks', () => {
    const task = createWorkTask({ prompt: 'Fixe den Bug im TypeScript-Code und führe Tests aus.' })
    const tools = augmentCrewToolsForTask([], task)

    expect(isCodingTask(task)).toBe(true)
    expect(tools).toEqual(expect.arrayContaining([
      'read_file',
      'glob',
      'grep',
      'edit_file',
      'create_directory',
      'bash',
    ]))
  })

  it('adds a real Office artifact tool to PPT and PPP tasks', () => {
    const pptTask = createWorkTask({ expectedOutput: 'Eine fertige PPTX-Präsentation.' })
    const pppTask = createWorkTask({ prompt: 'Erstelle die PPP Aufgabe als Folien.' })

    expect(isPresentationTask(pptTask)).toBe(true)
    expect(isPresentationTask(pppTask)).toBe(true)
    expect(augmentCrewToolsForTask([], pptTask)).toContain('office_workflow')
  })
})

describe('parallel CrewAI task compatibility', () => {
  it('keeps the final task synchronous so CrewAI accepts the crew', () => {
    const task = createWorkTask()
    const crew = {
      process: 'parallel',
      managerAgentId: null,
      agents: [{ id: 'agent', enabled: true }],
      tasks: [
        { id: 'one', description: 'One', expectedOutput: 'One', agentId: 'agent', context: [], dependencies: [], asyncExecution: false },
        { id: 'two', description: 'Two', expectedOutput: 'Two', agentId: 'agent', context: [], dependencies: [], asyncExecution: false },
        { id: 'three', description: 'Three', expectedOutput: 'Three', agentId: 'agent', context: [], dependencies: [], asyncExecution: false },
      ],
    } as unknown as Crew

    const runtimeTasks = buildCrewRuntimeTasks(crew, task, new Set(['agent']))

    expect(runtimeTasks.map((entry) => entry.asyncExecution)).toEqual([true, true, false])
  })
})

describe('crew provider resolution', () => {
  it('uses the current Settings profile ahead of a stale enabled crew copy', () => {
    const resolved = resolveExternalProviderConfig(
      {
        enabled: true,
        baseUrl: 'https://old-crew-endpoint.example.test/v1',
        model: 'old/crew-model',
        apiKey: 'old-crew-key',
        timeoutMs: 600000,
        verifyTlsCertificates: true,
      },
      {
        baseUrl: 'https://current-settings-endpoint.example.test/v1',
        model: 'current/settings-model',
        apiKey: 'current-settings-key',
        timeoutMs: 120000,
        verifyTlsCertificates: false,
      },
      'https://api.openai.com/v1',
    )

    expect(resolved).toEqual({
      baseUrl: 'https://current-settings-endpoint.example.test/v1',
      model: 'current/settings-model',
      models: [],
      apiKey: 'current-settings-key',
      timeoutMs: 120000,
      verifyTlsCertificates: false,
    })
  })

  it('uses the configured global profile when a legacy crew profile is disabled', () => {
    const resolved = resolveExternalProviderConfig(
      {
        enabled: false,
        baseUrl: '',
        model: '',
        apiKey: '',
        timeoutMs: 600000,
      },
      {
        baseUrl: 'https://inference.example.test/v1',
        model: 'example/model',
        apiKey: 'test-key',
        timeoutMs: 120000,
        verifyTlsCertificates: false,
      },
      'https://api.openai.com/v1',
    )

    expect(resolved).toEqual({
      baseUrl: 'https://inference.example.test/v1',
      model: 'example/model',
      models: [],
      apiKey: 'test-key',
      timeoutMs: 120000,
      verifyTlsCertificates: false,
    })
  })

  it('preserves disabled TLS verification for a trusted custom endpoint', () => {
    const resolved = resolveExternalProviderConfig(
      {
        enabled: false,
        baseUrl: '',
        model: '',
        apiKey: '',
        timeoutMs: 600000,
        verifyTlsCertificates: true,
      },
      {
        baseUrl: 'https://internal-inference.example.test',
        model: 'example/model',
        apiKey: 'test-key',
        timeoutMs: 120000,
        verifyTlsCertificates: false,
      },
      'https://api.openai.com/v1',
    )

    expect(resolved?.verifyTlsCertificates).toBe(false)
  })

  it('uses the loaded Settings model id when the configured alias is stale', () => {
    const resolved = resolveExternalProviderConfig(
      undefined,
      {
        baseUrl: 'https://inference.example.test/v1',
        model: 'google/gemma-4-31B-it',
        apiKey: 'test-key',
        timeoutMs: 120000,
        verifyTlsCertificates: false,
      },
      'https://api.openai.com/v1',
      ['RedHatAI/gemma-4-31B-it-FP8-block'],
    )

    expect(resolved?.model).toBe('RedHatAI/gemma-4-31B-it-FP8-block')
    expect(resolved?.models).toEqual(['RedHatAI/gemma-4-31B-it-FP8-block'])
  })

  it('routes a legacy OpenAI-compatible crew through a configured OpenRouter profile', () => {
    const provider = resolveEffectiveCrewProvider(
      'openai-compatible',
      { model: 'gemma4:31b' },
      {
        openAICompatible: undefined,
        openRouter: {
          baseUrl: 'https://openrouter.ai/api/v1',
          model: 'google/gemma-4-31b-it:free',
          models: [],
          apiKey: 'test-key',
          timeoutMs: 600000,
          verifyTlsCertificates: true,
        },
      },
    )

    expect(provider).toBe('openrouter')
  })

  it('keeps the requested provider when it has a configured model', () => {
    const provider = resolveEffectiveCrewProvider(
      'openai-compatible',
      { model: 'gemma4:31b' },
      {
        openAICompatible: {
          baseUrl: 'https://inference.example.test/v1',
          model: 'example/model',
          models: [],
          apiKey: 'test-key',
          timeoutMs: 600000,
          verifyTlsCertificates: true,
        },
        openRouter: {
          baseUrl: 'https://openrouter.ai/api/v1',
          model: 'google/gemma-4-31b-it:free',
          models: [],
          apiKey: 'test-key',
          timeoutMs: 600000,
          verifyTlsCertificates: true,
        },
      },
    )

    expect(provider).toBe('openai-compatible')
  })
})
