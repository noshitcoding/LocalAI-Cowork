import { useEffect, useMemo, useState } from 'react'
import { useCrewControlPlaneStore } from '../../stores/crewControlPlaneStore'
import { useCrewStore, type CrewGovernanceMode } from '../../stores/crewStore'

const GOVERNANCE_MODES: Array<{
  value: CrewGovernanceMode
  title: string
  description: string
}> = [
  {
    value: 'allow-all',
    title: 'Alles erlauben',
    description: 'Crew startet ohne Rueckfrage. Bereits vorhandene manuelle Freigaben bleiben trotzdem wirksam.',
  },
  {
    value: 'ask-risky',
    title: 'Nur bei riskanten Aktionen fragen',
    description: 'Rueckfrage nur bei riskanten Tools, MCP-Zugriffen oder Delegation.',
  },
  {
    value: 'ask-all',
    title: 'Immer vor Aktionen fragen',
    description: 'Jeder Crew-Start wird zuerst softwareseitig pausiert und erst nach Freigabe fortgesetzt.',
  },
  {
    value: 'read-only',
    title: 'Nur lesen',
    description: 'Erlaubt nur Lesezugriffe wie read_file, grep, glob, web_fetch und web_search.',
  },
]

type Props = {
  activeCrewId: string
}

function formatTimestamp(value: string | null): string {
  if (!value) return '—'
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString('de-DE')
}

function formatApprovalType(value: string): string {
  if (value === 'run_gate') return 'Startfreigabe'
  if (value === 'tool_gate') return 'Tool-Freigabe'
  if (value === 'delegation_gate') return 'Delegations-Freigabe'
  return value
}

function formatApprovalStatus(value: string): string {
  if (value === 'approved') return 'genehmigt'
  if (value === 'rejected') return 'abgelehnt'
  if (value === 'pending') return 'offen'
  return value
}

export default function CrewGovernancePanel({ activeCrewId }: Props) {
  const crew = useCrewStore((state) => state.crews.find((entry) => entry.id === activeCrewId) ?? null)
  const updateCrew = useCrewStore((state) => state.updateCrew)
  const {
    approvals,
    loading,
    error,
    loadApprovals,
    resolveApproval,
  } = useCrewControlPlaneStore()
  const [notice, setNotice] = useState<{ crewId: string; message: string } | null>(null)

  useEffect(() => {
    void loadApprovals(undefined, activeCrewId)
  }, [activeCrewId, loadApprovals])

  const activeMode = crew?.governanceMode ?? 'allow-all'
  const activeModeDefinition = useMemo(
    () => GOVERNANCE_MODES.find((entry) => entry.value === activeMode) ?? GOVERNANCE_MODES[0],
    [activeMode],
  )

  const handleModeSelect = (mode: CrewGovernanceMode) => {
    updateCrew(activeCrewId, { governanceMode: mode })
    const definition = GOVERNANCE_MODES.find((entry) => entry.value === mode)
    setNotice(definition ? { crewId: activeCrewId, message: `Governance-Modus gesetzt: ${definition.title}.` } : null)
  }

  const handleResolveApproval = async (approvalId: string, status: 'approved' | 'rejected') => {
    await resolveApproval({
      id: approvalId,
      crewId: activeCrewId,
      status,
      resolvedBy: 'manual-panel',
      resolutionNote: status === 'approved' ? 'Manuell genehmigt' : 'Manuell abgelehnt',
    })

    if (!useCrewControlPlaneStore.getState().error) {
      setNotice(
        {
          crewId: activeCrewId,
          message: status === 'approved'
            ? 'Freigabe erteilt. Falls ein Crew-Run pausiert war, wird er jetzt im Hintergrund fortgesetzt.'
            : 'Freigabe abgelehnt.',
        },
      )
    }
  }

  return (
    <div className="card crew-overview-card">
      <div className="crew-overview-copy">
        <div className="crew-overview-kicker">Governance</div>
        <strong className="crew-overview-title">Freigaben & Schutzmodus</strong>
      </div>

      {error && <div className="crew-inline-feedback error">{error}</div>}
      {notice?.crewId === activeCrewId && <div className="crew-inline-feedback">{notice.message}</div>}

      <div className="crew-stat-card crew-emphasis-card">
        <div className="crew-stat-label">Aktiver Modus</div>
        <div className="crew-stat-value">{activeModeDefinition.title}</div>
        <div className="crew-stat-meta">{activeModeDefinition.description}</div>
      </div>

      <div className="crew-choice-grid">
        {GOVERNANCE_MODES.map((mode) => {
          const selected = mode.value === activeMode
          return (
            <button
              key={mode.value}
              type="button"
              className={`crew-choice-card${selected ? ' is-active' : ''}`}
              onClick={() => handleModeSelect(mode.value)}
            >
              <strong>{mode.title}</strong>
              <span>{mode.description}</span>
            </button>
          )
        })}
      </div>

      <div>
        <div className="crew-stat-label" style={{ marginBottom: 8 }}>Offene / letzte Freigaben</div>
        {approvals.length === 0 ? (
          <div className="crew-inline-feedback">Noch keine Freigaben fuer diese Crew vorhanden.</div>
        ) : (
          <div className="crew-stack-list">
            {approvals.slice(0, 6).map((approval) => (
              <div key={approval.id} className="crew-stack-card">
                <div className="crew-stack-card-header">
                  <strong>{formatApprovalType(approval.approvalType)}</strong>
                  <span style={{ color: approval.status === 'approved' ? 'var(--success)' : approval.status === 'rejected' ? 'var(--danger)' : 'var(--warning)' }}>{formatApprovalStatus(approval.status)}</span>
                </div>
                <div className="crew-stat-meta">
                  Angefragt: {formatTimestamp(approval.requestedAt)}
                </div>
                {approval.resolvedAt && (
                  <div className="crew-stat-meta">
                    Entschieden: {formatTimestamp(approval.resolvedAt)}
                  </div>
                )}
                {approval.status === 'pending' && (
                  <div className="crew-button-row">
                    <button type="button" className="btn-sm crew-action-btn" disabled={loading} onClick={() => void handleResolveApproval(approval.id, 'approved')}>Genehmigen</button>
                    <button type="button" className="btn-sm crew-action-btn" disabled={loading} onClick={() => void handleResolveApproval(approval.id, 'rejected')}>Ablehnen</button>
                  </div>
                )}
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}
