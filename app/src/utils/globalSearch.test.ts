import { describe, expect, it } from 'vitest'
import type { ChatThread } from '../stores/chatStore'
import type { Project } from '../stores/projectStore'
import type { WorkTask } from '../stores/workTasksStore'
import { buildSearchIndex, filterSearchIndex } from './globalSearch'

const thread: ChatThread = {
  id: 'thread-1',
  title: 'Release Plan',
  messages: [
    { id: 'm-1', role: 'system', content: 'Internal setup', timestamp: 1 },
    { id: 'm-2', role: 'user', content: 'Please plane die Migration for den Connector.', timestamp: 2 },
  ],
  createdAt: 1,
  updatedAt: 4,
}

const task: WorkTask = {
  id: 'task-1',
  title: 'Connector Registry',
  prompt: 'Baue eine Registry mit Status und Permissions.',
  expectedOutput: 'Roadmap und Tests',
  workDir: 'C:/workspace',
  threadId: 'thread-1',
  runner: 'model',
  crewId: null,
  model: 'gpt-test',
  scheduleExpr: '',
  scheduleEnabled: false,
  status: 'idle',
  output: null,
  error: null,
  lastRunAt: null,
  createdAt: 1,
  updatedAt: 3,
}

const project: Project = {
  id: 'project-1',
  title: 'Desktop App',
  instructions: 'Arbeite mit Approvals und Audit.',
  resources: [
    {
      id: 'resource-1',
      path: 'docs/DESKTOP_SMOKE_TEST.md',
      kind: 'file',
      label: 'Smoke Test',
      enabled: true,
      addedAt: 2,
    },
  ],
  threadIds: ['thread-1'],
  createdAt: 1,
  updatedAt: 5,
}

describe('globalSearch', () => {
  it('indexes workspace objects and static destinations', () => {
    const entries = buildSearchIndex({
      threads: [thread],
      tasks: [task],
      projects: [project],
    })

    expect(entries.some((entry) => entry.id === 'thread:thread-1')).toBe(true)
    expect(entries.some((entry) => entry.id === 'message:thread-1:m-2')).toBe(true)
    expect(entries.some((entry) => entry.id === 'task:task-1')).toBe(true)
    expect(entries.some((entry) => entry.id === 'project:project-1')).toBe(true)
    expect(entries.some((entry) => entry.id === 'project-resource:project-1:resource-1')).toBe(true)
    expect(entries.some((entry) => entry.id === 'setting-safety')).toBe(true)
  })

  it('scores title matches ahead of body matches', () => {
    const entries = buildSearchIndex({
      threads: [thread],
      tasks: [task],
      projects: [project],
    })

    const results = filterSearchIndex(entries, 'Release')

    expect(results[0].id).toBe('thread:thread-1')
  })

  it('finds tasks, chat messages, resources and settings', () => {
    const entries = buildSearchIndex({
      threads: [thread],
      tasks: [task],
      projects: [project],
    })

    expect(filterSearchIndex(entries, 'permissions').some((entry) => entry.id === 'task:task-1')).toBe(true)
    expect(filterSearchIndex(entries, 'migration').some((entry) => entry.id === 'message:thread-1:m-2')).toBe(true)
    expect(filterSearchIndex(entries, 'smoke').some((entry) => entry.id === 'project-resource:project-1:resource-1')).toBe(true)
    expect(filterSearchIndex(entries, 'allowlist').some((entry) => entry.id === 'setting-safety')).toBe(true)
  })
})

