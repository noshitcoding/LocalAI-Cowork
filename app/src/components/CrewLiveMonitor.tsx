import { useEffect, useMemo, useRef, useState, type CSSProperties } from 'react'
import { ChevronLeft, ChevronRight } from 'lucide-react'
import type { CrewLiveEntry, CrewLiveEntryCategory, CrewLiveState } from '../stores/chatStore'

type CrewLiveMonitorProps = {
  live: CrewLiveState
}

type CrewLiveDisplayCategory = CrewLiveEntryCategory | 'runtime'
type CrewLiveFilter = 'all' | 'agent' | 'handoff' | 'thinking' | 'mcp' | 'tool' | 'error' | 'runtime'

type CrewLiveLogLine = {
  key: string
  timestamp: number
  agentId: string
  rawAgentId?: string | null
  agentLabel: string
  category: CrewLiveDisplayCategory
  label?: string
  message: string
  meta: string[]
  detail: boolean
  action: string
  severity?: 'info' | 'warning' | 'error' | null
}

type AgentStream = {
  agentId: string
  color: string
  label: string
  lastActiveAt: number
  lines: CrewLiveLogLine[]
  counts: Record<CrewLiveDisplayCategory, number>
}

const CREW_LIVE_FOCUS_COLUMNS = 3
const CREW_LIVE_FOCUS_LINES = 8
const CREW_LIVE_COLLAPSED_CHARS = 320
const CREW_LIVE_VIRTUALIZE_AFTER = 240
const CREW_LIVE_ESTIMATED_LINE_HEIGHT = 48
const CREW_LIVE_OVERSCAN_LINES = 18

const CATEGORY_LABELS: Record<CrewLiveDisplayCategory, string> = {
  status: 'Status',
  context: 'Kontext',
  agent: 'Agent',
  thinking: 'Prozess',
  handoff: 'Uebergabe',
  delegation: 'Delegation',
  tool: 'Tool',
  mcp: 'MCP',
  task: 'Task',
  result: 'Resultat',
  output: 'Ausgabe',
  error: 'Fehler',
  runtime: 'Runtime',
}

const FILTERS: Array<{ id: CrewLiveFilter; label: string }> = [
  { id: 'all', label: 'Alle' },
  { id: 'agent', label: 'Agent' },
  { id: 'handoff', label: 'Uebergabe' },
  { id: 'thinking', label: 'Prozess' },
  { id: 'mcp', label: 'MCP' },
  { id: 'tool', label: 'Tool' },
  { id: 'error', label: 'Fehler' },
  { id: 'runtime', label: 'Runtime' },
]

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

function isTechnicalAgentId(value: string): boolean {
  const normalized = value.trim().toLowerCase()
  return /(?:^|-)pers-\d{8,}/.test(normalized)
    || normalized.startsWith('personality-pers-')
    || normalized.startsWith('agent-personality-pers-')
}

function humanizeAgentId(agentId: string): string {
  const normalized = agentId
    .replace(/^agent-/, '')
    .replace(/^python-/, '')
    .replace(/^crew-/, '')
    .replace(/-/g, ' ')
    .trim()

  if (!normalized) return 'Runtime'
  return normalized
}

function getAgentLabel(agentId: string, fallbackName?: string | null): string {
  const fallback = fallbackName?.trim()
  if (fallback && !isTechnicalAgentId(fallback)) return fallback
  if (!agentId.trim()) return 'Runtime'
  if (isTechnicalAgentId(agentId)) return fallback || 'Crew-Person'
  return humanizeAgentId(agentId)
}

function splitStructuredLine(line: string): { label?: string; message: string } {
  const trimmed = line.trim()
  const match = trimmed.match(/^([A-Za-z][\w /-]{1,28}):\s*(.+)$/)
  if (!match) {
    return { message: trimmed }
  }

  return {
    label: match[1],
    message: match[2],
  }
}

