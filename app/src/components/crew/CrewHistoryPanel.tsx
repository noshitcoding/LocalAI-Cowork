import { useEffect, useState } from 'react'
import { safeInvoke } from '../../utils/safeInvoke'

type CrewRunHistoryRow = {
  id: string
  crewId: string
  crewName: string
  process: string
  status: string
  managerAgentId: string | null
  error: string | null
  startedAt: string
  finishedAt: string | null
}

type CrewExecutionLogRow = {
  id: string
  crewId: string
  agentId: string
  taskId: string
  action: string
  result: string
  timestamp: number
}

type CrewRunEventRow = {
  id: string
  runId: string
  crewId: string
  eventType: string
  payloadJson: string | null
  createdAt: string
}

type CrewExecutionResponse = {
  crewId: string
  status: string
  error: string | null
}

type Props = {
  activeCrewId: string
}

function formatTimestamp(value: string | number | null): string {
  if (!value) return '—'
  const date = typeof value === 'number' ? new Date(value) : new Date(value)
  return Number.isNaN(date.getTime()) ? String(value) : date.toLocaleString('de-DE')
}

export default function CrewHistoryPanel({ activeCrewId }: Props) {
  const [runs, setRuns] = useState<CrewRunHistoryRow[]>([])
  const [selectedRunId, setSelectedRunId] = useState<string | null>(null)
  const [logs, setLogs] = useState<CrewExecutionLogRow[]>([])
  const [events, setEvents] = useState<CrewRunEventRow[]>([])
  const [error, setError] = useState<string | null>(null)
  const [refreshToken, setRefreshToken] = useState(0)
  const [replaying, setReplaying] = useState(false)
  const [replayMessage, setReplayMessage] = useState<string | null>(null)

  const selectedRun = runs.find((run) => run.id === selectedRunId) ?? null

  useEffect(() => {
    let cancelled = false
    void safeInvoke<CrewRunHistoryRow[]>('crew_runs_list', { crewId: activeCrewId, limit: 12 }, [])
      .then((rows) => {
        if (cancelled) return
        const safeRows = Array.isArray(rows) ? rows : []
        setRuns(safeRows)
        setSelectedRunId((current) => current && safeRows.some((row) => row.id === current) ? current : safeRows[0]?.id ?? null)
      })
      .catch((value) => {
        if (!cancelled) setError(value instanceof Error ? value.message : String(value))
      })

    return () => {
      cancelled = true
    }
  }, [activeCrewId, refreshToken])

  useEffect(() => {
    if (!selectedRunId) {
      setLogs([])
      setEvents([])
      return
    }

    let cancelled = false
    void Promise.all([
      safeInvoke<CrewExecutionLogRow[]>('crew_run_logs_list', { runId: selectedRunId }, []),
      safeInvoke<CrewRunEventRow[]>('crew_run_events_list', { runId: selectedRunId, limit: 120 }, []),
    ])
      .then(([nextLogs, nextEvents]) => {
        if (cancelled) return
        setLogs(Array.isArray(nextLogs) ? nextLogs : [])
        setEvents(Array.isArray(nextEvents) ? nextEvents : [])
      })
      .catch((value) => {
        if (!cancelled) setError(value instanceof Error ? value.message : String(value))
      })

    return () => {
      cancelled = true
    }
  }, [selectedRunId, refreshToken])

  const handleReplaySelectedRun = async () => {
    if (!selectedRunId) {
      return
    }

    setReplaying(true)
    setReplayMessage(null)
    setError(null)

    try {
      const response = await safeInvoke<CrewExecutionResponse>('crew_run_replay', { runId: selectedRunId }, undefined)
      setReplayMessage(response.error ?? `Replay abgeschlossen: ${response.status}`)
      setSelectedRunId(null)
      setRefreshToken((value) => value + 1)
    } catch (value) {
      setError(value instanceof Error ? value.message : String(value))
    } finally {
      setReplaying(false)
    }
  }

  return (
    <div className="card crew-overview-card">
      <div className="crew-overview-head">
        <div className="crew-overview-copy">
          <div className="crew-overview-kicker">History</div>
          <strong className="crew-overview-title">Crew-Runs & Events</strong>
        </div>
        <button type="button" className="btn-sm crew-action-btn" disabled={!selectedRunId || replaying} onClick={() => void handleReplaySelectedRun()}>
          {replaying ? 'Replay laeuft…' : 'Ausgewaehlten Run replayen'}
        </button>
      </div>

      {error && <div className="crew-inline-feedback error">{error}</div>}
      {replayMessage && <div className="crew-inline-feedback">{replayMessage}</div>}

      {runs.length === 0 ? (
        <div className="crew-inline-feedback">Noch keine gespeicherten Runs fuer diese Crew.</div>
      ) : (
        <div className="crew-history-grid">
          <div className="crew-run-list">
            {runs.map((run) => (
              <button
                key={run.id}
                type="button"
                className={`crew-run-card${selectedRunId === run.id ? ' active' : ''}`}
                onClick={() => setSelectedRunId(run.id)}
              >
                <div className="crew-stack-card-header">
                  <strong>{run.process}</strong>
                  <span style={{ color: run.status === 'completed' ? 'var(--success)' : run.status === 'failed' ? 'var(--danger)' : 'var(--text-muted)' }}>{run.status}</span>
                </div>
                <div className="crew-stat-meta">{formatTimestamp(run.startedAt)}</div>
              </button>
            ))}
          </div>

          <div className="crew-run-body">
            {selectedRun && (
              <div className="crew-stack-card crew-emphasis-card">
                <div className="crew-stack-card-header">
                  <strong>{selectedRun.crewName}</strong>
                  <span style={{ color: selectedRun.status === 'completed' ? 'var(--success)' : selectedRun.status === 'failed' ? 'var(--danger)' : 'var(--text-muted)' }}>{selectedRun.status}</span>
                </div>
                <div className="crew-stat-meta">
                  Start: {formatTimestamp(selectedRun.startedAt)} · Ende: {formatTimestamp(selectedRun.finishedAt)}
                </div>
                {selectedRun.error && (
                  <div className="crew-inline-feedback error">{selectedRun.error}</div>
                )}
              </div>
            )}

            <div>
              <div className="crew-stat-label" style={{ marginBottom: 8 }}>Events</div>
              <div className="crew-stack-list crew-scroll-stack">
                {events.length === 0 ? (
                  <div className="crew-inline-feedback">Noch keine Events gespeichert.</div>
                ) : events.slice(0, 8).map((event) => (
                  <div key={event.id} className="crew-stack-card">
                    <div className="crew-stack-card-header">
                      <strong>{event.eventType}</strong>
                      <span>{formatTimestamp(event.createdAt)}</span>
                    </div>
                  </div>
                ))}
              </div>
            </div>

            <div>
              <div className="crew-stat-label" style={{ marginBottom: 8 }}>Logs</div>
              <div className="crew-stack-list crew-scroll-stack">
                {logs.length === 0 ? (
                  <div className="crew-inline-feedback">Noch keine Logs fuer diesen Run.</div>
                ) : logs.slice(0, 8).map((log) => (
                  <div key={log.id} className="crew-stack-card">
                    <div className="crew-stat-value">{log.action}</div>
                    <div className="crew-stat-meta">{log.agentId} • {log.taskId}</div>
                    <div className="crew-stat-meta">{log.result.slice(0, 180)}</div>
                  </div>
                ))}
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}