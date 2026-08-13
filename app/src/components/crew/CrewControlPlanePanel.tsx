import { useEffect, useState } from 'react'
import type { Crew } from '../../stores/crewStore'
import { useCrewControlPlaneStore } from '../../stores/crewControlPlaneStore'
import i18n, { tr } from '../../i18n'

type Props = {
  activeCrew: Crew
}

function formatTimestamp(value: string): string {
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString(i18n.resolvedLanguage ?? i18n.language ?? 'en')
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
          <div className="crew-overview-kicker">{tr("Control Plane")}</div>
          <strong className="crew-overview-title">{tr("Versioned crew definition")}</strong>
          <div className="crew-overview-description">{tr("The active crew is saved as a reproducible definition and can be validated against the Python crew runtime before the run.")}</div>
        </div>
        <div className="crew-overview-actions">
          <button type="button" className="btn-sm crew-action-btn" disabled={loading} onClick={() => void validateCrew(activeCrew)}>
            {loading ? tr('Loading...') : tr('Validate definition')}
          </button>
          <button type="button" className="btn-sm crew-action-btn" disabled={loading} onClick={() => void saveCrewDefinition(activeCrew, changeSummary)}>
            {loading ? tr('Saving...') : tr('Save new version')}
          </button>
        </div>
      </div>

      <input
        className="crew-toolbar-input crew-inline-input"
        placeholder={tr("Change comment for the next definition...")}
        value={changeSummary}
        onChange={(event) => setChangeSummary(event.target.value)}
      />

      {error && <div className="crew-inline-feedback error">{error}</div>}

      <div className="crew-stat-grid crew-stat-grid-compact">
        <div className="crew-stat-card">
          <div className="crew-stat-label">{tr("Active Definition")}</div>
          <div className="crew-stat-value">{activeDefinition ? `Version ${activeDefinition.versionCount}` : tr('not versioned yet')}</div>
          <div className="crew-stat-meta">
            {activeDefinition ? `Last updated: ${formatTimestamp(activeDefinition.updatedAt)}` : tr('No persisted DB state for this crew yet.')}
          </div>
        </div>
        <div className="crew-stat-card">
          <div className="crew-stat-label">{tr("Validation")}</div>
          <div className="crew-stat-value">{validation ? (validation.valid ? 'valid' : tr('with problems')) : tr('not checked yet')}</div>
          {validation && validation.issues.length > 0 && (
            <div className="crew-stat-meta crew-stat-meta-warning">
              {validation.issues.join(', ')}
            </div>
          )}
        </div>
        <div className="crew-stat-card">
          <div className="crew-stat-label">{tr("Library")}</div>
          <div className="crew-stat-value">{definitions.length}{tr("Definitionen saved")}</div>
          <div className="crew-stat-meta">{tr("The database stores versioned crew states for replay and scheduling.")}</div>
        </div>
      </div>

      <div>
        <div className="crew-stat-label crew-stat-label-spaced">{tr("Latest versions of this crew")}</div>
        {versions.length === 0 ? (
          <div className="crew-inline-feedback">{tr("No versions saved yet.")}</div>
        ) : (
          <div className="crew-stack-list">
            {versions.slice(0, 5).map((version) => (
              <div key={version.id} className="crew-stack-card">
                <div className="crew-stack-card-header">
                  <strong>{tr("Version")}{version.versionNumber}</strong>
                  <span>{formatTimestamp(version.createdAt)}</span>
                </div>
                <div className="crew-stat-meta">{version.changeSummary || tr('without comment')}</div>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}
