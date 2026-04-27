import { useEffect, useState } from 'react'
import { useEngineStore } from '../stores/engineStore'
import { safeInvoke } from '../utils/safeInvoke'

type EngineRunRow = {
  id: string
  parentRunId: string | null
  sessionId: string | null
  title: string
  inputSummary: string | null
  status: string
  phase: string
  cwd: string | null
  model: string | null
  provider: string | null
  retryCount: number
  resumedFromRunId: string | null
  checkpointJson: string | null
  resultSummary: string | null
  error: string | null
  updatedAt: string
  createdAt: string
}

type EngineRunCheckpointRow = {
  id: string
  runId: string
  label: string
  snapshotJson: string
  createdAt: string
}

type WorkerSandboxRow = {
  id: string
  runId: string
  backendId: string | null
  status: string
  mode: string
  sourceCwd: string
  workspaceRoot: string
  allowFileRead: boolean
  allowFileWrite: boolean
  allowShellExecution: boolean
  allowWebFetch: boolean
  allowWebSearch: boolean
  allowMcp: boolean
}

export default function RunPanel() {
  const currentRunId = useEngineStore((state) => state.currentRunId)
  const [runs, setRuns] = useState<EngineRunRow[]>([])
  const [selectedRun, setSelectedRun] = useState<string | null>(null)
  const [checkpoints, setCheckpoints] = useState<EngineRunCheckpointRow[]>([])
  const [sandbox, setSandbox] = useState<WorkerSandboxRow | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [busyRunId, setBusyRunId] = useState<string | null>(null)

  const refreshRuns = async () => {
    setLoading(true)
    setError(null)
    try {
      const rows = await safeInvoke<EngineRunRow[] | null>('engine_run_list', { limit: 100 }, [])
      const safeRows = Array.isArray(rows) ? rows : []
      setRuns(safeRows)
      const nextSelected = selectedRun && safeRows.some((row) => row.id === selectedRun)
        ? selectedRun
        : safeRows[0]?.id ?? null
      setSelectedRun(nextSelected)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }

  const refreshCheckpoints = async (runId: string) => {
    try {
      const rows = await safeInvoke<EngineRunCheckpointRow[] | null>('engine_run_checkpoint_list', {
        runId,
        limit: 20,
      }, [])
      setCheckpoints(Array.isArray(rows) ? rows : [])
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  const refreshSandbox = async (runId: string) => {
    try {
      const row = await safeInvoke<WorkerSandboxRow | null>('worker_sandbox_get_for_run', { runId }, null)
      setSandbox(row ?? null)
    } catch {
      setSandbox(null)
    }
  }

  useEffect(() => {
    void refreshRuns()
  }, [])

  useEffect(() => {
    if (!selectedRun) {
      setCheckpoints([])
      setSandbox(null)
      return
    }
    void refreshCheckpoints(selectedRun)
    void refreshSandbox(selectedRun)
  }, [selectedRun])

  const handleAction = async (runId: string, action: 'cancel' | 'resume' | 'retry') => {
    setBusyRunId(runId)
    setError(null)
    try {
      let nextSelectedRun = runId
      if (action === 'cancel') {
        await safeInvoke('engine_run_cancel', { id: runId }, null)
      } else if (action === 'resume') {
        await safeInvoke('engine_run_resume', { id: runId }, null)
      } else {
        nextSelectedRun = await safeInvoke<string>('engine_run_retry', { id: runId })
      }
      await refreshRuns()
      setSelectedRun(nextSelectedRun)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusyRunId(null)
    }
  }

  return (
    <div className="panel">
      <div className="panel-heading-row">
        <h2>Runs</h2>
        <button type="button" className="btn-sm" onClick={() => void refreshRuns()}>
          Aktualisieren
        </button>
      </div>

      {error && <p style={{ color: 'var(--danger)', fontSize: 12 }}>{error}</p>}

      <div style={{ fontSize: 12, color: 'var(--text-muted)', marginBottom: 12 }}>
        Aktiver Run: {currentRunId ?? 'keiner'}
      </div>

      {loading ? (
        <p className="panel-empty">Laden...</p>
      ) : runs.length === 0 ? (
        <p className="panel-empty">Noch keine Runs gespeichert.</p>
      ) : (
        <div style={{ display: 'grid', gridTemplateColumns: '1.3fr 1fr', gap: 12 }}>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 8, maxHeight: 420, overflowY: 'auto' }}>
            {runs.map((run) => (
              <button
                key={run.id}
                type="button"
                className="card"
                onClick={() => setSelectedRun(run.id)}
                style={{
                  textAlign: 'left',
                  border: selectedRun === run.id ? '1px solid var(--accent)' : '1px solid var(--border-color)',
                  background: 'var(--bg-secondary)',
                }}
              >
                <div style={{ display: 'flex', justifyContent: 'space-between', gap: 8 }}>
                  <strong style={{ fontSize: 13 }}>{run.title}</strong>
                  <span style={{ fontSize: 11, color: run.status === 'completed' ? 'var(--success)' : run.status === 'failed' ? 'var(--danger)' : 'var(--text-muted)' }}>
                    {run.status}
                  </span>
                </div>
                <div style={{ fontSize: 11, color: 'var(--text-muted)', marginTop: 4 }}>
                  {new Date(run.updatedAt).toLocaleString('de-DE')} • {run.phase}
                </div>
                {run.inputSummary && (
                  <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginTop: 6 }}>
                    {run.inputSummary.slice(0, 180)}
                  </div>
                )}
              </button>
            ))}
          </div>

          <div className="card" style={{ minHeight: 320 }}>
            {selectedRun ? (
              (() => {
                const run = runs.find((item) => item.id === selectedRun)
                if (!run) return <p className="panel-empty">Run nicht gefunden.</p>
                return (
                  <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
                    <div>
                      <strong>{run.title}</strong>
                      <div style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 4 }}>
                        {run.id}
                      </div>
                    </div>
                    <div style={{ fontSize: 12 }}>
                      <div>Status: {run.status}</div>
                      <div>Phase: {run.phase}</div>
                      <div>Provider: {run.provider ?? 'n/a'}</div>
                      <div>Modell: {run.model ?? 'n/a'}</div>
                      <div>Retry Count: {run.retryCount}</div>
                      {run.parentRunId && <div>Parent: {run.parentRunId}</div>}
                    </div>
                    {sandbox && (
                      <div>
                        <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 4 }}>Sandbox</div>
                        <div style={{ fontSize: 11, color: 'var(--text-secondary)' }}>
                          <div>Status: {sandbox.status}</div>
                          <div>Mode: {sandbox.mode}</div>
                          <div>Backend: {sandbox.backendId ?? 'local'}</div>
                          <div>Workspace: {sandbox.workspaceRoot}</div>
                          <div>Source: {sandbox.sourceCwd}</div>
                          <div>
                            Rechte: read {String(sandbox.allowFileRead)} | write {String(sandbox.allowFileWrite)} | shell {String(sandbox.allowShellExecution)} | web {String(sandbox.allowWebFetch || sandbox.allowWebSearch)} | mcp {String(sandbox.allowMcp)}
                          </div>
                        </div>
                      </div>
                    )}
                    <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
                      <button type="button" className="btn-sm" disabled={busyRunId === run.id || run.status === 'completed'} onClick={() => void handleAction(run.id, 'resume')}>
                        Resume
                      </button>
                      <button type="button" className="btn-sm" disabled={busyRunId === run.id} onClick={() => void handleAction(run.id, 'retry')}>
                        Retry
                      </button>
                      <button type="button" className="btn-sm" disabled={busyRunId === run.id || run.status === 'completed' || run.status === 'failed' || run.status === 'canceled'} onClick={() => void handleAction(run.id, 'cancel')}>
                        Cancel
                      </button>
                    </div>
                    {run.resultSummary && (
                      <div>
                        <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 4 }}>Ergebnis</div>
                        <pre style={{ fontSize: 11, whiteSpace: 'pre-wrap', background: 'var(--bg-primary)', padding: 8, borderRadius: 'var(--radius-sm)', maxHeight: 140, overflowY: 'auto' }}>
                          {run.resultSummary}
                        </pre>
                      </div>
                    )}
                    {run.error && (
                      <div>
                        <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 4, color: 'var(--danger)' }}>Fehler</div>
                        <pre style={{ fontSize: 11, whiteSpace: 'pre-wrap', background: 'var(--bg-primary)', padding: 8, borderRadius: 'var(--radius-sm)', maxHeight: 120, overflowY: 'auto', color: 'var(--danger)' }}>
                          {run.error}
                        </pre>
                      </div>
                    )}
                    <div>
                      <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 4 }}>Checkpoints</div>
                      {checkpoints.length === 0 ? (
                        <div style={{ fontSize: 11, color: 'var(--text-muted)' }}>Keine Checkpoints</div>
                      ) : (
                        <div style={{ display: 'flex', flexDirection: 'column', gap: 6, maxHeight: 140, overflowY: 'auto' }}>
                          {checkpoints.map((checkpoint) => (
                            <div key={checkpoint.id} style={{ fontSize: 11, background: 'var(--bg-primary)', padding: 8, borderRadius: 'var(--radius-sm)' }}>
                              <div style={{ fontWeight: 600 }}>{checkpoint.label}</div>
                              <div style={{ color: 'var(--text-muted)', margin: '3px 0 6px' }}>{new Date(checkpoint.createdAt).toLocaleString('de-DE')}</div>
                              <pre style={{ whiteSpace: 'pre-wrap', margin: 0 }}>{checkpoint.snapshotJson.slice(0, 400)}</pre>
                            </div>
                          ))}
                        </div>
                      )}
                    </div>
                  </div>
                )
              })()
            ) : (
              <p className="panel-empty">Run auswaehlen.</p>
            )}
          </div>
        </div>
      )}
    </div>
  )
}
