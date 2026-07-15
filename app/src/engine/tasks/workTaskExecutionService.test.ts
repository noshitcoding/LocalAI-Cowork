import { describe, expect, it } from 'vitest'
import type { Crew } from '../../stores/crewStore'
import {
  buildCrewMissionDraft,
  buildCrewMissionId,
  buildCrewMissionTask,
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
})
