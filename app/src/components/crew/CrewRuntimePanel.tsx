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
    <div className="card" style={{ marginBottom: 16, display: 'grid', gap: 12 }}>
      <div style={{ display: 'flex', justifyContent: 'space-between', gap: 12, alignItems: 'flex-start', flexWrap: 'wrap' }}>
        <div>
          <div style={{ fontSize: 12, textTransform: 'uppercase', letterSpacing: '0.08em', color: 'var(--text-muted)' }}>Crew Runtime</div>
          <div style={{ display: 'flex', gap: 8, alignItems: 'center', marginTop: 6, flexWrap: 'wrap' }}>
            <strong style={{ fontSize: 16 }}>Python + CrewAI</strong>
            <span style={{ fontSize: 12, padding: '4px 8px', borderRadius: 999, background: status?.ready ? 'rgba(56, 142, 60, 0.16)' : 'rgba(191, 54, 12, 0.14)', color: status?.ready ? 'var(--success)' : 'var(--warning)' }}>
              {status?.ready ? 'bereit' : 'Setup erforderlich'}
            </span>
          </div>
          <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginTop: 6, maxWidth: 860 }}>
            {status?.message ?? 'Die produktive Crew-Runtime wird ueber eine eingebettete Python-Umgebung mit CrewAI betrieben.'}
          </div>
        </div>
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
          <button type="button" className="btn-sm" onClick={() => void loadStatus()} disabled={loading || bootstrapping}>
            {loading ? 'Pruefe…' : 'Status laden'}
          </button>
          <button type="button" className="btn-sm" onClick={() => void bootstrap(false)} disabled={loading || bootstrapping}>
            {bootstrapping ? 'Initialisiere…' : 'Runtime initialisieren'}
          </button>
          <button type="button" className="btn-sm" onClick={() => void bootstrap(true)} disabled={loading || bootstrapping}>
            {bootstrapping ? 'Neuaufbau…' : 'Neu installieren'}
          </button>
        </div>
      </div>

      {error && (
        <div style={{ fontSize: 12, color: 'var(--danger)' }}>{error}</div>
      )}

      {status && (
        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(220px, 1fr))', gap: 12 }}>
          <div>
            <div style={{ fontSize: 12, color: 'var(--text-muted)' }}>Python</div>
            <div style={{ fontSize: 13 }}>{status.pythonVersion ?? 'unbekannt'}</div>
            <div style={{ fontSize: 11, color: 'var(--text-secondary)', marginTop: 4 }}>{status.detectedPythonPath ?? status.embeddedPythonPath ?? 'kein Interpreter erkannt'}</div>
          </div>
          <div>
            <div style={{ fontSize: 12, color: 'var(--text-muted)' }}>CrewAI</div>
            <div style={{ fontSize: 13 }}>{status.crewaiInstalled ? `installiert${status.crewaiVersion ? ` (${status.crewaiVersion})` : ''}` : 'nicht installiert'}</div>
            <div style={{ fontSize: 11, color: 'var(--text-secondary)', marginTop: 4 }}>Letztes Bootstrap: {formatTimestamp(status.lastBootstrapAt)}</div>
          </div>
          <div>
            <div style={{ fontSize: 12, color: 'var(--text-muted)' }}>Runtime Root</div>
            <div style={{ fontSize: 11, color: 'var(--text-secondary)', wordBreak: 'break-word' }}>{status.runtimeRoot}</div>
          </div>
          <div>
            <div style={{ fontSize: 12, color: 'var(--text-muted)' }}>Venv</div>
            <div style={{ fontSize: 11, color: 'var(--text-secondary)', wordBreak: 'break-word' }}>{status.venvPythonPath ?? 'noch nicht erzeugt'}</div>
          </div>
        </div>
      )}
    </div>
  )
}