function isRuntimeAgent(agentId: string): boolean {
  const normalized = agentId.trim().toLowerCase()
  return !normalized
    || normalized === 'runtime'
    || normalized.includes('runtime')
}

function isRuntimeEntry(entry: Pick<CrewLiveEntry, 'agentId' | 'action' | 'category'>): boolean {
  return isRuntimeAgent(entry.agentId)
    || entry.action === 'runtime_context'
    || entry.action === 'run_started'
    || entry.action === 'crew_kickoff'
    || entry.action === 'crew_finished'
    || entry.category === 'context'
    || entry.category === 'status'
}

function inferLineCategory(entry: CrewLiveEntry, line: string): CrewLiveDisplayCategory {
  const normalized = `${entry.action}\n${line}`.toLowerCase()

  if (/(traceback|error|failed|exception|stderr)/.test(normalized)) {
    return 'error'
  }
  if (/^(arbeitsprotokoll|thinking|reasoning):/i.test(line) || normalized.includes('arbeitsprozess')) {
    return 'thinking'
  }
  if (/^(mcp|server):/i.test(line) || /\bmcp\b/.test(normalized)) {
    return 'mcp'
  }
  if (/^(tool|args|input|call id|result):/i.test(line) || normalized.includes('tool execution')) {
    return 'tool'
  }
  if (isRuntimeEntry(entry)) {
    return 'runtime'
  }
  if (/^(task|output):/i.test(line)) {
    return entry.category === 'error' ? 'error' : 'output'
  }

  return entry.category
}

function normalizeSummaryCategory(entry: CrewLiveEntry): CrewLiveDisplayCategory {
  if (entry.category === 'error') return 'error'
  if (entry.category === 'mcp') return 'mcp'
  if (entry.category === 'tool') return 'tool'
  if (isRuntimeEntry(entry)) return 'runtime'
  return entry.category
}

function formatEntrySummary(entry: CrewLiveEntry, category: CrewLiveDisplayCategory): string {
  const fallback = entry.title.trim() || CATEGORY_LABELS[category]
  const summary = entry.summary?.trim() || fallback
  const source = entry.sourceAgent?.trim()
  const target = entry.targetAgent?.trim()

  if ((entry.category === 'handoff' || entry.category === 'delegation') && (source || target)) {
    const route = [source || 'Runtime', target || entry.agentName || entry.agentId].filter(Boolean).join(' -> ')
    const task = entry.taskTitle?.trim()
    return [route, task, summary !== route && summary !== task ? summary : ''].filter(Boolean).join(' | ')
  }

  return summary
}

function buildEntryMeta(entry: CrewLiveEntry): string[] {
  return [
    entry.provider?.trim() ? `Provider ${entry.provider.trim()}` : '',
    entry.model?.trim() ? `Modell ${entry.model.trim()}` : '',
    entry.taskTitle?.trim() ? `Task ${entry.taskTitle.trim()}` : '',
  ].filter(Boolean)
}

function buildRollingWindowLines(entries: CrewLiveEntry[]): CrewLiveLogLine[] {
  const allLines = entries.flatMap((entry) => {
    const summaryCategory = normalizeSummaryCategory(entry)
    const agentLabel = getAgentLabel(entry.agentId, entry.agentName)
    const summaryLine: CrewLiveLogLine = {
      key: `${entry.id}-summary`,
      timestamp: entry.timestamp,
      agentId: entry.agentId,
      rawAgentId: entry.rawAgentId,
      agentLabel,
      category: summaryCategory,
      message: formatEntrySummary(entry, summaryCategory),
      meta: buildEntryMeta(entry),
      detail: false,
      action: entry.action,
      severity: entry.severity,
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
          rawAgentId: entry.rawAgentId,
          agentLabel,
          category: inferLineCategory(entry, line),
          label: structured.label,
          message: structured.message,
          meta: [],
          detail: true,
          action: entry.action,
          severity: entry.severity,
        } satisfies CrewLiveLogLine
      })

    return [summaryLine, ...detailLines]
  })

  return allLines
}

