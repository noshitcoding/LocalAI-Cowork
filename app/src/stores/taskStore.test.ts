import { describe, it, expect, beforeEach } from 'vitest'
import { useTaskStore } from './taskStore'

describe('taskStore', () => {
  beforeEach(() => {
    useTaskStore.setState({
      tasks: [],
      activeTaskId: null,
    })
  })

  it('creates a task', () => {
    const id = useTaskStore.getState().createTask('Do something', 'Test Task', null)
    const state = useTaskStore.getState()
    expect(state.tasks).toHaveLength(1)
    expect(state.tasks[0].id).toBe(id)
    expect(state.tasks[0].title).toBe('Test Task')
    expect(state.tasks[0].prompt).toBe('Do something')
    expect(state.tasks[0].status).toBe('created')
    expect(state.activeTaskId).toBe(id)
  })

  it('updates task status', () => {
    const id = useTaskStore.getState().createTask('Test', 'Task', null)
    useTaskStore.getState().updateTaskStatus(id, 'planned')
    expect(useTaskStore.getState().tasks[0].status).toBe('planned')
    useTaskStore.getState().setTaskSteps(id, [
      { id: 's1', index: 0, title: 'Step 1', state: 'pending', requiresApproval: false, riskLevel: 'low', output: null },
    ])
    useTaskStore.getState().updateTaskStatus(id, 'running')
    expect(useTaskStore.getState().tasks[0].status).toBe('running')
  })

  it('fails fast when starting a task without steps', () => {
    const id = useTaskStore.getState().createTask('Test', 'Task', null)
    useTaskStore.getState().updateTaskStatus(id, 'running')
    const task = useTaskStore.getState().tasks[0]
    expect(task.status).toBe('failed')
    expect(task.error).toBe('Task kann nicht ohne Schritte gestartet werden.')
  })

  it('sets task steps', () => {
    const id = useTaskStore.getState().createTask('Test', 'Task', null)
    const steps = [
      { id: 's1', index: 0, title: 'Step 1', state: 'pending' as const, requiresApproval: false, riskLevel: 'low' as const, output: null },
      { id: 's2', index: 1, title: 'Step 2', state: 'pending' as const, requiresApproval: true, riskLevel: 'high' as const, output: null },
    ]
    useTaskStore.getState().setTaskSteps(id, steps)
    expect(useTaskStore.getState().tasks[0].steps).toHaveLength(2)
    expect(useTaskStore.getState().tasks[0].steps[1].requiresApproval).toBe(true)
  })

  it('updates a step', () => {
    const id = useTaskStore.getState().createTask('Test', 'Task', null)
    useTaskStore.getState().setTaskSteps(id, [
      { id: 's1', index: 0, title: 'Step 1', state: 'pending', requiresApproval: false, riskLevel: 'low', output: null },
    ])
    useTaskStore.getState().updateStep(id, 's1', { state: 'completed', output: 'done' })
    const step = useTaskStore.getState().tasks[0].steps[0]
    expect(step.state).toBe('completed')
    expect(step.output).toBe('done')
  })

  it('sets task error and updates status to failed', () => {
    const id = useTaskStore.getState().createTask('Test', 'Task', null)
    useTaskStore.getState().setTaskError(id, 'Something went wrong')
    const task = useTaskStore.getState().tasks[0]
    expect(task.status).toBe('failed')
    expect(task.error).toBe('Something went wrong')
  })

  it('sets active task', () => {
    const id = useTaskStore.getState().createTask('Test', 'Task', null)
    useTaskStore.getState().setActiveTask(null)
    expect(useTaskStore.getState().activeTaskId).toBeNull()
    useTaskStore.getState().setActiveTask(id)
    expect(useTaskStore.getState().activeTaskId).toBe(id)
  })
})
