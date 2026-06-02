import { useEffect } from 'react'
import { useCrewRuntimeStore } from '../../stores/crewRuntimeStore'

function formatTimestamp(value: string | null): string {
  if (!value) return 'nie'
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString('de-DE')
}

export default function CrewRuntimePanel() {
  const { status, loading, bootstrapping, error, loadStatus, bootstrap } = useCrewRuntimeStore()

  useEffect(() => {
    if (!status && !loading) {
      void loadStatus()
    }
  }, [loadStatus, loading, status])

  return (
    <div className="card crew-overview-card crew-runtime-panel">
      <div className="crew-overview-head">
        <div className="crew-overview-copy">
          <div className="crew-overview-kicker">Crew Runtime</div>
          <div className="crew-overview-title-row">
            <strong className="crew-overview-title">Python + CrewAI</strong>
            <span className={`crew-status-pill${status?.ready ? ' ready' : ' warning'}`}>
              {status?.ready ? 'bereit' : 'Setup erforderlich'}
            </span>
          </div>
          <div className="crew-overview-description">
            {status?.message ?? 'Die produktive Crew-Runtime wird ueber eine eingebettete Python-Umgebung mit CrewAI betrieben.'}
          </div>
        </div>
        <div className="crew-overview-actions">
          <button type="button" className="btn-sm crew-action-btn" onClick={() => void loadStatus()} disabled={loading || bootstrapping}>
            {loading ? 'Pruefe…' : 'Status laden'}
          </button>
          <button type="button" className="btn-sm crew-action-btn" onClick={() => void bootstrap(false)} disabled={loading || bootstrapping}>
            {bootstrapping ? 'Initialisiere…' : 'Runtime initialisieren'}
          </button>
          <button type="button" className="btn-sm crew-action-btn" onClick={() => void bootstrap(true)} disabled={loading || bootstrapping}>
            {bootstrapping ? 'Neuaufbau…' : 'Neu installieren'}
          </button>
        </div>
      </div>

      {error && (
        <div className="crew-inline-feedback error">{error}</div>
      )}

      {status && (
        <div className="crew-stat-grid">
          <div className="crew-stat-card">
            <div className="crew-stat-label">Python</div>
            <div className="crew-stat-value">{status.pythonVersion ?? 'unbekannt'}</div>
            <div className="crew-stat-meta crew-wrap-anywhere">{status.detectedPythonPath ?? status.embeddedPythonPath ?? 'kein Interpreter erkannt'}</div>
          </div>
          <div className="crew-stat-card">
            <div className="crew-stat-label">CrewAI</div>
            <div className="crew-stat-value">{status.crewaiInstalled ? `installiert${status.crewaiVersion ? ` (${status.crewaiVersion})` : ''}` : 'nicht installiert'}</div>
            <div className="crew-stat-meta">Letztes Bootstrap: {formatTimestamp(status.lastBootstrapAt)}</div>
          </div>
          <div className="crew-stat-card">
            <div className="crew-stat-label">Runtime Root</div>
            <div className="crew-stat-value crew-wrap-anywhere">{status.runtimeRoot}</div>
          </div>
          <div className="crew-stat-card">
            <div className="crew-stat-label">Venv</div>
            <div className="crew-stat-value crew-wrap-anywhere">{status.venvPythonPath ?? 'noch nicht erzeugt'}</div>
          </div>
        </div>
      )}
    </div>
  )
}