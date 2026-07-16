import { describe, expect, it } from 'vitest'
import type { Crew } from '../../stores/crewStore'
import type { WorkTask } from '../../stores/workTasksStore'
import {
  augmentCrewToolsForTask,
  buildCrewRuntimeTasks,
  isCodingTask,
  isPresentationTask,
  isResearchTask,
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
