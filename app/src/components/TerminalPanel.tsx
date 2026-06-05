import { useEffect, useState } from 'react'
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
    loadBackends()
  }, [loadBackends])

  const handleAdd = async () => {
    if (!form.name.trim()) return
    await upsertBackend({ id: randomId(), name: form.name.trim(), backendType: form.type, configJson: form.config })
    setForm({ name: '', type: 'local', config: '{}' })
    setShowAdd(false)
    loadBackends()
  }

  const handleExec = async () => {
    if (!execBackendId || !execCmd.trim()) return
    setExecLoading(true)
    try {
      const result = await execCommand(execBackendId, execCmd.trim())
      setExecResult(result as { stdout: string; stderr: string; exitCode: number | null })
    } catch (e) {
      setExecResult({ stdout: '', stderr: String(e), exitCode: -1 })
    }
    setExecLoading(false)
  }

  const handleEnsureLocal = async () => {
    await ensureLocalBackend()
    loadBackends()
  }

  return (
    <div className="panel">
      <div className="panel-heading-row">
        <h2>{tr("🖥️ Terminal backends")}</h2>
        <div style={{ display: 'flex', gap: 8 }}>
          <button type="button" className="btn-sm" onClick={handleEnsureLocal}>{tr("Ensure local")}</button>
          <button type="button" className="btn-sm" onClick={() => setShowAdd(!showAdd)}>{tr("New")}</button>
        </div>
      </div>

      {error && <p style={{ color: 'var(--danger)', fontSize: 12 }}>{error}</p>}

      <div className="card" style={{ marginBottom: 12 }}>
        <h3 style={{ fontSize: 13, fontWeight: 600, marginBottom: 8 }}>{tr("Terminal-Dock")}</h3>
        <label style={{ display: 'flex', flexDirection: 'column', gap: 6, fontSize: 12 }}>
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
        <div className="card" style={{ marginBottom: 12 }}>
          <div className="grid" style={{ gridTemplateColumns: '1fr 1fr 2fr', marginBottom: 8 }}>
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
            <label>{tr("Config (JSON)")}<input type="text" value={form.config} onChange={(e) => setForm({ ...form, config: e.target.value })} />
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
        <div style={{ display: 'flex', flexDirection: 'column', gap: 6, marginBottom: 16 }}>
          {backends.map((b: TerminalBackend) => (
            <div key={b.id} className="card" style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
              <div>
                <strong style={{ fontSize: 13 }}>{b.name}</strong>
                <span style={{ fontSize: 11, color: 'var(--text-muted)', marginLeft: 8 }}>({b.backend_type})</span>
              </div>
              <div style={{ display: 'flex', gap: 6 }}>
                <button type="button" className="btn-sm" onClick={() => setExecBackendId(b.id)} title={tr("Command execute")}>▶</button>
                <button type="button" onClick={() => { deleteBackend(b.id); loadBackends() }}
                  style={{ background: 'none', border: 'none', cursor: 'pointer', color: 'var(--danger)', fontSize: 14 }}>{tr("×")}</button>
              </div>
            </div>
          ))}
        </div>
      )}

      {/* Command execution */}
      {execBackendId && (
        <div className="card" style={{ marginBottom: 12 }}>
          <h3 style={{ fontSize: 13, fontWeight: 600, marginBottom: 8 }}>{tr("Command execute (")}{backends.find((b) => b.id === execBackendId)?.name})</h3>
          <div style={{ display: 'flex', gap: 8, marginBottom: 8 }}>
            <input type="text" value={execCmd} onChange={(e) => setExecCmd(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && handleExec()}
              placeholder={tr("Enter command...")}
              style={{ flex: 1, padding: '6px 10px', borderRadius: 'var(--radius-sm)', border: '1px solid var(--border-color)', fontSize: 13, fontFamily: 'monospace' }} />
            <button type="button" className="btn-sm" onClick={handleExec} disabled={execLoading}>
              {execLoading ? '...' : tr('Execute')}
            </button>
            <button type="button" className="btn-sm" onClick={() => { setExecBackendId(''); setExecResult(null) }}>{tr("Close")}</button>
          </div>
          {execResult && (
            <div>
              {execResult.stdout && (
                <pre style={{ fontSize: 11, background: 'var(--bg-primary)', padding: 8, borderRadius: 'var(--radius-sm)', overflowX: 'auto', maxHeight: 200 }}>
                  {execResult.stdout}
                </pre>
              )}
              {execResult.stderr && (
                <pre style={{ fontSize: 11, background: 'var(--bg-primary)', padding: 8, borderRadius: 'var(--radius-sm)', overflowX: 'auto', maxHeight: 100, color: 'var(--danger)' }}>
                  {execResult.stderr}
                </pre>
              )}
              <div style={{ fontSize: 11, color: execResult.exitCode === 0 ? 'var(--success)' : 'var(--danger)', marginTop: 4 }}>{tr("Exit-Code:")}{execResult.exitCode ?? 'N/A'}
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  )
}