function createEmptyCounts(): Record<CrewLiveDisplayCategory, number> {
  return {
    status: 0,
    context: 0,
    agent: 0,
    thinking: 0,
    handoff: 0,
    delegation: 0,
    tool: 0,
    mcp: 0,
    task: 0,
    result: 0,
    output: 0,
    error: 0,
    runtime: 0,
  }
}

function isPersonLine(line: CrewLiveLogLine): boolean {
  return !isRuntimeAgent(line.agentId)
}

function buildAgentStreams(lines: CrewLiveLogLine[], agentColors: Record<string, string>): AgentStream[] {
  const streams = new Map<string, AgentStream>()

  for (const line of lines) {
    if (!isPersonLine(line)) continue

    const existing = streams.get(line.agentId)
    const stream = existing ?? {
      agentId: line.agentId,
      color: agentColors[line.agentId] ?? '#64748b',
      label: line.agentLabel,
      lastActiveAt: 0,
      lines: [],
      counts: createEmptyCounts(),
    }

    stream.lastActiveAt = Math.max(stream.lastActiveAt, line.timestamp)
    stream.lines.push(line)
    stream.counts[line.category] += 1
    streams.set(line.agentId, stream)
  }

  return [...streams.values()].sort((a, b) => b.lastActiveAt - a.lastActiveAt || a.label.localeCompare(b.label))
}

function getVisibleAgentIds(streams: AgentStream[], startAgentId: string | null): string[] {
  if (streams.length <= CREW_LIVE_FOCUS_COLUMNS) {
    return streams.map((stream) => stream.agentId)
  }

  const startIndex = Math.max(0, streams.findIndex((stream) => stream.agentId === startAgentId))
  return Array.from({ length: CREW_LIVE_FOCUS_COLUMNS }, (_, offset) => (
    streams[(startIndex + offset) % streams.length].agentId
  ))
}

function filterLine(line: CrewLiveLogLine, filter: CrewLiveFilter): boolean {
  if (filter === 'all') return true
  if (filter === 'agent') {
    return isPersonLine(line) && !['handoff', 'delegation', 'mcp', 'tool', 'error', 'runtime'].includes(line.category)
  }
  if (filter === 'handoff') {
    return line.category === 'handoff' || line.category === 'delegation'
  }
  return line.category === filter
}

function getFilterCount(lines: CrewLiveLogLine[], filter: CrewLiveFilter): number {
  return lines.filter((line) => filterLine(line, filter)).length
}

function getCollapsedMessage(message: string, expanded: boolean): string {
  if (expanded || message.length <= CREW_LIVE_COLLAPSED_CHARS) return message
  return `${message.slice(0, CREW_LIVE_COLLAPSED_CHARS).trimEnd()}...`
}

