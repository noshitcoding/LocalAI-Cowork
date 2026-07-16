import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import type { Crew } from '../../stores/crewStore'
import type { WorkTask } from '../../stores/workTasksStore'
import TaskCreatePanel from './TaskCreatePanel'
import TaskDetailPane from './TaskDetailPane'
import TaskListPane from './TaskListPane'
import TaskSchedulerPanel from './TaskSchedulerPanel'

const crew = {
  id: 'crew-1',
  name: 'Research Crew',
  tasks: [{
    id: 'crew-task-1',
    description: 'Research topic',
    expectedOutput: 'Notes',
    status: 'pending',
    output: null,
  }],
} as Crew

const baseTask: WorkTask = {
  id: 'task-1',
  title: 'Weekly Report',
  prompt: 'Summarize the week',
  expectedOutput: 'Bullets',
  workDir: '',
  threadId: null,
  runner: 'model',
  crewId: null,
  model: '',
  scheduleExpr: '',
  scheduleEnabled: false,
  status: 'idle',
  output: null,
  error: null,
  lastRunAt: null,
  createdAt: 100,
  updatedAt: 100,
}

describe('task panels', () => {
  it('points empty crew setup to the Crew workspace, not Settings', () => {
    render(
      <TaskCreatePanel
        crews={[]}
        defaultModel="qwen3"
        open
        title=""
        prompt="Draft a task"
        expectedOutput=""
        workDir=""
        runner="crew"
        crewId=""
        model=""
        canCreateTask={false}
        onOpenChange={vi.fn()}
        onTitleChange={vi.fn()}
        onPromptChange={vi.fn()}
        onExpectedOutputChange={vi.fn()}
        onWorkDirChange={vi.fn()}
        onRunnerChange={vi.fn()}
        onCrewIdChange={vi.fn()}
        onModelChange={vi.fn()}
        onPickWorkDir={vi.fn()}
        onCreateTask={vi.fn()}
      />,
    )

    expect(screen.getByText('Create a crew under Crew first to run crew tasks.')).toBeInTheDocument()
    expect(screen.queryByText(/settings first/i)).not.toBeInTheDocument()
  })

  it('selects tasks and exposes one crew mission action', async () => {
    const user = userEvent.setup()
    const onSelectTask = vi.fn()
    const onImportCrewTasks = vi.fn()

    render(
      <TaskListPane
        tasks={[baseTask]}
        crews={[crew]}
        selectedTaskId={baseTask.id}
        importCrewId={crew.id}
        onSelectTask={onSelectTask}
        onImportCrewIdChange={vi.fn()}
        onImportCrewTasks={onImportCrewTasks}
        scheduledTasks={[]}
      />,
    )

    await user.click(screen.getByRole('button', { name: /Weekly Report/i }))
    await user.click(screen.getByRole('button', { name: /Create crew mission/i }))

    expect(onSelectTask).toHaveBeenCalledWith('task-1')
    expect(onImportCrewTasks).toHaveBeenCalledTimes(1)
  })

  it('disables mission creation when the selected crew already has a mission', () => {
    const onImportCrewTasks = vi.fn()

    render(
      <TaskListPane
        tasks={[{ ...baseTask, id: 'crew-mission-crew-1' }]}
        crews={[crew]}
        selectedTaskId="crew-mission-crew-1"
        importCrewId={crew.id}
        onSelectTask={vi.fn()}
        onImportCrewIdChange={vi.fn()}
        onImportCrewTasks={onImportCrewTasks}
        scheduledTasks={[]}
      />,
    )

    expect(screen.getByRole('button', { name: /Mission created/i })).toBeDisabled()
    expect(onImportCrewTasks).not.toHaveBeenCalled()
  })

  it('keeps run disabled for relative working folders', () => {
    render(
      <TaskDetailPane
        task={{ ...baseTask, workDir: 'relative/path' }}
        crews={[crew]}
        defaultModel="qwen3"
        scheduled={null}
        crewScheduleMetadata={null}
        projectContext={{ title: 'Project Alpha' }}
        onUpdateTask={vi.fn()}
        onPickWorkDir={vi.fn()}
        onOpenChat={vi.fn()}
        onRunTask={vi.fn()}
        onCancelTask={vi.fn()}
        onDeleteTask={vi.fn()}
        onToggleSchedule={vi.fn()}
        onSaveSchedule={vi.fn()}
        onRemoveSchedule={vi.fn()}
      />,
    )

    expect(screen.getByRole('button', { name: 'Start' })).toBeDisabled()
    expect(screen.getByText('Working folder must be absolute.')).toBeInTheDocument()
    expect(screen.getByText(/Project Alpha/)).toBeInTheDocument()
  })

  it('keeps waiting approval tasks cancellable while start remains disabled', async () => {
    const user = userEvent.setup()
    const onCancelTask = vi.fn()
    const waitingTask: WorkTask = {
      ...baseTask,
      status: 'waiting_approval',
    }

    render(
      <TaskDetailPane
        task={waitingTask}
        crews={[crew]}
        defaultModel="qwen3"
        scheduled={null}
        crewScheduleMetadata={null}
        projectContext={null}
        onUpdateTask={vi.fn()}
        onPickWorkDir={vi.fn()}
        onOpenChat={vi.fn()}
        onRunTask={vi.fn()}
        onCancelTask={onCancelTask}
        onDeleteTask={vi.fn()}
        onToggleSchedule={vi.fn()}
        onSaveSchedule={vi.fn()}
        onRemoveSchedule={vi.fn()}
      />,
    )

    expect(screen.getByRole('button', { name: 'Start' })).toBeDisabled()
    await user.click(screen.getByRole('button', { name: 'Stop' }))

    expect(onCancelTask).toHaveBeenCalledWith(waitingTask)
  })

  it('saves, removes, and toggles schedules from the scheduler panel', async () => {
    const user = userEvent.setup()
    const onToggleSchedule = vi.fn()
    const onSaveSchedule = vi.fn()
    const onRemoveSchedule = vi.fn()
    const task = { ...baseTask, scheduleExpr: 'daily 09:00' }

    render(
      <TaskSchedulerPanel
        task={task}
        scheduled={null}
        crewScheduleMetadata={null}
        onUpdateTask={vi.fn()}
        onToggleSchedule={onToggleSchedule}
        onSaveSchedule={onSaveSchedule}
        onRemoveSchedule={onRemoveSchedule}
      />,
    )

    await user.click(screen.getByRole('checkbox'))
    await user.click(screen.getByRole('button', { name: 'Save' }))
    await user.click(screen.getByRole('button', { name: 'Remove' }))

    expect(onToggleSchedule).toHaveBeenCalledWith(task, true)
    expect(onSaveSchedule).toHaveBeenCalledWith(task)
    expect(onRemoveSchedule).toHaveBeenCalledWith(task)
  })

  it('uses the loaded scheduler active state when it differs from the WorkTask flag', () => {
    render(
      <TaskSchedulerPanel
        task={{ ...baseTask, scheduleExpr: 'daily 09:00', scheduleEnabled: true }}
        scheduled={{
          id: baseTask.id,
          name: 'Weekly Report',
          prompt: baseTask.prompt,
          cronLike: 'daily 09:00',
          taskKind: 'prompt',
          crewId: null,
          crewSnapshotJson: null,
          modelConfigJson: null,
          priority: 100,
          dependsOnTaskIds: [],
          active: false,
          lastRunAt: null,
          nextRunAt: null,
        }}
        crewScheduleMetadata={null}
        onUpdateTask={vi.fn()}
        onToggleSchedule={vi.fn()}
        onSaveSchedule={vi.fn()}
        onRemoveSchedule={vi.fn()}
      />,
    )

    expect(screen.getByRole('checkbox')).not.toBeChecked()
    expect(screen.getByText('Job paused')).toBeInTheDocument()
  })
})
