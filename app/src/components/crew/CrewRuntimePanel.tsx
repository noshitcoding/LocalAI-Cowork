import { useEffect } from 'react'
import { useCrewRuntimeStore } from '../../stores/crewRuntimeStore'
import { hasTauriRuntime } from '../../utils/safeInvoke'
import i18n, { tr } from '../../i18n'

function formatTimestamp(value: string | null): string {
  if (!value) return '-'
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString(i18n.resolvedLanguage ?? i18n.language ?? 'en')
}

export default function CrewRuntimePanel() {
  const { status, loading, bootstrapping, error, loadStatus, bootstrap } = useCrewRuntimeStore()

  useEffect(() => {
    if (hasTauriRuntime() && !status && !loading) {
      void loadStatus()
    }
  }, [loadStatus, loading, status])

  return (
    <div className="card crew-overview-card crew-runtime-panel">
      <div className="crew-overview-head">
        <div className="crew-overview-copy">
          <div className="crew-overview-kicker">{tr("Crew Runtime")}</div>
          <div className="crew-overview-title-row">
            <strong className="crew-overview-title">{tr("Python + CrewAI")}</strong>
            <span className={`crew-status-pill${status?.ready ? ' ready' : ' warning'}`}>
              {status?.ready ? tr('Ready') : tr('Setup required')}
            </span>
          </div>
          <div className="crew-overview-description">
            {status?.message ?? tr('The production crew runtime runs through an embedded Python environment with CrewAI.')}
          </div>
        </div>
        <div className="crew-overview-actions">
          <button type="button" className="btn-sm crew-action-btn" onClick={() => void loadStatus()} disabled={loading || bootstrapping}>
            {loading ? tr('Loading...') : tr('Load status')}
          </button>
          <button type="button" className="btn-sm crew-action-btn" onClick={() => void bootstrap(false)} disabled={loading || bootstrapping}>
            {bootstrapping ? tr('Initializing...') : tr('Initialize runtime')}
          </button>
          <button type="button" className="btn-sm crew-action-btn" onClick={() => void bootstrap(true)} disabled={loading || bootstrapping}>
            {bootstrapping ? tr('Rebuilding...') : tr('Reinstall')}
          </button>
        </div>
      </div>

      {error && (
        <div className="crew-inline-feedback error">{error}</div>
      )}

      {status && (
        <div className="crew-stat-grid">
          <div className="crew-stat-card">
            <div className="crew-stat-label">{tr("Python")}</div>
            <div className="crew-stat-value">{status.pythonVersion ?? tr('Unknown')}</div>
            <div className="crew-stat-meta crew-wrap-anywhere">{status.detectedPythonPath ?? status.embeddedPythonPath ?? tr('No interpreter detected')}</div>
          </div>
          <div className="crew-stat-card">
            <div className="crew-stat-label">{tr("CrewAI")}</div>
            <div className="crew-stat-value">{status.crewaiInstalled ? `${tr('Installed')}${status.crewaiVersion ? ` (${status.crewaiVersion})` : ''}` : tr('Not installed')}</div>
            <div className="crew-stat-meta">
              {status.runtimeCompatible
                ? tr('Runtime dependencies verified')
                : `${tr('Required CrewAI version')}: ${status.expectedCrewaiVersion ?? '-'}`}
            </div>
            <div className="crew-stat-meta">{tr("Letztes Bootstrap:")}{formatTimestamp(status.lastBootstrapAt)}</div>
          </div>
          <div className="crew-stat-card">
            <div className="crew-stat-label">{tr("Runtime Root")}</div>
            <div className="crew-stat-value crew-wrap-anywhere">{status.runtimeRoot}</div>
          </div>
          <div className="crew-stat-card">
            <div className="crew-stat-label">{tr("Venv")}</div>
            <div className="crew-stat-value crew-wrap-anywhere">{status.venvPythonPath ?? tr('Not created yet')}</div>
          </div>
        </div>
      )}
    </div>
  )
}
