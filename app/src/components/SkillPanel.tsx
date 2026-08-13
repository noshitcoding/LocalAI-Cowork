import { useEffect, useState } from 'react'
import { Pencil, Trash2 } from 'lucide-react'
import i18n, { tr } from '../i18n'
import { useSkillStore, type Skill } from '../stores/skillStore'

type SkillTab = 'skills' | 'learnings'

const emptyForm = {
  name: '',
  description: '',
  promptTemplate: '',
  triggerPattern: '',
  runMode: 'execute',
}

const runModeLabels: Record<string, string> = {
  execute: 'Run',
  plan: 'Plan',
  hybrid: 'Hybrid',
}

function randomId() {
  return `skill-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}

function formatDateTime(value: string) {
  const date = new Date(value)
  return Number.isNaN(date.getTime())
    ? value
    : date.toLocaleString(i18n.resolvedLanguage ?? i18n.language ?? 'en')
}

function getRunModeLabel(mode: string) {
  return tr(runModeLabels[mode] ?? mode)
}

function getTabLabel(tab: SkillTab) {
  return tab === 'skills' ? tr('Skills') : tr('Learning history')
}

export default function SkillPanel() {
  const {
    skills, learnings, loading, error,
    loadSkills, upsertSkill, deleteSkill, improveSkill,
    loadLearnings,
  } = useSkillStore()

  const [tab, setTab] = useState<SkillTab>('skills')
  const [showAdd, setShowAdd] = useState(false)
  const [form, setForm] = useState(emptyForm)
  const [improveId, setImproveId] = useState<string | null>(null)
  const [improveTemplate, setImproveTemplate] = useState('')
  const [improveReason, setImproveReason] = useState('')

  useEffect(() => {
    void loadSkills()
  }, [loadSkills])

  useEffect(() => {
    if (tab === 'learnings') void loadLearnings()
  }, [tab, loadLearnings])

  const handleAdd = async () => {
    if (!form.name.trim() || !form.promptTemplate.trim()) return
    await upsertSkill({
      id: randomId(),
      name: form.name.trim(),
      description: form.description,
      promptTemplate: form.promptTemplate,
      triggerPattern: form.triggerPattern || undefined,
      runMode: form.runMode,
    })
    setForm(emptyForm)
    setShowAdd(false)
    void loadSkills()
  }

  const handleImprove = async () => {
    if (!improveId || !improveTemplate.trim()) return
    await improveSkill(improveId, improveTemplate, improveReason)
    setImproveId(null)
    setImproveTemplate('')
    setImproveReason('')
    void loadSkills()
  }

  const handleDeleteSkill = async (id: string) => {
    await deleteSkill(id)
    void loadSkills()
  }

  return (
    <div className="panel skill-panel">
      <h2>{tr('Skills and learning history')}</h2>

      <div className="skill-tab-row" role="tablist" aria-label={tr('Skills and learning history')}>
        {(['skills', 'learnings'] as SkillTab[]).map((item) => (
          <button
            key={item}
            type="button"
            role="tab"
            aria-selected={tab === item}
            className={`btn-sm${tab === item ? ' active' : ''}`}
            onClick={() => setTab(item)}
          >
            {getTabLabel(item)}
          </button>
        ))}
      </div>

      {error && <p className="skill-error">{error}</p>}

      {tab === 'skills' && (
        <>
          <div className="skill-summary-row">
            <span className="skill-count">
              {skills.length} {tr('Skills registered')}
            </span>
            <button type="button" className="btn-sm" onClick={() => setShowAdd(!showAdd)}>
              {tr('+ New skill')}
            </button>
          </div>

          {showAdd && (
            <div className="card skill-form-card">
              <div className="grid skill-form-grid">
                <label className="skill-label">
                  {tr('Name')}
                  <input type="text" value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} />
                </label>
                <label className="skill-label">
                  {tr('Mode')}
                  <select value={form.runMode} onChange={(e) => setForm({ ...form, runMode: e.target.value })}>
                    <option value="execute">{tr('Run')}</option>
                    <option value="plan">{tr('Plan')}</option>
                    <option value="hybrid">{tr('Hybrid')}</option>
                  </select>
                </label>
              </div>
              <label className="skill-label">
                {tr('Description')}
                <input
                  type="text"
                  value={form.description}
                  onChange={(e) => setForm({ ...form, description: e.target.value })}
                />
              </label>
              <label className="skill-label">
                {tr('Prompt-Template')}
                <textarea
                  value={form.promptTemplate}
                  onChange={(e) => setForm({ ...form, promptTemplate: e.target.value })}
                  rows={3}
                />
              </label>
              <label className="skill-label">
                {tr('Trigger-Pattern (optional)')}
                <input
                  type="text"
                  value={form.triggerPattern}
                  onChange={(e) => setForm({ ...form, triggerPattern: e.target.value })}
                  placeholder={tr('e.g. *test*, *deploy*')}
                />
              </label>
              <button type="button" className="btn-sm" onClick={handleAdd}>{tr('Save')}</button>
            </div>
          )}

          {loading ? (
            <p className="panel-empty">{tr('Loading...')}</p>
          ) : skills.length === 0 ? (
            <p className="panel-empty">{tr('No skills created yet')}</p>
          ) : (
            <div className="skill-list">
              {skills.map((skill: Skill) => (
                <div key={skill.id} className="card skill-card">
                  <div className="skill-card-header">
                    <div className="skill-main">
                      <div className="skill-title-row">
                        <span className="skill-name">{skill.name}</span>
                        {skill.auto_generated && <span className="skill-auto-badge">{tr('auto')}</span>}
                      </div>
                      <div className="skill-description">{skill.description}</div>
                      <div className="skill-meta">
                        <span>{tr('Mode:')} {getRunModeLabel(skill.run_mode)}</span>
                        <span>{tr('Usage:')} {skill.usage_count}</span>
                        <span>{tr('Success:')} {skill.success_count}/{skill.usage_count}</span>
                        <span>{tr('Quality:')} {skill.avg_quality.toFixed(1)}</span>
                        {skill.trigger_pattern && <span>{tr('Trigger:')} {skill.trigger_pattern}</span>}
                      </div>
                    </div>
                    <div className="skill-actions">
                      <button
                        type="button"
                        className="btn-sm"
                        onClick={() => {
                          setImproveId(skill.id)
                          setImproveTemplate(skill.prompt_template)
                        }}
                        title={tr('Improve')}
                        aria-label={tr('Improve skill')}
                      >
                        <Pencil size={14} aria-hidden="true" />
                      </button>
                      <button
                        type="button"
                        className="skill-danger-button"
                        onClick={() => { void handleDeleteSkill(skill.id) }}
                        title={tr('Delete skill')}
                        aria-label={tr('Delete skill')}
                      >
                        <Trash2 size={14} aria-hidden="true" />
                      </button>
                    </div>
                  </div>

                  {improveId === skill.id && (
                    <div className="skill-improve-panel">
                      <label className="skill-label">
                        {tr('New prompt template')}
                        <textarea value={improveTemplate} onChange={(e) => setImproveTemplate(e.target.value)} rows={3} />
                      </label>
                      <label className="skill-label">
                        {tr('Reason')}
                        <input type="text" value={improveReason} onChange={(e) => setImproveReason(e.target.value)} />
                      </label>
                      <div className="skill-improve-actions">
                        <button type="button" className="btn-sm" onClick={handleImprove}>{tr('Apply')}</button>
                        <button type="button" className="btn-sm" onClick={() => setImproveId(null)}>{tr('Cancel')}</button>
                      </div>
                    </div>
                  )}
                </div>
              ))}
            </div>
          )}
        </>
      )}

      {tab === 'learnings' && (
        <>
          {loading ? (
            <p className="panel-empty">{tr('Loading...')}</p>
          ) : learnings.length === 0 ? (
            <p className="panel-empty">{tr('No learning entries yet')}</p>
          ) : (
            <div className="skill-list">
              {learnings.map((learning) => (
                <div key={learning.id} className="card skill-learning-card">
                  <div className="skill-learning-meta">
                    <span>{learning.outcome_type}</span>
                    <span>{tr('Confidence')}: {learning.confidence.toFixed(2)}</span>
                    <span>{formatDateTime(learning.created_at)}</span>
                  </div>
                  <div className="skill-learning-description">{learning.description}</div>
                  {learning.learned_pattern && (
                    <div className="skill-learning-pattern">
                      {tr('Pattern:')} {learning.learned_pattern}
                    </div>
                  )}
                </div>
              ))}
            </div>
          )}
        </>
      )}
    </div>
  )
}
