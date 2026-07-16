import { Save, Trash2 } from 'lucide-react'
import type { ScheduledTask } from '../../stores/coworkStore'
import type { WorkTask } from '../../stores/workTasksStore'
import { tr } from '../../i18n'
import { formatTimestamp } from '../../engine/tasks/workTaskExecutionService'
import type { CrewScheduleSnapshotMetadata } from '../../engine/tasks/workTaskScheduleService'

type TaskSchedulerPanelProps = {
  task: WorkTask
  scheduled: ScheduledTask | null
  crewScheduleMetadata: CrewScheduleSnapshotMetadata | null
  onUpdateTask: (id: string, patch: Partial<Omit<WorkTask, 'id' | 'createdAt'>>) => void
  onToggleSchedule: (task: WorkTask, enabled: boolean) => void
  onSaveSchedule: (task: WorkTask) => void
  onRemoveSchedule: (task: WorkTask) => void
}

export default function TaskSchedulerPanel({
  task,
  scheduled,
  crewScheduleMetadata,
  onUpdateTask,
  onToggleSchedule,
  onSaveSchedule,
  onRemoveSchedule,
}: TaskSchedulerPanelProps) {
  const resolvedScheduleExpr = task.scheduleExpr.trim() || scheduled?.cronLike || ''
  const taskForSchedule = resolvedScheduleExpr === task.scheduleExpr ? task : { ...task, scheduleExpr: resolvedScheduleExpr }
  const active = scheduled ? scheduled.active : task.scheduleEnabled

  return (
    <div className="task-scheduler-panel" data-doc-id="element:/tasks/task-scheduler-panel">
      <div className="task-scheduler-header">
        <strong>{tr('Scheduler')}</strong>
        <div className="task-scheduler-meta">
          {tr('Last run')}: {formatTimestamp(scheduled?.lastRunAt ?? null)} / {tr('Next run')}: {formatTimestamp(scheduled?.nextRunAt ?? null)}
        </div>
      </div>

      <div className="grid task-scheduler-grid">
        <label>
          {tr('Expression')}
          <input
            className="ui-field"
            value={resolvedScheduleExpr}
            onChange={(e) => onUpdateTask(task.id, { scheduleExpr: e.target.value })}
            placeholder={tr('e.g. daily 09:00')}
          />
        </label>
        <label>
          {tr('Active')}
          <div className="task-checkbox-row">
            <input
              type="checkbox"
              data-doc-id="button:/tasks/task-scheduler-panel/toggle-active"
              checked={active}
              onChange={(e) => onToggleSchedule(task, e.target.checked)}
            />
            <span className="hint-text">{active ? tr('Job active') : tr('Job paused')}</span>
          </div>
        </label>
      </div>
      <div className="actions task-scheduler-actions">
        <button type="button" className="ui-button ui-button--primary ui-button--sm" data-doc-id="button:/tasks/task-scheduler-panel/save" onClick={() => onSaveSchedule(taskForSchedule)} disabled={!resolvedScheduleExpr}>
          <Save size={14} aria-hidden="true" />
          {tr('Save')}
        </button>
        <button type="button" className="ui-button ui-button--secondary ui-button--sm" data-doc-id="button:/tasks/task-scheduler-panel/remove" onClick={() => onRemoveSchedule(task)} disabled={!scheduled && !resolvedScheduleExpr}>
          <Trash2 size={14} aria-hidden="true" />
          {tr('Remove')}
        </button>
        {task.runner === 'crew' && !task.crewId ? (
          <span className="hint-text">{tr('Crew required for crew schedule')}</span>
        ) : null}
      </div>
      {task.runner === 'crew' && crewScheduleMetadata ? (
        <div className="hint-text task-scheduler-source">
          {crewScheduleMetadata.snapshotSource === 'saved-version'
            ? `${tr('Source')}: ${tr('saved crew version')} v${crewScheduleMetadata.definitionVersionNumber ?? '-'}${crewScheduleMetadata.definitionSavedAt ? ` ${tr('from')} ${new Date(crewScheduleMetadata.definitionSavedAt).toLocaleString('de-DE')}` : ''}${crewScheduleMetadata.definitionChangeSummary ? ` / ${crewScheduleMetadata.definitionChangeSummary}` : ''}`
            : `${tr('Source')}: ${tr('current crew editor state')}`}
        </div>
      ) : null}
    </div>
  )
}
