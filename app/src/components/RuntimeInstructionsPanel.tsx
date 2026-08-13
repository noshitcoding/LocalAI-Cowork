import { useEffect, useState } from 'react'
import { safeInvoke } from '../utils/safeInvoke'
import { tr } from '../i18n'

type RuntimeInstructionRow = {
  id: string
  scopeType: string
  scopeRef: string | null
  title: string
  content: string
  enabled: boolean
  priority: number
  updatedAt: string
}

function randomId() {
  return `ri-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}

export default function RuntimeInstructionsPanel() {
  const [items, setItems] = useState<RuntimeInstructionRow[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [form, setForm] = useState({
    title: '',
    scopeType: 'global',
    scopeRef: '',
    content: '',
    priority: '100',
  })

  const loadItems = async () => {
    setLoading(true)
    setError(null)
    try {
      const rows = await safeInvoke<RuntimeInstructionRow[] | null>('runtime_instruction_list', {
        enabledOnly: false,
      }, [])
      setItems(Array.isArray(rows) ? rows : [])
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    void loadItems()
  }, [])

  const handleSave = async () => {
    if (!form.title.trim() || !form.content.trim()) return
    setError(null)
    try {
      await safeInvoke('runtime_instruction_upsert', {
        request: {
          id: randomId(),
          title: form.title.trim(),
          scopeType: form.scopeType,
          scopeRef: form.scopeRef.trim() || null,
          content: form.content.trim(),
          priority: Number(form.priority) || 100,
          enabled: true,
        },
      }, undefined)
      setForm({ title: '', scopeType: 'global', scopeRef: '', content: '', priority: '100' })
      await loadItems()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  const handleDelete = async (id: string) => {
    try {
      await safeInvoke('runtime_instruction_delete', { id }, undefined)
      await loadItems()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  return (
    <div className="panel runtime-instructions-panel">
      <div className="panel-heading-row">
        <h2>{tr("Runtime instructions")}</h2>
        <button type="button" className="btn-sm" onClick={() => void loadItems()}>{tr("Refresh")}</button>
      </div>

      {error && <p className="runtime-instructions-error">{error}</p>}

      <div className="card runtime-instructions-form">
        <div className="grid runtime-instructions-grid">
          <label>{tr("Title")}<input value={form.title} onChange={(event) => setForm((current) => ({ ...current, title: event.target.value }))} />
          </label>
          <label>{tr("Scope")}<select value={form.scopeType} onChange={(event) => setForm((current) => ({ ...current, scopeType: event.target.value }))}>
            <option value="global">{tr("Global")}</option>
            <option value="workspace">{tr("Workspace")}</option>
            <option value="folder">{tr("Folder")}</option>
          </select>
          </label>
          <label className="runtime-instructions-full">{tr("Scope Ref")}<input
            value={form.scopeRef}
            onChange={(event) => setForm((current) => ({ ...current, scopeRef: event.target.value }))}
            placeholder={tr("Optional, e.g. C:/path/to/project")}
          />
          </label>
          <label>{tr("Priority")}<input value={form.priority} onChange={(event) => setForm((current) => ({ ...current, priority: event.target.value }))} />
          </label>
        </div>
        <label className="runtime-instructions-content">{tr("Content")}<textarea
          rows={4}
          value={form.content}
          onChange={(event) => setForm((current) => ({ ...current, content: event.target.value }))}
        />
        </label>
        <button type="button" className="btn-sm" onClick={handleSave}>{tr("Save")}</button>
      </div>

      {loading ? (
        <p className="panel-empty">{tr("Loading...")}</p>
      ) : items.length === 0 ? (
        <p className="panel-empty">{tr("No runtime instructions available.")}</p>
      ) : (
        <div className="runtime-instructions-list">
          {items.map((item) => (
            <div key={item.id} className="card runtime-instructions-card">
              <div className="runtime-instructions-card-header">
                <div className="runtime-instructions-card-main">
                  <strong className="runtime-instructions-title">{item.title}</strong>
                  <div className="runtime-instructions-meta">
                    <span>{item.scopeType}</span>
                    {item.scopeRef && <span>{item.scopeRef}</span>}
                    <span>{tr("Priority")}: {item.priority}</span>
                  </div>
                </div>
                <button type="button" className="btn-sm" onClick={() => void handleDelete(item.id)}>{tr("Delete")}</button>
              </div>
              <pre className="runtime-instructions-content-preview">
                {item.content}
              </pre>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
