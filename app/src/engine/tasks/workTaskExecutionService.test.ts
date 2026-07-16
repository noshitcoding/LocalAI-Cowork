import { describe, expect, it } from 'vitest'
import type { Crew } from '../../stores/crewStore'
import {
  buildCrewMissionDraft,
  buildCrewMissionId,
  buildCrewMissionTask,
  resolveWorkTaskChatProviderSettings,
} from './workTaskExecutionService'

const crew = {
  id: 'crew-build',
  name: 'Build Crew',
  description: 'Plan, build, and verify the requested outcome.',
  tasks: [
    {
      id: 'plan',
      description: 'Create the plan.',
      expectedOutput: 'An approved plan.',
    },
    {
      id: 'review',
      description: 'Review the result.',
      expectedOutput: 'A reviewed, user-ready deliverable.',
    },
  ],
} as Crew

describe('crew mission handoff', () => {
  it('turns a multi-step crew into one mission draft', () => {
    expect(buildCrewMissionDraft(crew)).toEqual({
      title: 'Build Crew · Mission',
      prompt: crew.description,
      expectedOutput: 'A reviewed, user-ready deliverable.',
      workDir: '',
      runner: 'crew',
      crewId: crew.id,
      model: '',
    })
  })

  it('uses one stable WorkTask id instead of exposing internal crew steps as tasks', () => {
    const task = buildCrewMissionTask(crew, 42)

    expect(task.id).toBe(buildCrewMissionId(crew.id))
    expect(task.id).toBe('crew-mission-crew-build')
    expect(task.status).toBe('idle')
    expect(task.createdAt).toBe(42)
    expect(task.threadId).toBeNull()
  })

  it('restores a crew task chat with the crew free OpenRouter model', () => {
    const freeModel = 'nvidia/nemotron-3-super-120b-a12b:free'
    const task = buildCrewMissionTask(crew, 42)

    expect(resolveWorkTaskChatProviderSettings(task, {
      crews: [{
        id: crew.id,
        defaultProvider: 'openrouter',
        defaultModel: freeModel,
      }],
      ollamaModel: 'llama3.1:8b',
      defaultLlmProfileIds: {
        ollama: 'default-ollama',
        'openai-compatible': 'default-openai-compatible',
        openrouter: 'openrouter-free',
      },
      llmProfiles: [{
        id: 'openrouter-free',
        name: 'OpenRouter Free',
        provider: 'openrouter',
        baseUrl: 'https://openrouter.ai/api/v1',
        model: freeModel,
        apiKey: '',
        timeoutMs: 600000,
        verifyTlsCertificates: true,
        contextWindow: null,
        temperature: null,
      }],
      fallbackProviderSettings: { provider: 'ollama', model: 'llama3.1:8b' },
    })).toEqual({
      provider: 'openrouter',
      model: freeModel,
      profileId: 'openrouter-free',
    })
  })
})
