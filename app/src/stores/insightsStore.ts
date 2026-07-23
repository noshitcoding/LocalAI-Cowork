import { create } from 'zustand'
import { safeInvoke } from '../utils/safeInvoke'

export type InsightsEvent = {
  id: string
  event_type: string
  category: string
  value_num: number | null
  value_text: string | null
  run_id: string | null
  thread_id: string | null
  metadata_json: string | null
  created_at: string
}

export type CategoryCount = {
  category: string
  count: number
}

export type EventSummary = {
  eventType: string
  category: string
  valueText: string | null
  createdAt: string
}

export type InsightsSummary = {
  totalEvents: number
  totalChats: number
  totalRuns: number
  totalMessagesSent: number
  totalTokensEst: number
  avgRunDurationMin: number
  topCategories: CategoryCount[]
  recentEvents: EventSummary[]
  skillUsageCount: number
  memoryEntryCount: number
}

type RawRecord = Record<string, unknown>

const asRecord = (value: unknown): RawRecord =>
  value && typeof value === 'object' ? value as RawRecord : {}

const asNumber = (value: unknown, fallback = 0): number =>
  typeof value === 'number' && Number.isFinite(value) ? value : fallback

const asString = (value: unknown, fallback = ''): string =>
  typeof value === 'string' ? value : fallback

const asNullableString = (value: unknown): string | null =>
  typeof value === 'string' ? value : null

const asArray = <T>(value: unknown, mapItem: (item: unknown) => T): T[] =>
  Array.isArray(value) ? value.map(mapItem) : []

const normalizeEvent = (value: unknown): InsightsEvent => {
  const event = asRecord(value)
  return {
    id: asString(event.id),
    event_type: asString(event.event_type ?? event.eventType),
    category: asString(event.category),
    value_num: typeof (event.value_num ?? event.valueNum) === 'number' ? event.value_num as number : null,
    value_text: asNullableString(event.value_text ?? event.valueText),
    run_id: asNullableString(event.run_id ?? event.runId),
    thread_id: asNullableString(event.thread_id ?? event.threadId),
    metadata_json: asNullableString(event.metadata_json ?? event.metadataJson),
    created_at: asString(event.created_at ?? event.createdAt),
  }
}

const normalizeSummaryEvent = (value: unknown): EventSummary => {
  const event = asRecord(value)
  return {
    eventType: asString(event.eventType ?? event.event_type),
    category: asString(event.category),
    valueText: asNullableString(event.valueText ?? event.value_text),
    createdAt: asString(event.createdAt ?? event.created_at),
  }
}

const normalizeCategory = (value: unknown): CategoryCount => {
  const category = asRecord(value)
  return {
    category: asString(category.category),
    count: asNumber(category.count),
  }
}

const normalizeSummary = (value: unknown): InsightsSummary => {
  const summary = asRecord(value)
  return {
    totalEvents: asNumber(summary.totalEvents ?? summary.total_events),
    totalChats: asNumber(summary.totalChats ?? summary.total_chats),
    totalRuns: asNumber(summary.totalRuns ?? summary.total_runs),
    totalMessagesSent: asNumber(summary.totalMessagesSent ?? summary.total_messages_sent),
    totalTokensEst: asNumber(summary.totalTokensEst ?? summary.total_tokens_est),
    avgRunDurationMin: asNumber(summary.avgRunDurationMin ?? summary.avg_run_duration_min),
    topCategories: asArray(summary.topCategories ?? summary.top_categories, normalizeCategory),
    recentEvents: asArray(summary.recentEvents ?? summary.recent_events, normalizeSummaryEvent),
    skillUsageCount: asNumber(summary.skillUsageCount ?? summary.skill_usage_count),
    memoryEntryCount: asNumber(summary.memoryEntryCount ?? summary.memory_entry_count),
  }
}

/* ── Local fallback for insights events ─────────────────────────────── */

const LOCAL_INSIGHTS_KEY = 'open-cowork-insights-events'

function getLocalEvents(): InsightsEvent[] {
  try {
    const raw = localStorage.getItem(LOCAL_INSIGHTS_KEY)
    return raw ? JSON.parse(raw) : []
  } catch { return [] }
}

function addLocalEvent(ev: InsightsEvent): void {
  try {
    const events = getLocalEvents()
    events.unshift(ev)
    localStorage.setItem(LOCAL_INSIGHTS_KEY, JSON.stringify(events.slice(0, 500)))
  } catch { /* noop */ }
}

function buildLocalSummary(): InsightsSummary {
  const events = getLocalEvents()
  const categories = new Map<string, number>()
  for (const ev of events) {
    categories.set(ev.category, (categories.get(ev.category) ?? 0) + 1)
  }
  return {
    totalEvents: events.length,
    totalChats: 0,
    totalRuns: 0,
    totalMessagesSent: events.filter(e => e.event_type === 'message').length,
    totalTokensEst: 0,
    avgRunDurationMin: 0,
    topCategories: Array.from(categories.entries())
      .sort((a, b) => b[1] - a[1])
      .slice(0, 10)
      .map(([category, count]) => ({ category, count })),
    recentEvents: events.slice(0, 10).map(e => ({
      eventType: e.event_type,
      category: e.category,
      valueText: e.value_text,
      createdAt: e.created_at,
    })),
    skillUsageCount: events.filter(e => e.event_type === 'skill_use').length,
    memoryEntryCount: 0,
  }
}

/* ── Store ────────────────────────────────────────────────────────────── */

type InsightsState = {
  events: InsightsEvent[]
  summary: InsightsSummary | null
  loading: boolean
  error: string | null

  loadEvents: (category?: string, limit?: number) => Promise<void>
  recordEvent: (e: {
    eventType: string; category: string
    valueNum?: number; valueText?: string
    runId?: string; threadId?: string; metadataJson?: string
  }) => Promise<string>
  loadSummary: () => Promise<void>
}

export const useInsightsStore = create<InsightsState>()((set) => ({
  events: [],
  summary: null,
  loading: false,
  error: null,

  loadEvents: async (category, limit = 100) => {
    set({ loading: true, error: null })
    try {
      const events = await safeInvoke<unknown[]>('insights_list', {
        category: category ?? null,
        limit,
      }, getLocalEvents().slice(0, limit))
      set({ events: asArray(events, normalizeEvent), loading: false })
    } catch (e) {
      set({ error: String(e), loading: false })
    }
  },

  recordEvent: async (e) => {
    const id = `ev-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
    try {
      const result = await safeInvoke<string>('insights_record', {
        eventType: e.eventType,
        category: e.category,
        valueNum: e.valueNum ?? null,
        valueText: e.valueText ?? null,
        runId: e.runId ?? null,
        threadId: e.threadId ?? null,
        metadataJson: e.metadataJson ?? null,
      }, id)
      return result
    } catch {
      // Fallback: store locally
      addLocalEvent({
        id,
        event_type: e.eventType,
        category: e.category,
        value_num: e.valueNum ?? null,
        value_text: e.valueText ?? null,
        run_id: e.runId ?? null,
        thread_id: e.threadId ?? null,
        metadata_json: e.metadataJson ?? null,
        created_at: new Date().toISOString(),
      })
      return id
    }
  },

  loadSummary: async () => {
    set({ loading: true, error: null })
    try {
      const summary = await safeInvoke<unknown>('insights_summary', undefined, buildLocalSummary())
      set({ summary: normalizeSummary(summary), loading: false })
    } catch (e) {
      set({ error: String(e), loading: false })
    }
  },
}))
