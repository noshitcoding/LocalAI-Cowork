import { useState } from 'react'
import { ChevronDown, FolderOpen, Plus, SlidersHorizontal, Sparkles } from 'lucide-react'
import type { Crew } from '../../stores/crewStore'
import type { WorkTaskRunner } from '../../stores/workTasksStore'
import { tr } from '../../i18n'
import { isAbsolutePath } from '../../engine/tasks/workTaskExecutionService'

type TaskCreatePanelProps = {
  crews: Crew[]
  defaultModel: string
  title: string
  prompt: string
  expectedOutput: string
  workDir: string
  runner: WorkTaskRunner
  crewId: string
  model: string
  canCreateTask: boolean
  onTitleChange: (value: string) => void
  onPromptChange: (value: string) => void
  onExpectedOutputChange: (value: string) => void
  onWorkDirChange: (value: string) => void
  onRunnerChange: (runner: WorkTaskRunner) => void
  onCrewIdChange: (crewId: string) => void
  onModelChange: (model: string) => void
  onPickWorkDir: () => void
  onCreateTask: () => void
}

export default function TaskCreatePanel({
  crews,
  defaultModel,
  title,
  prompt,
  expectedOutput,
  workDir,
  runner,
  crewId,
  model,
  canCreateTask,
  onTitleChange,
  onPromptChange,
  onExpectedOutputChange,
  onWorkDirChange,
  onRunnerChange,
  onCrewIdChange,
  onModelChange,
  onPickWorkDir,
  onCreateTask,
}: TaskCreatePanelProps) {
  const normalizedWorkDir = workDir.trim()
  const [advancedOpen, setAdvancedOpen] = useState(Boolean(expectedOutput.trim() || normalizedWorkDir))

  return (
    <section className="panel task-create-panel" data-doc-id="element:/tasks/task-create-panel">
      <div className="task-create-heading">
        <span className="task-create-icon" aria-hidden="true"><Sparkles size={17} /></span>
        <div>
          <h2>{tr('New task')}</h2>
          <p>{tr('Define the outcome once, then run it now or schedule it for later.')}</p>
        </div>
      </div>
      <div className="grid">
        <label>
          {tr('Title (optional)')}
          <input className="ui-field" value={title} onChange={(e) => onTitleChange(e.target.value)} placeholder={tr('e.g. Weekly Report')} />
        </label>
        <label>
          {tr('Execution')}
          <select className="ui-field" value={runner} onChange={(e) => onRunnerChange(e.target.value as WorkTaskRunner)}>
            <option value="crew">{tr('Crew')}</option>
            <option value="model">{tr('Model')}</option>
          </select>
        </label>
        {runner === 'crew' ? (
          <label>
            {tr('Crew')}
            <select className="ui-field" value={crewId} onChange={(e) => onCrewIdChange(e.target.value)}>
              {crews.length === 0 && (
                <option value="">{tr('No crews available')}</option>
              )}
              {crews.map((crew) => (
                <option key={crew.id} value={crew.id}>{crew.name}</option>
              ))}
            </select>
          </label>
        ) : (
          <label>
            {tr('Model (optional)')}
            <input className="ui-field" value={model} onChange={(e) => onModelChange(e.target.value)} placeholder={`${tr('Default')}: ${defaultModel || '-'}`} />
          </label>
        )}
        <label className="task-field-full">
          {tr('Task')}
          <textarea className="ui-field" value={prompt} onChange={(e) => onPromptChange(e.target.value)} rows={3} placeholder={tr('What should the task do?')} />
        </label>
      </div>
      <button
        type="button"
        className="task-advanced-toggle"
        aria-expanded={advancedOpen}
        aria-controls="task-create-advanced"
        onClick={() => setAdvancedOpen((open) => !open)}
      >
        <span className="task-advanced-toggle-icon" aria-hidden="true"><SlidersHorizontal size={16} /></span>
        <span className="task-advanced-toggle-copy">
          <strong>{tr('Advanced setup')}</strong>
          <small>{tr('Optional output and workspace controls')}</small>
        </span>
        <ChevronDown className={advancedOpen ? 'is-open' : ''} size={17} aria-hidden="true" />
      </button>
      {advancedOpen && (
        <div id="task-create-advanced" className="grid task-create-advanced">
          <label>
            {tr('Expected output (optional)')}
            <input className="ui-field" value={expectedOutput} onChange={(e) => onExpectedOutputChange(e.target.value)} placeholder={tr('e.g. Bullet report')} />
          </label>
          <label>
            {tr('Working folder (optional, absolute)')}
            <div className="task-inline-field">
              <input className="ui-field" value={workDir} onChange={(e) => onWorkDirChange(e.target.value)} placeholder="C:\\Projects\\my-task" />
              <button type="button" className="ui-button ui-button--secondary" data-doc-id="button:/tasks/task-create-panel/choose-folder" onClick={onPickWorkDir}>
                <FolderOpen size={15} aria-hidden="true" />
                {tr('Choose folder')}
              </button>
            </div>
            {normalizedWorkDir && !isAbsolutePath(normalizedWorkDir) ? (
              <div className="hint-text">{tr('Working folder must be absolute.')}</div>
            ) : null}
          </label>
        </div>
      )}
      <div className="actions task-create-actions">
        <button type="button" className="ui-button ui-button--primary" data-doc-id="button:/tasks/task-create-panel/create" onClick={onCreateTask} disabled={!canCreateTask}>
          <Plus size={15} aria-hidden="true" />
          {tr('Create task')}
        </button>
      </div>
      {runner === 'crew' && crews.length === 0 && (
        <p className="hint-text">{tr('Create a crew under Crew first to run crew tasks.')}</p>
      )}
    </section>
  )
}
