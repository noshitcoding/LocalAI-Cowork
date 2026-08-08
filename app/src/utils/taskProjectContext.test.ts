import { beforeEach, describe, expect, it } from 'vitest'
import { useProjectStore } from '../stores/projectStore'
import { useWorkTasksStore } from '../stores/workTasksStore'
import {
  appendTaskProjectPrompt,
  resolveTaskProjectRunContext,
} from './taskProjectContext'

describe('task project run context', () => {
  beforeEach(() => {
    const now = Date.now()
    useWorkTasksStore.setState({
      tasks: [{
        id: 'task-1',
        title: 'Project task',
        prompt: 'Use the project',
        expectedOutput: '',
        workDir: 'C:\\task-workspace',
        threadId: 'thread-1',
        runner: 'model',
        crewId: null,
        model: '',
        scheduleExpr: '',
        scheduleEnabled: false,
        status: 'idle',
        output: null,
        error: null,
        lastRunAt: null,
        createdAt: now,
        updatedAt: now,
      }],
    })
    useProjectStore.setState({
      activeProjectId: 'project-1',
      projects: [{
        id: 'project-1',
        title: 'Current project',
        instructions: 'Use current instructions.',
        resources: [
          {
            id: 'folder-1',
            path: 'C:\\project-root',
            kind: 'folder',
            enabled: true,
            access: 'read_write',
            isPrimary: true,
            addedAt: now,
          },
          {
            id: 'file-1',
            path: 'C:\\project-root\\single.txt',
            kind: 'file',
            enabled: true,
            access: 'read_write',
            isPrimary: false,
            addedAt: now,
          },
          {
            id: 'file-disabled',
            path: 'C:\\private\\disabled.txt',
            kind: 'file',
            enabled: false,
            access: 'read_write',
            isPrimary: false,
            addedAt: now,
          },
        ],
        threadIds: ['thread-1'],
        createdAt: now,
        updatedAt: now,
      }],
    })
  })

  it('derives the project from the task chat and prioritizes the task cwd', async () => {
    const context = await resolveTaskProjectRunContext({
      taskId: 'task-1',
      prompt: 'Run now',
      workDir: 'C:\\task-workspace',
    })

    expect(context.projectId).toBe('project-1')
    expect(context.projectTitle).toBe('Current project')
    expect(context.promptContext).toContain('Use current instructions.')
    expect(context.preferredCwd).toBe('C:\\task-workspace')
    expect(context.authorizedPaths).toEqual([
      { path: 'C:\\task-workspace', kind: 'folder', access: 'read_write', isPrimary: true },
      { path: 'C:\\project-root', kind: 'folder', access: 'read_write', id: 'folder-1', label: undefined, isPrimary: true },
      { path: 'C:\\project-root\\single.txt', kind: 'file', access: 'read_write', id: 'file-1', label: undefined, isPrimary: false },
    ])
    expect(context.authorizedPaths.some((entry) => entry.path.includes('disabled'))).toBe(false)
  })

  it('keeps resolver warnings in the final run prompt', () => {
    const prompt = appendTaskProjectPrompt('Do the work', {
      projectId: 'project-1',
      projectTitle: 'Current project',
      promptContext: 'Project instructions',
      authorizedPaths: [],
      preferredCwd: null,
      warnings: ['Missing source'],
    })

    expect(prompt).toContain('Do the work')
    expect(prompt).toContain('Project instructions')
    expect(prompt).toContain('- Missing source')
  })
})
