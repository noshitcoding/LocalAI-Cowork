import { ListChecks, Sparkles } from 'lucide-react'
import type { Crew } from '../../stores/crewStore'
import type { ScheduledTask } from '../../stores/coworkStore'
import type { WorkTask } from '../../stores/workTasksStore'
import { tr } from '../../i18n'
import { buildCrewMissionId, deriveTaskName, formatWorkTaskStatus } from '../../engine/tasks/workTaskExecutionService'
import { findScheduledTask } from '../../engine/tasks/workTaskScheduleService'

type TaskListPaneProps = {
  tasks: WorkTask[]
  crews: Crew[]
  selectedTaskId: string | null
  importCrewId: string
  onSelectTask: (taskId: string) => void
  onImportCrewIdChange: (crewId: string) => void
  onImportCrewTasks: () => void
  scheduledTasks: ScheduledTask[]
}

export default function TaskListPane({
  tasks,
  crews,
  selectedTaskId,
  importCrewId,
  onSelectTask,
  onImportCrewIdChange,
  onImportCrewTasks,
  scheduledTasks,
}: TaskListPaneProps) {
  const importCrew = crews.find((crew) => crew.id === importCrewId) ?? null
  const existingTaskIds = new Set(tasks.map((task) => task.id))
  const missionExists = importCrew ? existingTaskIds.has(buildCrewMissionId(importCrew.id)) : false

  return (
    <aside className="task-list-pane" data-doc-id="element:/tasks/task-list-pane" aria-label={tr('Tasks')}>
      <div className="task-list-pane-header">
        <div>
          <h2>{tr('Your tasks')}</h2>
          <span className="hint-text">{tasks.length} {tr('task(s)')}</span>
        </div>
      </div>

      <div className="task-import-strip" data-doc-id="element:/tasks/crew-task-import">
        <label>
          {tr('Crew mission')}
          <select className="ui-field" value={importCrewId} onChange={(e) => onImportCrewIdChange(e.target.value)}>
            {crews.length === 0 && (
              <option value="">{tr('No crews available')}</option>
            )}
            {crews.map((crew) => (
              <option key={crew.id} value={crew.id}>{crew.name}</option>
            ))}
          </select>
        </label>
        <button type="button" className="ui-button ui-button--secondary" data-doc-id="button:/tasks/crew-task-import/import" onClick={onImportCrewTasks} disabled={!importCrewId || missionExists}>
          <Sparkles size={15} aria-hidden="true" />
          {missionExists ? tr('Mission created') : tr('Create crew mission')}
        </button>
      </div>

      {tasks.length === 0 ? (
        <div className="task-list-empty">
          <span className="task-empty-icon" aria-hidden="true"><ListChecks size={20} /></span>
          <strong>{tr('Your queue is clear')}</strong>
          <p className="hint-text">{tr('Create your first task above or turn a crew into one complete mission.')}</p>
        </div>
      ) : (
        <div className="task-list-items" role="list">
          {tasks.map((task) => {
            const scheduled = findScheduledTask(scheduledTasks, task.id)
            const selected = task.id === selectedTaskId

            return (
              <button
                key={task.id}
                type="button"
                className={`task-list-item${selected ? ' active' : ''}`}
                data-doc-id="button:/tasks/task-list-pane/select-task"
                aria-current={selected ? 'true' : undefined}
                onClick={() => onSelectTask(task.id)}
              >
                <span className="task-list-item-title">{deriveTaskName(task)}</span>
                <span className="task-list-item-meta">
                  <span className="ui-badge task-pill task-pill-runner">
                    {task.runner === 'crew' ? tr('Crew') : tr('Model')}
                  </span>
                  <span className={`ui-badge task-pill task-status task-status-${task.status}`}>
                    {formatWorkTaskStatus(task.status)}
                  </span>
                  {scheduled ? <span className="ui-badge task-pill task-pill-scheduled">{tr('Scheduled')}</span> : null}
                </span>
              </button>
            )
          })}
        </div>
      )}
    </aside>
  )
}
