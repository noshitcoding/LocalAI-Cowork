import { useEffect } from 'react'
import { useInsightsStore } from '../stores/insightsStore'
import i18n, { tr } from '../i18n'

type MetricTone = 'accent' | 'info' | 'success' | 'warning' | 'primary'

function getLocale(): string {
  return i18n.resolvedLanguage ?? i18n.language ?? 'en'
}

function formatDateTime(value: string): string {
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString(getLocale())
}

function MetricCard({
  value,
  label,
  tone = 'primary',
  compact = false,
}: {
  value: string | number
  label: string
  tone?: MetricTone
  compact?: boolean
}) {
  return (
    <div className="card insights-metric-card">
      <div className={`insights-metric-value tone-${tone}${compact ? ' compact' : ''}`}>{value}</div>
      <div className="insights-metric-label">{label}</div>
    </div>
  )
}

export default function InsightsPanel() {
  const { summary, events, loading, error, loadSummary, loadEvents } = useInsightsStore()
  const locale = getLocale()

  useEffect(() => {
    loadSummary()
    loadEvents(undefined, 50)
  }, [loadSummary, loadEvents])

  const recentSummaryEvents = summary?.recentEvents ?? []
  const topCategories = summary?.topCategories ?? []
  const totalTokensEst = summary?.totalTokensEst ?? 0
  const avgSessionDurationMin = summary?.avgSessionDurationMin ?? 0

  return (
    <div className="panel insights-panel">
      <h2>{tr("Insights dashboard")}</h2>

      {error && <p className="insights-error">{error}</p>}

      {loading && <p className="panel-empty">{tr("Loading...")}</p>}

      {summary && (
        <>
          <div className="insights-metric-grid">
            <MetricCard value={summary.totalSessions} label={tr("Sessions")} tone="accent" />
            <MetricCard value={summary.skillUsageCount} label={tr("Skill uses")} tone="info" />
            <MetricCard value={summary.memoryEntryCount} label={tr("Memory")} tone="success" />
            <MetricCard value={summary.totalEvents} label={tr("Events")} tone="warning" />
          </div>

          <div className="insights-metric-grid insights-metric-grid-compact">
            <MetricCard value={summary.totalMessagesSent} label={tr("Messages")} compact />
            <MetricCard value={totalTokensEst.toLocaleString(locale)} label={tr("Tokens (est.)")} compact />
            <MetricCard value={`${avgSessionDurationMin.toFixed(1)}${tr("min")}`} label={tr("Avg. duration")} compact />
          </div>

          {recentSummaryEvents.length > 0 && (
            <section className="insights-section" aria-labelledby="insights-summary-events-title">
              <h3 id="insights-summary-events-title" className="insights-section-heading">{tr("Latest events")}</h3>
              <div className="insights-event-list">
                {recentSummaryEvents.slice(0, 10).map((event, index) => (
                  <div key={`${event.eventType}-${event.createdAt}-${index}`} className="insights-event-row">
                    <span className="insights-event-main">
                      <span className="insights-event-type">{event.eventType}</span>
                      <span className="insights-event-category">({event.category})</span>
                    </span>
                    <span className="insights-event-time">{formatDateTime(event.createdAt)}</span>
                  </div>
                ))}
              </div>
            </section>
          )}

          {topCategories.length > 0 && (
            <section className="insights-section" aria-labelledby="insights-top-categories-title">
              <h3 id="insights-top-categories-title" className="insights-section-heading">{tr("Top categories")}</h3>
              <div className="insights-category-list">
                {topCategories.map((category) => (
                  <span key={category.category} className="card insights-category-pill">
                    {category.category}: {category.count}
                  </span>
                ))}
              </div>
            </section>
          )}
        </>
      )}

      {events.length > 0 && (
        <section className="insights-section" aria-labelledby="insights-events-title">
          <h3 id="insights-events-title" className="insights-section-heading">{tr("Latest events")}</h3>
          <div className="insights-event-list insights-event-list-scroll">
            {events.slice(0, 30).map((event) => (
              <div key={event.id} className="insights-event-row">
                <div className="insights-event-main">
                  <span className="insights-event-type">{event.event_type}</span>
                  <span className="insights-event-category">{event.category}</span>
                </div>
                <span className="insights-event-time">{formatDateTime(event.created_at)}</span>
              </div>
            ))}
          </div>
        </section>
      )}
    </div>
  )
}
