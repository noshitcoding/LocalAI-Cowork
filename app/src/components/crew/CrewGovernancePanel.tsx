import { useEffect, useMemo, useState } from 'react'
import { useCrewControlPlaneStore } from '../../stores/crewControlPlaneStore'
import { useCrewStore, type CrewGovernanceMode } from '../../stores/crewStore'
import i18n, { tr } from '../../i18n'

const GOVERNANCE_MODES: Array<{
  value: CrewGovernanceMode
  title: string
  description: string
}> = [
  {
    value: 'allow-all',
    title: 'Allow all',
    description: 'Crew starts without a question. Existing manual approvals still remain effective.',
  },
  {
    value: 'ask-risky',
    title: 'Ask only for risky actions',
    description: 'Ask only for risky tools, MCP access, or delegation.',
  },
  {
    value: 'ask-all',
    title: 'Always ask before actions',
    description: 'Every crew start is first paused by the software and continues only after approval.',
  },
  {
    value: 'read-only',
    title: 'Read only',
    description: 'Allow only read access such as read_file, grep, glob, web_fetch, and web_search.',
  },
]

type Props = {
  activeCrewId: string
}

function formatTimestamp(value: string | null): string {
  if (!value) return '-'
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString(i18n.resolvedLanguage ?? i18n.language ?? 'en')
}

function formatApprovalType(value: string): string {
  if (value === 'run_gate') return 'Start approval'
  if (value === 'tool_gate') return 'Tool approval'
  if (value === 'delegation_gate') return 'Delegation approval'
  return value
}

function formatApprovalStatus(value: string): string {
  if (value === 'approved') return 'approved'
  if (value === 'rejected') return 'rejected'
  if (value === 'pending') return 'pending'
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
    setNotice(definition ? { crewId: activeCrewId, message: `Governance mode set: ${definition.title}.` } : null)
  }

  const handleResolveApproval = async (approvalId: string, status: 'approved' | 'rejected') => {
    await resolveApproval({
      id: approvalId,
      crewId: activeCrewId,
      status,
      resolvedBy: 'manual-panel',
      resolutionNote: status === 'approved' ? 'Manually approved' : 'Manually rejected',
    })

    if (!useCrewControlPlaneStore.getState().error) {
      setNotice(
        {
          crewId: activeCrewId,
          message: status === 'approved'
            ? 'Approval granted. If a crew run was paused, it will now continue in the background.'
            : 'Approval rejected.',
        },
      )
    }
  }

  return (
    <div className="card crew-overview-card">
      <div className="crew-overview-copy">
        <div className="crew-overview-kicker">{tr("Governance")}</div>
        <strong className="crew-overview-title">{tr("Approvals and protection mode")}</strong>
      </div>

      {error && <div className="crew-inline-feedback error">{error}</div>}
      {notice?.crewId === activeCrewId && <div className="crew-inline-feedback">{notice.message}</div>}

      <div className="crew-stat-card crew-emphasis-card">
        <div className="crew-stat-label">{tr("Active mode")}</div>
        <div className="crew-stat-value">{tr(activeModeDefinition.title)}</div>
        <div className="crew-stat-meta">{tr(activeModeDefinition.description)}</div>
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
              <strong>{tr(mode.title)}</strong>
              <span>{tr(mode.description)}</span>
            </button>
          )
        })}
      </div>

      <div>
        <div className="crew-stat-label crew-stat-label-spaced">{tr("Open / latest approvals")}</div>
        {approvals.length === 0 ? (
          <div className="crew-inline-feedback">{tr("No approvals available for this crew yet.")}</div>
        ) : (
          <div className="crew-stack-list">
            {approvals.slice(0, 6).map((approval) => (
              <div key={approval.id} className="crew-stack-card">
                <div className="crew-stack-card-header">
                  <strong>{formatApprovalType(approval.approvalType)}</strong>
                  <span className={`crew-approval-status ${approval.status}`}>{formatApprovalStatus(approval.status)}</span>
                </div>
                <div className="crew-stat-meta">{tr("Angefragt:")}{formatTimestamp(approval.requestedAt)}
                </div>
                {approval.resolvedAt && (
                  <div className="crew-stat-meta">{tr("Entschieden:")}{formatTimestamp(approval.resolvedAt)}
                  </div>
                )}
                {approval.status === 'pending' && (
                  <div className="crew-button-row">
                    <button type="button" className="btn-sm crew-action-btn" disabled={loading} onClick={() => void handleResolveApproval(approval.id, 'approved')}>{tr("Approve")}</button>
                    <button type="button" className="btn-sm crew-action-btn" disabled={loading} onClick={() => void handleResolveApproval(approval.id, 'rejected')}>{tr("Reject")}</button>
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
