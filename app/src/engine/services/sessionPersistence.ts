// ── Session Persistence Service ────────────────────────────────────────────
// Uses the dedicated `sessions` table instead of `chat_threads`.
// Provides: create, end, list, get, delete for engine sessions.

import { invoke } from '@tauri-apps/api/core'
import type { Message, TokenUsage, AppState } from '../types'

// ── Types ──────────────────────────────────────────────────────────────────

export type SessionRecord = {
  id: string
  title: string
  threadId?: string
  cwd: string
  messages: Message[]
  totalUsage: TokenUsage
  totalCostUsd: number
  appState: Partial<AppState>
  createdAt: number
  updatedAt: number
}

export type SessionSummary = {
  id: string
  title: string
  threadId?: string
  cwd?: string
  messageCount: number
  createdAt: number
  updatedAt: number
}

type DbSessionRow = {
  id: string
  thread_id?: string
  title: string
  summary?: string
  model_used?: string
  provider?: string
  personality?: string
  total_messages: number
  total_tokens_est: number
  outcome?: string
  started_at: string
  ended_at?: string
}

function parseDbDate(value: string | undefined): number {
  if (!value) return Date.now()
  const parsed = Date.parse(value)
  return Number.isFinite(parsed) ? parsed : Date.now()
}

function rowToSummary(row: DbSessionRow): SessionSummary {
  return {
    id: row.id,
    title: row.title || 'Untitled Session',
    threadId: row.thread_id,
    cwd: '',
    messageCount: row.total_messages ?? 0,
    createdAt: parseDbDate(row.started_at),
    updatedAt: parseDbDate(row.ended_at ?? row.started_at),
  }
}

// ── Save/Load ──────────────────────────────────────────────────────────────

/**
 * Start a new session in the sessions table.
 */
export async function createSession(params: {
  id: string
  threadId?: string
  title: string
  model?: string
  provider?: string
}): Promise<void> {
  await invoke('session_create', {
    id: params.id,
    threadId: params.threadId ?? null,
    title: params.title,
    modelUsed: params.model ?? null,
    provider: params.provider ?? null,
    personality: null,
  })
}

/**
 * End a session with final stats.
 */
export async function endSession(params: {
  id: string
  summary?: string
  totalMessages?: number
  totalTokensEst?: number
  outcome?: string
}): Promise<void> {
  await invoke('session_end', {
    id: params.id,
    summary: params.summary ?? null,
    totalMessages: params.totalMessages ?? 0,
    totalTokensEst: params.totalTokensEst ?? 0,
    outcome: params.outcome ?? null,
  })
}

/**
 * Load a session row from the database.
 */
export async function loadSession(sessionId: string): Promise<SessionRecord | null> {
  try {
    const row = await invoke<DbSessionRow | null>('session_get', { id: sessionId })
    if (!row) return null
    return {
      id: row.id,
      title: row.title || 'Untitled Session',
      threadId: row.thread_id,
      cwd: '',
      messages: [],
      totalUsage: { input_tokens: 0, output_tokens: 0 },
      totalCostUsd: 0,
      appState: {},
      createdAt: parseDbDate(row.started_at),
      updatedAt: parseDbDate(row.ended_at ?? row.started_at),
    }
  } catch {
    return null
  }
}

/**
 * List all saved sessions.
 */
export async function listSessions(): Promise<SessionSummary[]> {
  try {
    const rows = await invoke<DbSessionRow[]>('session_list', { limit: 100 })
    if (!Array.isArray(rows)) return []
    return rows.map(rowToSummary)
  } catch {
    return []
  }
}

/**
 * Delete a session from the database.
 */
export async function deleteSession(sessionId: string): Promise<void> {
  await invoke('session_delete', { id: sessionId })
}

/**
 * Generate a title for a session from its first user message.
 */
export function generateSessionTitle(messages: Message[]): string {
  const firstUser = messages.find(m => m.type === 'user')
  if (!firstUser) return 'New session'

  const text = firstUser.type === 'user'
    ? firstUser.content
        .filter((b): b is { type: 'text'; text: string } => b.type === 'text')
        .map(b => b.text)
        .join(' ')
    : ''

  if (!text) return 'New session'
  return text.length > 60 ? text.slice(0, 60) + '...' : text
}

/**
 * Auto-save current session after a run completes.
 * Now uses the dedicated sessions table and links to the thread.
 */
export async function autoSaveSession(
  sessionId: string,
  title: string,
  _cwd: string,
  messages: Message[],
  totalUsage: TokenUsage,
  _totalCostUsd: number,
  _appState: Partial<AppState>,
  threadId?: string,
): Promise<void> {
  await endSession({
    id: sessionId,
    summary: title,
    totalMessages: messages.length,
    totalTokensEst: totalUsage.input_tokens + totalUsage.output_tokens,
    outcome: 'completed',
  })
  // If this is the first time we save and a threadId is provided, update the row
  if (threadId) {
    try {
      await invoke('db_update_thread_provider_settings', {
        id: threadId,
        providerSettingsJson: JSON.stringify({ sessionId }),
      })
    } catch {
      // optional: thread may not exist
    }
  }
}