export default function CrewLiveMonitor({ live }: CrewLiveMonitorProps) {
  const logRef = useRef<HTMLDivElement | null>(null)
  const shouldAutoScrollRef = useRef(true)
  const [filter, setFilter] = useState<CrewLiveFilter>('all')
  const [expandedKeys, setExpandedKeys] = useState<Set<string>>(() => new Set())
  const [manualFocus, setManualFocus] = useState(false)
  const [focusStartAgentId, setFocusStartAgentId] = useState<string | null>(null)
  const [mobileAgentId, setMobileAgentId] = useState<string | null>(null)
  const [logViewport, setLogViewport] = useState({ scrollTop: 0, height: 320 })

  const rollingWindowLines = useMemo(() => buildRollingWindowLines(live.entries), [live.entries])
  const agentStreams = useMemo(
    () => buildAgentStreams(rollingWindowLines, live.agentColors),
    [rollingWindowLines, live.agentColors],
  )
  const focusedAgentIds = useMemo(() => {
    if (agentStreams.length === 0) return []
    const startAgentId = manualFocus ? focusStartAgentId : agentStreams[0]?.agentId ?? null
    return getVisibleAgentIds(agentStreams, startAgentId)
  }, [agentStreams, focusStartAgentId, manualFocus])
  const focusedStreams = useMemo(() => {
    const streamById = new Map(agentStreams.map((stream) => [stream.agentId, stream]))
    return focusedAgentIds.map((agentId) => streamById.get(agentId)).filter(Boolean) as AgentStream[]
  }, [agentStreams, focusedAgentIds])
  const activeMobileAgentId = mobileAgentId && focusedAgentIds.includes(mobileAgentId)
    ? mobileAgentId
    : focusedAgentIds[0] ?? null
  const filteredLines = useMemo(
    () => rollingWindowLines.filter((line) => filterLine(line, filter)),
    [rollingWindowLines, filter],
  )
  const highlightedCounts = useMemo(() => ({
    handoff: getFilterCount(rollingWindowLines, 'handoff'),
    thinking: getFilterCount(rollingWindowLines, 'thinking'),
    tool: getFilterCount(rollingWindowLines, 'tool'),
    mcp: getFilterCount(rollingWindowLines, 'mcp'),
    error: getFilterCount(rollingWindowLines, 'error'),
    runtime: getFilterCount(rollingWindowLines, 'runtime'),
  }), [rollingWindowLines])
  const virtualized = filteredLines.length > CREW_LIVE_VIRTUALIZE_AFTER
  const virtualRange = useMemo(() => {
    if (!virtualized) {
      return { start: 0, end: filteredLines.length }
    }

    const start = Math.max(
      0,
      Math.floor(logViewport.scrollTop / CREW_LIVE_ESTIMATED_LINE_HEIGHT) - CREW_LIVE_OVERSCAN_LINES,
    )
    const visibleCount = Math.ceil(logViewport.height / CREW_LIVE_ESTIMATED_LINE_HEIGHT) + (CREW_LIVE_OVERSCAN_LINES * 2)
    return {
      start,
      end: Math.min(filteredLines.length, start + visibleCount),
    }
  }, [filteredLines.length, logViewport.height, logViewport.scrollTop, virtualized])
  const visibleLines = useMemo(
    () => filteredLines.slice(virtualRange.start, virtualRange.end),
    [filteredLines, virtualRange.end, virtualRange.start],
  )

  useEffect(() => {
    const node = logRef.current
    if (!node || !shouldAutoScrollRef.current) return
    node.scrollTop = node.scrollHeight
    setLogViewport({
      scrollTop: node.scrollTop,
      height: node.clientHeight || 320,
    })
  }, [filteredLines.length, live.updatedAt])

  useEffect(() => {
    const node = logRef.current
    if (!node) return
    setLogViewport({
      scrollTop: node.scrollTop,
      height: node.clientHeight || 320,
    })
  }, [filter, filteredLines.length])

  const handleLogScroll = () => {
    const node = logRef.current
    if (!node) return
    shouldAutoScrollRef.current = node.scrollHeight - node.scrollTop - node.clientHeight < 40
    setLogViewport({
      scrollTop: node.scrollTop,
      height: node.clientHeight || 320,
    })
  }

  const shiftFocus = (direction: -1 | 1) => {
    if (agentStreams.length <= CREW_LIVE_FOCUS_COLUMNS) return
    const currentStart = focusStartAgentId ?? focusedAgentIds[0] ?? agentStreams[0].agentId
    const currentIndex = Math.max(0, agentStreams.findIndex((stream) => stream.agentId === currentStart))
    const nextIndex = (currentIndex + direction + agentStreams.length) % agentStreams.length
    setManualFocus(true)
    setFocusStartAgentId(agentStreams[nextIndex].agentId)
  }

  const toggleExpanded = (key: string) => {
    setExpandedKeys((current) => {
      const next = new Set(current)
      if (next.has(key)) {
        next.delete(key)
      } else {
        next.add(key)
      }
      return next
    })
  }

  const renderLine = (line: CrewLiveLogLine, mode: 'focus' | 'log') => {
    const color = live.agentColors[line.agentId] ?? '#64748b'
    const categoryLabel = CATEGORY_LABELS[line.category]
    const expanded = expandedKeys.has(line.key)
    const canExpand = line.message.length > CREW_LIVE_COLLAPSED_CHARS
    const message = getCollapsedMessage(line.message, expanded)
    const rawAgentTitle = line.rawAgentId && line.rawAgentId !== line.agentId ? `Technische ID: ${line.rawAgentId}` : undefined

    return (
      <div
        key={`${mode}-${line.key}`}
        className={`${mode === 'log' ? 'crew-live-line' : 'crew-live-focus-event'} tone-${line.category}${line.detail ? ' is-detail' : ''}`}
        style={{ '--crew-agent-color': color } as CSSProperties}
      >
        <span className="crew-live-line-time">{formatTime(line.timestamp)}</span>
        {mode === 'log' && <span className="crew-live-line-agent" title={rawAgentTitle}>{line.agentLabel}</span>}
        <span className={`crew-live-line-badge tone-${line.category}`}>{categoryLabel}</span>
        <span className="crew-live-line-message">
          <span className="crew-live-line-message-main">
            {line.label ? <span className="crew-live-line-message-key">{line.label}</span> : null}
            <span>{message}</span>
          </span>
          {line.meta.length > 0 ? (
            <span className="crew-live-line-meta">{line.meta.join(' | ')}</span>
          ) : null}
          {canExpand ? (
            <button
              type="button"
              className="crew-live-expand"
              onClick={() => toggleExpanded(line.key)}
            >
              {expanded ? 'Weniger anzeigen' : 'Mehr anzeigen'}
            </button>
          ) : null}
        </span>
      </div>
    )
  }

  return (
    <section className={`crew-live-monitor status-${live.status}`}>
      <div className="crew-live-header">
        <div>
          <div className="crew-live-kicker">Crew Live Verbose</div>
          <div className="crew-live-title">{live.title}</div>
        </div>
        <div className={`crew-live-status status-${live.status}`}>
          {live.status}
        </div>
      </div>

      <div className="crew-live-summary" aria-label="Log-Zusammenfassung">
        <div className="crew-live-summary-item">
          <span className="crew-live-summary-label">Eventliste</span>
          <strong className="crew-live-summary-value">
            {rollingWindowLines.length} Zeilen
          </strong>
        </div>
        <div className="crew-live-summary-item">
          <span className="crew-live-summary-label">Personen-Fokus</span>
          <strong className="crew-live-summary-value">{focusedStreams.length} / {agentStreams.length}</strong>
        </div>
        <div className="crew-live-summary-item">
          <span className="crew-live-summary-label">Uebergaben</span>
          <strong className="crew-live-summary-value tone-handoff">{highlightedCounts.handoff}</strong>
        </div>
        <div className="crew-live-summary-item">
          <span className="crew-live-summary-label">Prozess</span>
          <strong className="crew-live-summary-value tone-thinking">{highlightedCounts.thinking}</strong>
        </div>
        <div className="crew-live-summary-item">
          <span className="crew-live-summary-label">Tool</span>
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
        <div className="crew-live-summary-item">
          <span className="crew-live-summary-label">Runtime</span>
          <strong className="crew-live-summary-value tone-runtime">{highlightedCounts.runtime}</strong>
        </div>
      </div>

      <div className="crew-live-focus-shell">
        <div className="crew-live-section-title">
          <span>Aktive Personen</span>
          <div className="crew-live-gallery-controls" aria-label="Personen-Galerie">
            <button
              type="button"
              className="crew-live-icon-button"
              aria-label="Vorherige Personen anzeigen"
              disabled={agentStreams.length <= CREW_LIVE_FOCUS_COLUMNS}
              onClick={() => shiftFocus(-1)}
            >
              <ChevronLeft size={16} aria-hidden="true" />
            </button>
            <button
              type="button"
              className="crew-live-icon-button"
              aria-label="Naechste Personen anzeigen"
              disabled={agentStreams.length <= CREW_LIVE_FOCUS_COLUMNS}
              onClick={() => shiftFocus(1)}
            >
              <ChevronRight size={16} aria-hidden="true" />
            </button>
          </div>
        </div>

        {focusedStreams.length > 0 ? (
          <>
            <div className="crew-live-focus-tabs" aria-label="Mobile Personen-Auswahl">
              {focusedStreams.map((stream) => (
                <button
                  key={stream.agentId}
                  type="button"
                  className={`crew-live-focus-tab${activeMobileAgentId === stream.agentId ? ' active' : ''}`}
                  onClick={() => setMobileAgentId(stream.agentId)}
                >
                  {stream.label}
                </button>
              ))}
            </div>
            <div className="crew-live-focus-grid" aria-label="Aktive Crew-Personen">
              {focusedStreams.map((stream) => (
                <article
                  key={stream.agentId}
                  className={`crew-live-focus-column${activeMobileAgentId !== stream.agentId ? ' is-mobile-hidden' : ''}`}
                  style={{ '--crew-agent-color': stream.color } as CSSProperties}
                >
                  <div className="crew-live-focus-header">
                    <div>
                      <div className="crew-live-focus-name">{stream.label}</div>
                      <div className="crew-live-focus-time">aktiv {formatTime(stream.lastActiveAt)}</div>
                    </div>
                    <div className="crew-live-focus-count">{stream.lines.length}</div>
                  </div>
                  <div className="crew-live-focus-metrics" aria-label={`${stream.label} Ereignisse`}>
                    <span className="tone-handoff">Uebergabe {stream.counts.handoff + stream.counts.delegation}</span>
                    <span className="tone-thinking">Prozess {stream.counts.thinking}</span>
                    <span className="tone-tool">Tool {stream.counts.tool}</span>
                    <span className="tone-mcp">MCP {stream.counts.mcp}</span>
                    <span className="tone-error">Fehler {stream.counts.error}</span>
                  </div>
                  <div className="crew-live-focus-events">
                    {stream.lines.slice(-CREW_LIVE_FOCUS_LINES).map((line) => renderLine(line, 'focus'))}
                  </div>
                </article>
              ))}
            </div>
          </>
        ) : (
          <div className="crew-live-empty">Warte auf Agent-Ausgaben...</div>
        )}
      </div>

      <div className="crew-live-events">
        <div className="crew-live-section-title">
          <span>Event-Historie</span>
          <div className="crew-live-filterbar" aria-label="Log-Filter">
            {FILTERS.map((entry) => (
              <button
                key={entry.id}
                type="button"
                className={`crew-live-filter-chip${filter === entry.id ? ' active' : ''}`}
                aria-label={`Filter ${entry.label}`}
                onClick={() => setFilter(entry.id)}
              >
                {entry.label}
                <span>{getFilterCount(rollingWindowLines, entry.id)}</span>
              </button>
            ))}
          </div>
        </div>

        {filteredLines.length === 0 ? (
          <div className="crew-live-empty">Keine Ereignisse fuer diesen Filter.</div>
        ) : (
          <div
            className="crew-live-log"
            ref={logRef}
            aria-label="Crew-Live-Log"
            aria-rowcount={filteredLines.length}
            aria-live={live.status === 'running' ? 'polite' : undefined}
            onScroll={handleLogScroll}
          >
            {virtualized ? (
              <div
                aria-hidden="true"
                className="crew-live-virtual-pad"
                style={{ height: virtualRange.start * CREW_LIVE_ESTIMATED_LINE_HEIGHT }}
              />
            ) : null}
            {visibleLines.map((line) => renderLine(line, 'log'))}
            {virtualized ? (
              <div
                aria-hidden="true"
                className="crew-live-virtual-pad"
                style={{ height: Math.max(0, filteredLines.length - virtualRange.end) * CREW_LIVE_ESTIMATED_LINE_HEIGHT }}
              />
            ) : null}
          </div>
        )}
      </div>
    </section>
  )
}
