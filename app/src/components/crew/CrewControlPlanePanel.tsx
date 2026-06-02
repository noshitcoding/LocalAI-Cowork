import { useEffect, useState } from 'react'
import type { Crew } from '../../stores/crewStore'
import { useCrewControlPlaneStore } from '../../stores/crewControlPlaneStore'

type Props = {
  activeCrew: Crew
}

function formatTimestamp(value: string): string {
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString('de-DE')
}

export default function CrewControlPlanePanel({ activeCrew }: Props) {
  const {
    definitions,
    versions,
    validation,
    loading,
    error,
    loadDefinitions,
    loadVersions,
    saveCrewDefinition,
    validateCrew,
  } = useCrewControlPlaneStore()
  const [changeSummary, setChangeSummary] = useState('')

  useEffect(() => {
    void loadDefinitions()
  }, [loadDefinitions])

  useEffect(() => {
    void loadVersions(activeCrew.id)
  }, [activeCrew.id, loadVersions])

  const activeDefinition = definitions.find((entry) => entry.id === activeCrew.id) ?? null

  return (
    <div className="card crew-overview-card">
      <div className="crew-overview-head">
        <div className="crew-overview-copy">
          <div className="crew-overview-kicker">Control Plane</div>
          <strong className="crew-overview-title">Versionierte Crew-Definition</strong>
          <div className="crew-overview-description">
            Die aktive Crew wird als reproduzierbare Definition gespeichert und kann vor dem Lauf gegen die Python-Crew-Runtime validiert werden.
          </div>
        </div>
        <div className="crew-overview-actions">
          <button type="button" className="btn-sm crew-action-btn" disabled={loading} onClick={() => void validateCrew(activeCrew)}>
            {loading ? 'Pruefe…' : 'Definition validieren'}
          </button>
          <button type="button" className="btn-sm crew-action-btn" disabled={loading} onClick={() => void saveCrewDefinition(activeCrew, changeSummary)}>
            {loading ? 'Speichere…' : 'Neue Version speichern'}
          </button>
        </div>
      </div>

      <input
        className="crew-toolbar-input crew-inline-input"
        placeholder="Aenderungskommentar fuer die naechste Definition…"
        value={changeSummary}
        onChange={(event) => setChangeSummary(event.target.value)}
      />

      {error && <div className="crew-inline-feedback error">{error}</div>}

      <div className="crew-stat-grid crew-stat-grid-compact">
        <div className="crew-stat-card">
          <div className="crew-stat-label">Aktive Definition</div>
          <div className="crew-stat-value">{activeDefinition ? `Version ${activeDefinition.versionCount}` : 'noch nicht versioniert'}</div>
          <div className="crew-stat-meta">
            {activeDefinition ? `Zuletzt aktualisiert: ${formatTimestamp(activeDefinition.updatedAt)}` : 'Noch kein persistierter DB-Stand fuer diese Crew.'}
          </div>
        </div>
        <div className="crew-stat-card">
          <div className="crew-stat-label">Validation</div>
          <div className="crew-stat-value">{validation ? (validation.valid ? 'gueltig' : 'mit Problemen') : 'noch nicht geprueft'}</div>
          {validation && validation.issues.length > 0 && (
            <div className="crew-stat-meta" style={{ color: 'var(--warning)' }}>
              {validation.issues.join(' • ')}
            </div>
          )}
        </div>
        <div className="crew-stat-card">
          <div className="crew-stat-label">Bibliothek</div>
          <div className="crew-stat-value">{definitions.length} Definitionen gespeichert</div>
          <div className="crew-stat-meta">Die DB haelt versionierte Crew-Staende fuer Replay und Scheduling fest.</div>
        </div>
      </div>

      <div>
        <div className="crew-stat-label" style={{ marginBottom: 8 }}>Neueste Versionen dieser Crew</div>
        {versions.length === 0 ? (
          <div className="crew-inline-feedback">Noch keine Versionen gespeichert.</div>
        ) : (
          <div className="crew-stack-list">
            {versions.slice(0, 5).map((version) => (
              <div key={version.id} className="crew-stack-card">
                <div className="crew-stack-card-header">
                  <strong>Version {version.versionNumber}</strong>
                  <span>{formatTimestamp(version.createdAt)}</span>
                </div>
                <div className="crew-stat-meta">{version.changeSummary || 'ohne Kommentar'}</div>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}