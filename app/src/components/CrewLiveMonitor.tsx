import { useEffect, useMemo, useRef, type CSSProperties } from 'react'
import type { CrewLiveEntry, CrewLiveState } from '../stores/chatStore'

type CrewLiveMonitorProps = {
  live: CrewLiveState
}

type CrewLiveLogLine = {
  key: string
  timestamp: number
  agentId: string
  category: CrewLiveEntry['category']
  label?: string
  message: string
  detail: boolean
}

const CREW_LIVE_ROLLING_WINDOW_LINES = 150

const CATEGORY_LABELS: Record<CrewLiveEntry['category'], string> = {
  status: 'Status',
  context: 'Kontext',
  agent: 'Agent',
  handoff: 'Uebergabe',
  delegation: 'Delegation',
  tool: 'Tool',
  mcp: 'MCP',
  task: 'Task',
  output: 'Ausgabe',
  error: 'Fehler',
}

function formatTime(timestamp: number): string {
  try {
    return new Date(timestamp).toLocaleTimeString('de-DE', {
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
    })
  } catch {
    return '--:--:--'
  }
}

function getAgentLabel(agentId: string): string {
  if (!agentId.trim()) return 'runtime'
  return agentId
    .replace(/^agent-/, '')
    .replace(/^python-/, '')
    .replace(/^crew-/, '')
}

function splitStructuredLine(line: string): { label?: string; message: string } {
  const trimmed = line.trim()
  const match = trimmed.match(/^([A-Za-z][\w /-]{1,24}):\s*(.+)$/)
  if (!match) {
    return { message: trimmed }
  }

  return {
    label: match[1],
    message: match[2],
  }
}

function inferLineCategory(entry: CrewLiveEntry, line: string): CrewLiveEntry['category'] {
  const normalized = line.toLowerCase()

  if (/(traceback|error|failed|exception)/.test(normalized)) {
    return 'error'
  }
  if (/^(tool|args|input|call id):/i.test(line)) {
    return 'tool'
  }
  if (/^(mcp|server):/i.test(line) || normalized.includes(' mcp ')) {
    return 'mcp'
  }
  if (/^(task|result|output):/i.test(line)) {
    return entry.category === 'error' ? 'error' : 'output'
  }

  return entry.category
}

function buildRollingWindowLines(entries: CrewLiveEntry[]): CrewLiveLogLine[] {
  const allLines = entries.flatMap((entry) => {
    const summaryLine: CrewLiveLogLine = {
      key: `${entry.id}-summary`,
      timestamp: entry.timestamp,
      agentId: entry.agentId,
      category: entry.category,
      message: entry.title.trim() || CATEGORY_LABELS[entry.category],
      detail: false,
    }

    const detailLines = entry.detail
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean)
      .filter((line) => line !== summaryLine.message)
      .map((line, index) => {
        const structured = splitStructuredLine(line)
        return {
          key: `${entry.id}-detail-${index}`,
          timestamp: entry.timestamp,
          agentId: entry.agentId,
          category: inferLineCategory(entry, line),
          label: structured.label,
          message: structured.message,
          detail: true,
        } satisfies CrewLiveLogLine
      })

    return [summaryLine, ...detailLines]
  })

  return allLines.slice(-CREW_LIVE_ROLLING_WINDOW_LINES)
}

export default function CrewLiveMonitor({ live }: CrewLiveMonitorProps) {
  const eventsRef = useRef<HTMLDivElement | null>(null)
  const agents = useMemo(
    () => Object.entries(live.agentColors).filter(([agentId]) => agentId.trim()),
    [live.agentColors],
  )
  const rollingWindowLines = useMemo(() => buildRollingWindowLines(live.entries), [live.entries])
  const highlightedCounts = useMemo(() => ({
    tool: rollingWindowLines.filter((line) => line.category === 'tool').length,
    mcp: rollingWindowLines.filter((line) => line.category === 'mcp').length,
    error: rollingWindowLines.filter((line) => line.category === 'error').length,
  }), [rollingWindowLines])

  useEffect(() => {
    const node = eventsRef.current
    if (!node) return
    node.scrollTop = node.scrollHeight
  }, [rollingWindowLines.length, live.updatedAt])

  return (
    <section className={`crew-live-monitor status-${live.status}`}>
      <div className="crew-live-header">
        <div>
          <div className="crew-live-kicker">Crew Live</div>
          <div className="crew-live-title">{live.title}</div>
        </div>
        <div className={`crew-live-status status-${live.status}`}>
          {live.status}
        </div>
      </div>

      {agents.length > 0 && (
        <div className="crew-live-agents" aria-label="Crew-Mitglieder">
          {agents.map(([agentId, color]) => (
            <span
              key={agentId}
              className="crew-live-agent-chip"
              style={{ '--crew-agent-color': color } as CSSProperties}
            >
              {getAgentLabel(agentId)}
            </span>
          ))}
        </div>
      )}

      <div className="crew-live-summary" aria-label="Log-Zusammenfassung">
        <div className="crew-live-summary-item">
          <span className="crew-live-summary-label">Rolling Window</span>
          <strong className="crew-live-summary-value">
            {rollingWindowLines.length} / {CREW_LIVE_ROLLING_WINDOW_LINES} Zeilen
          </strong>
        </div>
        <div className="crew-live-summary-item">
          <span className="crew-live-summary-label">Tool-Aufrufe</span>
          <strong className="crew-live-summary-value tone-tool">{highlightedCounts.tool}</strong>
        </div>
        <div className="crew-live-summary-item">
          <span className="crew-live-summary-label">MCP</span>
          <strong className="crew-live-summary-value tone-mcp">{highlightedCounts.mcp}</strong>
        </div>
        <div className="crew-live-summary-item">
          <span className="crew-live-summary-label">Fehler</span>
          <strong className="crew-live-summary-value tone-error">{highlightedCounts.error}</strong>
        </div>
      </div>

      <div className="crew-live-events">
        {rollingWindowLines.length === 0 ? (
          <div className="crew-live-empty">Warte auf Crew-Ereignisse...</div>
        ) : (
          <div
            className="crew-live-log"
            ref={eventsRef}
            aria-label="Crew-Live-Log"
            aria-live={live.status === 'running' ? 'polite' : undefined}
          >
            {rollingWindowLines.map((line) => {
              const color = live.agentColors[line.agentId] ?? '#64748b'
              const agentLabel = getAgentLabel(line.agentId)
              const categoryLabel = CATEGORY_LABELS[line.category]

              return (
                <div
                  key={line.key}
                  className={`crew-live-line tone-${line.category}${line.detail ? ' is-detail' : ''}`}
                  style={{ '--crew-agent-color': color } as CSSProperties}
                >
                  <span className="crew-live-line-time">{formatTime(line.timestamp)}</span>
                  <span className="crew-live-line-agent">{agentLabel}</span>
                  <span className={`crew-live-line-badge tone-${line.category}`}>{categoryLabel}</span>
                  <span className="crew-live-line-message">
                    {line.label ? <span className="crew-live-line-message-key">{line.label}</span> : null}
                    <span>{line.message}</span>
                  </span>
                </div>
              )
            })}
          </div>
        )}
      </div>
    </section>
  )
}
