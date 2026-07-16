import { FolderOpen, PanelTopOpen } from 'lucide-react'
import type { Crew } from '../../stores/crewStore'
import type { ScheduledTask } from '../../stores/coworkStore'
import type { WorkTask, WorkTaskRunner } from '../../stores/workTasksStore'
import { tr } from '../../i18n'
import { deriveTaskName, formatWorkTaskStatus, isAbsolutePath } from '../../engine/tasks/workTaskExecutionService'
import type { CrewScheduleSnapshotMetadata } from '../../engine/tasks/workTaskScheduleService'
import TaskRunToolbar from './TaskRunToolbar'
import TaskSchedulerPanel from './TaskSchedulerPanel'

type TaskProjectContext = {
  title: string
} | null

type TaskDetailPaneProps = {
  task: WorkTask | null
  crews: Crew[]
  defaultModel: string
  scheduled: ScheduledTask | null
  crewScheduleMetadata: CrewScheduleSnapshotMetadata | null
  projectContext: TaskProjectContext
  onUpdateTask: (id: string, patch: Partial<Omit<WorkTask, 'id' | 'createdAt'>>) => void
  onPickWorkDir: (task: WorkTask) => void
  onOpenChat: (task: WorkTask) => void
  onRunTask: (task: WorkTask) => void
  onCancelTask: (task: WorkTask) => void
  onDeleteTask: (task: WorkTask) => void
  onToggleSchedule: (task: WorkTask, enabled: boolean) => void
  onSaveSchedule: (task: WorkTask) => void
  onRemoveSchedule: (task: WorkTask) => void
}

export default function TaskDetailPane({
  task,
  crews,
  defaultModel,
  scheduled,
  crewScheduleMetadata,
  projectContext,
  onUpdateTask,
  onPickWorkDir,
  onOpenChat,
  onRunTask,
  onCancelTask,
  onDeleteTask,
  onToggleSchedule,
  onSaveSchedule,
  onRemoveSchedule,
}: TaskDetailPaneProps) {
  if (!task) {
    return (
      <section className="task-detail-pane task-detail-empty" data-doc-id="element:/tasks/task-detail-pane" aria-label={tr('Task detail')}>
        <span className="task-empty-icon task-empty-icon-large" aria-hidden="true"><PanelTopOpen size={22} /></span>
        <span className="task-detail-kicker">{tr('Task workspace')}</span>
        <h2>{tr('No task selected')}</h2>
        <p className="hint-text">{tr('Create or select a task to edit its runner, schedule, chat, and output.')}</p>
      </section>
    )
  }

  const crewName = task.crewId ? crews.find((crew) => crew.id === task.crewId)?.name : null
  const invalidWorkDir = Boolean(task.workDir.trim() && !isAbsolutePath(task.workDir))
  const isBusy = task.status === 'running' || task.status === 'waiting_approval'
  const runDisabled = isBusy
    || !task.prompt.trim()
    || (task.runner === 'crew' && !task.crewId)
    || invalidWorkDir

  return (
    <section className="task-detail-pane" data-doc-id="element:/tasks/task-detail-pane" aria-label={tr('Task detail')}>
      <div className="work-task-card-header">
        <div className="work-task-title-row">
          <strong>{deriveTaskName(task)}</strong>
          <span className="ui-badge task-pill task-pill-runner">
            {task.runner === 'crew' ? tr('Crew') : tr('Model')}
          </span>
          <span className={`ui-badge task-pill task-status task-status-${task.status}`}>
            {formatWorkTaskStatus(task.status)}
          </span>
        </div>
        <TaskRunToolbar
          task={task}
          chatLabel={task.threadId ? tr('Open chat') : tr('Create chat')}
          onOpenChat={onOpenChat}
          onRunTask={onRunTask}
          onCancelTask={onCancelTask}
          onDeleteTask={onDeleteTask}
          runDisabled={runDisabled}
          deleteDisabled={task.status === 'running'}
        />
      </div>

      <div className="task-context-strip">
        <span>
          {tr('Project context')}: {projectContext ? projectContext.title : tr('No linked project')}
        </span>
        {task.threadId ? (
          <span>{tr('Chat')}: {task.threadId}</span>
        ) : (
          <span>{tr('No task chat yet')}</span>
        )}
      </div>

      <div className="grid task-edit-grid">
        <label>
          {tr('Title')}
          <input className="ui-field" value={task.title} onChange={(e) => onUpdateTask(task.id, { title: e.target.value })} />
        </label>
        <label>
          {tr('Execution')}
          <select className="ui-field" value={task.runner} onChange={(e) => onUpdateTask(task.id, { runner: e.target.value as WorkTaskRunner })}>
            <option value="crew">{tr('Crew')}</option>
            <option value="model">{tr('Model')}</option>
          </select>
        </label>
        {task.runner === 'crew' ? (
          <label>
            {tr('Crew')}
            <select className="ui-field" value={task.crewId ?? ''} onChange={(e) => onUpdateTask(task.id, { crewId: e.target.value || null })}>
              <option value="">{tr('Select crew')}</option>
              {crews.map((crew) => (
                <option key={crew.id} value={crew.id}>{crew.name}</option>
              ))}
            </select>
            {task.crewId && !crewName ? (
              <div className="hint-text">{tr('Assigned crew no longer exists.')}</div>
            ) : null}
          </label>
        ) : (
          <label>
            {tr('Model (optional)')}
            <input
              className="ui-field"
              value={task.model}
              onChange={(e) => onUpdateTask(task.id, { model: e.target.value })}
              placeholder={`${tr('Default')}: ${defaultModel || '-'}`}
            />
          </label>
        )}
        <label>
          {tr('Expected output')}
          <input className="ui-field" value={task.expectedOutput} onChange={(e) => onUpdateTask(task.id, { expectedOutput: e.target.value })} />
        </label>
        <label className="task-field-full">
          {tr('Working folder (absolute)')}
          <div className="task-inline-field">
            <input className="ui-field" value={task.workDir} onChange={(e) => onUpdateTask(task.id, { workDir: e.target.value })} placeholder="C:\\Projects\\my-task" />
            <button type="button" className="ui-button ui-button--secondary" data-doc-id="button:/tasks/task-detail-pane/choose-folder" onClick={() => onPickWorkDir(task)}>
              <FolderOpen size={15} aria-hidden="true" />
              {tr('Choose folder')}
            </button>
          </div>
          {invalidWorkDir ? (
            <div className="hint-text">{tr('Working folder must be absolute.')}</div>
          ) : null}
        </label>
        <label className="task-field-full">
          {tr('Task')}
          <textarea className="ui-field" value={task.prompt} onChange={(e) => onUpdateTask(task.id, { prompt: e.target.value })} rows={4} />
        </label>
      </div>

      <TaskSchedulerPanel
        task={task}
        scheduled={scheduled}
        crewScheduleMetadata={crewScheduleMetadata}
        onUpdateTask={onUpdateTask}
        onToggleSchedule={onToggleSchedule}
        onSaveSchedule={onSaveSchedule}
        onRemoveSchedule={onRemoveSchedule}
      />

      {(task.output || task.error) && (
        <pre className="task-output-preview">
          {(task.error ?? task.output ?? '').slice(0, 6000)}
        </pre>
      )}
    </section>
  )
}

