import { MessageSquare, Play, Square, Trash2 } from 'lucide-react'
import type { WorkTask } from '../../stores/workTasksStore'
import { tr } from '../../i18n'

type TaskRunToolbarProps = {
  task: WorkTask
  chatLabel?: string
  onOpenChat: (task: WorkTask) => void
  onRunTask: (task: WorkTask) => void
  onCancelTask: (task: WorkTask) => void
  onDeleteTask: (task: WorkTask) => void
  runDisabled?: boolean
  deleteDisabled?: boolean
}

export default function TaskRunToolbar({
  task,
  chatLabel,
  onOpenChat,
  onRunTask,
  onCancelTask,
  onDeleteTask,
  runDisabled = false,
  deleteDisabled = false,
}: TaskRunToolbarProps) {
  const canCancel = task.status === 'running' || task.status === 'waiting_approval'

  return (
    <div className="actions work-task-card-actions task-run-toolbar" data-doc-id="element:/tasks/task-run-toolbar">
      <button type="button" className="ui-button ui-button--secondary" data-doc-id="button:/tasks/task-run-toolbar/open-chat" onClick={() => onOpenChat(task)}>
        <MessageSquare size={15} aria-hidden="true" />
        {chatLabel ?? tr('Chat')}
      </button>
      <button type="button" className="ui-button ui-button--primary" data-doc-id="button:/tasks/task-run-toolbar/start" onClick={() => onRunTask(task)} disabled={runDisabled}>
        <Play size={15} aria-hidden="true" />
        {tr('Start')}
      </button>
      {canCancel && (
        <button type="button" className="ui-button ui-button--danger btn-stop" data-doc-id="button:/tasks/task-run-toolbar/stop" onClick={() => onCancelTask(task)}>
          <Square size={15} aria-hidden="true" />
          {tr('Stop')}
        </button>
      )}
      <button type="button" className="ui-button ui-button--danger" data-doc-id="button:/tasks/task-run-toolbar/delete" onClick={() => onDeleteTask(task)} disabled={deleteDisabled}>
        <Trash2 size={15} aria-hidden="true" />
        {tr('Delete')}
      </button>
    </div>
  )
}
