import { useEffect, useState } from 'react'
import { usePersonalityStore, type Personality } from '../stores/personalityStore'
import { useConfigStore } from '../stores/configStore'
import type { AgentRole } from '../stores/crewStore'
import { tr } from '../i18n'

type PersonalityForm = {
  name: string
  role: AgentRole
  goal: string
  systemPrompt: string
  skillsMarkdown: string
  icon: string
  modelOverride: string
  temperature: string
  isDefault: boolean
}

const EMPTY_FORM: PersonalityForm = {
  name: '',
  role: 'custom',
  goal: '',
  systemPrompt: '',
  skillsMarkdown: '',
  icon: 'AI',
  modelOverride: '',
  temperature: '',
  isDefault: false,
}

const ROLE_OPTIONS: AgentRole[] = ['researcher', 'writer', 'reviewer', 'planner', 'executor', 'analyst', 'custom']

function randomId() {
  return `pers-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}

function toForm(personality: Personality): PersonalityForm {
  return {
    name: personality.name,
    role: personality.role,
    goal: personality.goal || personality.description,
    systemPrompt: personality.system_prompt,
    skillsMarkdown: personality.skills_markdown,
    icon: personality.icon || 'AI',
    modelOverride: personality.model_override || '',
    temperature: personality.temperature != null ? String(personality.temperature) : '',
    isDefault: personality.is_default,
  }
}

function PersonalityEditor({
  form,
  availableModels,
  onChange,
  onSave,
  onCancel,
  showDefaultToggle,
}: {
  form: PersonalityForm
  availableModels: string[]
  onChange: (form: PersonalityForm) => void
  onSave: () => void
  onCancel?: () => void
  showDefaultToggle: boolean
}) {
  return (
    <div>
      <div className="grid" style={{ gridTemplateColumns: '80px 1fr 160px 1fr', marginBottom: 8 }}>
        <label>{tr("Icon")}<input type="text" value={form.icon} onChange={(e) => onChange({ ...form, icon: e.target.value })} maxLength={4} />
        </label>
        <label>{tr("Name")}<input type="text" value={form.name} onChange={(e) => onChange({ ...form, name: e.target.value })} />
        </label>
        <label>{tr("Rolle")}<select value={form.role} onChange={(e) => onChange({ ...form, role: e.target.value as AgentRole })}>
            {ROLE_OPTIONS.map((role) => <option key={role} value={role}>{role}</option>)}
          </select>
        </label>
        <label>{tr("Model")}<select value={form.modelOverride} onChange={(e) => onChange({ ...form, modelOverride: e.target.value })}>
            <option value="">{tr("Standard")}</option>
            {availableModels.map((model) => <option key={model} value={model}>{model}</option>)}
            {form.modelOverride && !availableModels.includes(form.modelOverride) && (
              <option value={form.modelOverride}>{form.modelOverride}</option>
            )}
          </select>
        </label>
      </div>
      <label style={{ display: 'flex', flexDirection: 'column', gap: 4, fontSize: 13, marginBottom: 8 }}>{tr("Target / Prompt-Fokus")}<textarea
          value={form.goal}
          onChange={(e) => onChange({ ...form, goal: e.target.value })}
          rows={3}
          style={{ padding: '6px 10px', borderRadius: 'var(--radius-sm)', border: '1px solid var(--border-color)', fontSize: 13, resize: 'vertical' }}
        />
      </label>
      <label style={{ display: 'flex', flexDirection: 'column', gap: 4, fontSize: 13, marginBottom: 8 }}>{tr("Background / system prompt")}<textarea
          value={form.systemPrompt}
          onChange={(e) => onChange({ ...form, systemPrompt: e.target.value })}
          rows={8}
          style={{ padding: '6px 10px', borderRadius: 'var(--radius-sm)', border: '1px solid var(--border-color)', fontSize: 13, resize: 'vertical', fontFamily: 'monospace' }}
        />
      </label>
      <label style={{ display: 'flex', flexDirection: 'column', gap: 4, fontSize: 13, marginBottom: 8 }}>{tr("skills.md")}<textarea
          value={form.skillsMarkdown}
          onChange={(e) => onChange({ ...form, skillsMarkdown: e.target.value })}
          rows={4}
          style={{ padding: '6px 10px', borderRadius: 'var(--radius-sm)', border: '1px solid var(--border-color)', fontSize: 13, resize: 'vertical', fontFamily: 'monospace' }}
        />
      </label>
      <div style={{ display: 'flex', gap: 12, alignItems: 'center', marginBottom: 8 }}>
        <label style={{ fontSize: 13 }}>{tr("Temperatur")}<input
            type="number"
            value={form.temperature}
            onChange={(e) => onChange({ ...form, temperature: e.target.value })}
            min={0}
            max={2}
            step={0.1}
            style={{ width: 80, marginLeft: 8, padding: '4px 8px', borderRadius: 'var(--radius-sm)', border: '1px solid var(--border-color)', fontSize: 13 }}
          />
        </label>
        {showDefaultToggle && (
          <label style={{ fontSize: 13, display: 'flex', gap: 6, alignItems: 'center' }}>
            <input type="checkbox" checked={form.isDefault} onChange={(e) => onChange({ ...form, isDefault: e.target.checked })} />{tr("Als Standard verwenden")}</label>
        )}
      </div>
      <div style={{ display: 'flex', gap: 8 }}>
        <button type="button" className="btn-sm" onClick={onSave}>{tr("Save")}</button>
        {onCancel && <button type="button" className="btn-sm" onClick={onCancel}>{tr("Cancel")}</button>}
      </div>
    </div>
  )
}

export default function PersonalitySelector() {
  const { personalities, activeId, loading, error, loadPersonalities, upsertPersonality, deletePersonality, setActive } = usePersonalityStore()
  const availableModels = useConfigStore((s) => s.availableModels)
  const [showAdd, setShowAdd] = useState(false)
  const [form, setForm] = useState<PersonalityForm>(EMPTY_FORM)
  const [editId, setEditId] = useState<string | null>(null)
  const [editForm, setEditForm] = useState<PersonalityForm>(EMPTY_FORM)

  useEffect(() => {
    loadPersonalities()
  }, [loadPersonalities])

  const savePersonality = async (id: string, source: PersonalityForm) => {
    if (!source.name.trim() || !source.systemPrompt.trim()) return
    await upsertPersonality({
      id,
      name: source.name.trim(),
      description: source.goal,
      role: source.role,
      goal: source.goal,
      systemPrompt: source.systemPrompt,
      skillsMarkdown: source.skillsMarkdown,
      icon: source.icon || undefined,
      modelOverride: source.modelOverride || undefined,
      temperature: source.temperature ? Number(source.temperature) : undefined,
      isDefault: source.isDefault,
    })
    await loadPersonalities()
  }

  const handleAdd = async () => {
    await savePersonality(randomId(), form)
    setForm(EMPTY_FORM)
    setShowAdd(false)
  }

  const handleEdit = async () => {
    if (!editId) return
    await savePersonality(editId, editForm)
    setEditId(null)
    setEditForm(EMPTY_FORM)
  }

  const startEdit = (personality: Personality) => {
    setEditId(personality.id)
    setEditForm(toForm(personality))
  }

  return (
    <div className="panel">
      <div className="panel-heading-row">
        <h2>{tr("Manage personalities")}</h2>
        <button type="button" className="btn-sm" onClick={() => setShowAdd(!showAdd)}>{tr("New")}</button>
      </div>

      {error && <p style={{ color: 'var(--danger)', fontSize: 12 }}>{error}</p>}

      {showAdd && (
        <div className="card" style={{ marginBottom: 12 }}>
          <PersonalityEditor
            form={form}
            availableModels={availableModels}
            onChange={setForm}
            onSave={handleAdd}
            onCancel={() => setShowAdd(false)}
            showDefaultToggle
          />
        </div>
      )}

      {loading ? (
        <p className="panel-empty">{tr("Loading...")}</p>
      ) : personalities.length === 0 ? (
        <p className="panel-empty">{tr("No personalities configured. Create one to adjust the agent style.")}</p>
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
          {personalities.map((personality) => (
            <div
              key={personality.id}
              className="card"
              style={{
                border: activeId === personality.id ? '2px solid var(--accent)' : '1px solid transparent',
                cursor: 'pointer',
              }}
              onClick={() => setActive(personality.id)}
            >
              {editId === personality.id ? (
                <div onClick={(event) => event.stopPropagation()}>
                  <PersonalityEditor
                    form={editForm}
                    availableModels={availableModels}
                    onChange={setEditForm}
                    onSave={handleEdit}
                    onCancel={() => {
                      setEditId(null)
                      setEditForm(EMPTY_FORM)
                    }}
                    showDefaultToggle
                  />
                </div>
              ) : (
                <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', gap: 12 }}>
                  <div style={{ display: 'flex', gap: 8, alignItems: 'flex-start' }}>
                    <span style={{ fontSize: 18, minWidth: 28 }}>{personality.icon || 'AI'}</span>
                    <div>
                      <div style={{ fontWeight: 600, fontSize: 14 }}>
                        {personality.name}
                        {activeId === personality.id && <span style={{ fontSize: 10, color: 'var(--accent)', marginLeft: 6 }}>{tr("active")}</span>}
                        {personality.is_default && <span style={{ fontSize: 10, color: 'var(--info)', marginLeft: 6 }}>{tr("Standard")}</span>}
                      </div>
                      <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginTop: 2 }}>{personality.goal || personality.description}</div>
                      <div style={{ fontSize: 11, color: 'var(--text-muted)', marginTop: 4, display: 'flex', gap: 10 }}>
                        <span>{tr("Rolle:")}{personality.role}</span>
                        {personality.model_override && <span>{tr("Model:")}{personality.model_override}</span>}
                        {personality.temperature != null && <span>{tr("Temp:")}{personality.temperature}</span>}
                      </div>
                    </div>
                  </div>
                  <div style={{ display: 'flex', gap: 6 }}>
                    <button type="button" className="btn-sm" onClick={(event) => { event.stopPropagation(); startEdit(personality) }}>{tr("Edit")}</button>
                    <button
                      type="button"
                      onClick={(event) => { event.stopPropagation(); void deletePersonality(personality.id).then(loadPersonalities) }}
                      style={{ background: 'none', border: 'none', cursor: 'pointer', color: 'var(--danger)', fontSize: 14 }}
                    >{tr("x")}</button>
                  </div>
                </div>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
