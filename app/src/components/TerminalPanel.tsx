import { useEffect, useState } from 'react'
import { Play, Trash2 } from 'lucide-react'
import { useConfigStore } from '../stores/configStore'
import { useTerminalStore, type TerminalBackend } from '../stores/terminalStore'
import { tr } from '../i18n'

function randomId() {
  return `be-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}

export default function TerminalPanel() {
  const { backends, loading, error, loadBackends, upsertBackend, deleteBackend, execCommand, ensureLocalBackend } = useTerminalStore()
  const terminalPersistenceMode = useConfigStore((state) => state.preferences.terminalPersistenceMode)
  const setPreference = useConfigStore((state) => state.setPreference)
  const [showAdd, setShowAdd] = useState(false)
  const [form, setForm] = useState({ name: '', type: 'local', config: '{}' })
  const [execBackendId, setExecBackendId] = useState('')
  const [execCmd, setExecCmd] = useState('')
  const [execResult, setExecResult] = useState<{ stdout: string; stderr: string; exitCode: number | null } | null>(null)
  const [execLoading, setExecLoading] = useState(false)

  useEffect(() => {
    void loadBackends()
  }, [loadBackends])

  const handleAdd = async () => {
    if (!form.name.trim()) return
    const saved = await upsertBackend({ id: randomId(), name: form.name.trim(), backendType: form.type, configJson: form.config })
    if (!saved) return
    setForm({ name: '', type: 'local', config: '{}' })
    setShowAdd(false)
    void loadBackends()
  }

  const handleExec = async () => {
    if (!execBackendId || !execCmd.trim()) return
    setExecLoading(true)
    try {
      const result = await execCommand(execBackendId, execCmd.trim())
      setExecResult(result as { stdout: string; stderr: string; exitCode: number | null })
    } catch (e) {
      setExecResult({ stdout: '', stderr: String(e), exitCode: -1 })
    } finally {
      setExecLoading(false)
    }
  }

  const handleEnsureLocal = async () => {
    await ensureLocalBackend()
    void loadBackends()
  }

  const handleDeleteBackend = async (backendId: string) => {
    await deleteBackend(backendId)
    void loadBackends()
  }

  const selectedBackend = backends.find((backend) => backend.id === execBackendId)

  return (
    <div className="panel terminal-panel">
      <div className="panel-heading-row">
        <h2>{tr("Terminal backends")}</h2>
        <div className="terminal-panel-actions">
          <button type="button" className="btn-sm" onClick={handleEnsureLocal}>{tr("Ensure local")}</button>
          <button type="button" className="btn-sm" onClick={() => setShowAdd(!showAdd)}>{tr("New")}</button>
        </div>
      </div>

      {error && <p className="terminal-panel-error">{error}</p>}

      <div className="card terminal-panel-card">
        <h3 className="terminal-panel-title">{tr("Terminal-Dock")}</h3>
        <label className="terminal-panel-label">
          {tr("Persistence")}
          <select
            value={terminalPersistenceMode}
            onChange={(event) => setPreference('terminalPersistenceMode', event.currentTarget.value as typeof terminalPersistenceMode)}
          >
            <option value="runtime">{tr("Nur Laufzeit")}</option>
            <option value="scrollback">{tr("Save scrollback")}</option>
            <option value="restore-tabs">{tr("Tabs wieder oeffnen")}</option>
          </select>
        </label>
      </div>

      {showAdd && (
        <div className="card terminal-panel-card">
          <div className="grid terminal-add-grid">
            <label>{tr("Name")}<input type="text" value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} />
            </label>
            <label>{tr("Type")}<select value={form.type} onChange={(e) => setForm({ ...form, type: e.target.value })}>
              <option value="local">{tr("Local")}</option>
              <option value="container">{tr("Container")}</option>
              <option value="ssh">{tr("SSH")}</option>
              <option value="hpc">{tr("HPC")}</option>
              <option value="serverless">{tr("Serverless")}</option>
            </select>
            </label>
            <label>{tr("Protected config (JSON)")}<input type="text" value={form.config} onChange={(e) => setForm({ ...form, config: e.target.value })} />
            </label>
          </div>
          <button type="button" className="btn-sm" onClick={handleAdd}>{tr("Create backend")}</button>
        </div>
      )}

      {loading ? (
        <p className="panel-empty">{tr("Loading...")}</p>
      ) : backends.length === 0 ? (
        <p className="panel-empty">{tr("No Backends configured")}</p>
      ) : (
        <div className="terminal-backend-list">
          {backends.map((backend: TerminalBackend) => (
            <div key={backend.id} className="card terminal-backend-card">
              <div className="terminal-backend-info">
                <strong className="terminal-backend-name">{backend.name}</strong>
                <span className="terminal-backend-type">({backend.backend_type})</span>
              </div>
              <div className="terminal-backend-actions">
                <button type="button" className="btn-sm" onClick={() => setExecBackendId(backend.id)} title={tr("Execute command")} aria-label={tr("Execute command")}>
                  <Play size={14} aria-hidden="true" />
                </button>
                <button type="button" className="terminal-danger-icon" onClick={() => void handleDeleteBackend(backend.id)} title={tr("Delete backend")} aria-label={tr("Delete backend")}>
                  <Trash2 size={14} aria-hidden="true" />
                </button>
              </div>
            </div>
          ))}
        </div>
      )}

      {execBackendId && (
        <div className="card terminal-panel-card">
          <h3 className="terminal-panel-title">{tr("Command execute (")}{selectedBackend?.name})</h3>
          <div className="terminal-exec-row">
            <input
              type="text"
              className="terminal-command-input"
              value={execCmd}
              onChange={(e) => setExecCmd(e.target.value)}
              onKeyDown={(e) => { if (e.key === 'Enter') void handleExec() }}
              placeholder={tr("Enter command...")}
            />
            <button type="button" className="btn-sm" onClick={handleExec} disabled={execLoading}>
              {execLoading ? tr("Loading...") : tr("Execute")}
            </button>
            <button type="button" className="btn-sm" onClick={() => { setExecBackendId(''); setExecResult(null) }}>{tr("Close")}</button>
          </div>
          {execResult && (
            <div className="terminal-exec-result">
              {execResult.stdout && (
                <pre className="terminal-output">
                  {execResult.stdout}
                </pre>
              )}
              {execResult.stderr && (
                <pre className="terminal-output terminal-output-error">
                  {execResult.stderr}
                </pre>
              )}
              <div className={`terminal-exit-code ${execResult.exitCode === 0 ? 'success' : 'error'}`}>
                {tr("Exit-Code:")}{execResult.exitCode ?? 'N/A'}
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  )
}
