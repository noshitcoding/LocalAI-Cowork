import { useCallback, useEffect, useMemo, useState } from 'react'
import type { SessionSummary } from '../engine'
import { useChatStore } from '../stores/chatStore'
import { useEngineStore } from '../stores/engineStore'
import { resolveSessionRecord, toChatThread } from '../utils/sessionThreads'
import i18n, { tr } from '../i18n'

type SessionLike = Partial<SessionSummary> & { id?: unknown }

const toNumber = (value: unknown): number => {
  if (typeof value === 'number' && Number.isFinite(value)) return value
  if (typeof value === 'string' && value.trim()) {
    const parsed = Number(value)
    if (Number.isFinite(parsed)) return parsed
  }
  return Date.now()
}

const normalizeSessionSummary = (value: unknown): SessionSummary | null => {
  if (!value || typeof value !== 'object') return null
  const session = value as SessionLike
  const id = typeof session.id === 'string' ? session.id : ''
  if (!id) return null

  return {
    id,
    title: typeof session.title === 'string' && session.title.trim() ? session.title : tr('Untitled session'),
    cwd: typeof session.cwd === 'string' ? session.cwd : '',
    messageCount: typeof session.messageCount === 'number' && Number.isFinite(session.messageCount)
      ? session.messageCount
      : 0,
    createdAt: toNumber(session.createdAt),
    updatedAt: toNumber(session.updatedAt),
  }
}

const normalizeSessions = (values: unknown): SessionSummary[] => {
  if (!Array.isArray(values)) return []
  return values
    .map(normalizeSessionSummary)
    .filter((session): session is SessionSummary => session !== null)
}

export default function SessionSearchPanel() {
  const getSessions = useEngineStore((s) => s.getSessions)
  const deleteSessionById = useEngineStore((s) => s.deleteSessionById)
  const loadSessionById = useEngineStore((s) => s.loadSessionById)
  const currentSessionId = useEngineStore((s) => s.currentSessionId)
  const hydrateThread = useChatStore((s) => s.hydrateThread)
  const [query, setQuery] = useState('')
  const [sessions, setSessions] = useState<SessionSummary[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [busySessionId, setBusySessionId] = useState<string | null>(null)

  const refreshSessions = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const result = await getSessions()
      setSessions(normalizeSessions(result))
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      setSessions([])
    } finally {
      setLoading(false)
    }
  }, [getSessions])

  useEffect(() => {
    void refreshSessions()
  }, [refreshSessions])

  const filteredSessions = useMemo(() => {
    const trimmed = query.trim().toLowerCase()
    if (!trimmed) return sessions
    return sessions.filter((session) =>
      `${session.title} ${session.cwd}`.toLowerCase().includes(trimmed),
    )
  }, [query, sessions])

  const handleLoadSession = async (sessionId: string) => {
    setBusySessionId(sessionId)
    setError(null)
    try {
      const session = await resolveSessionRecord(sessionId, loadSessionById)
      if (!session) {
        setError(tr("Session could not be loaded."))
        return
      }
      hydrateThread(toChatThread(session))
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusySessionId(null)
    }
  }

  const handleDeleteSession = async (sessionId: string) => {
    setBusySessionId(sessionId)
    setError(null)
    try {
      await deleteSessionById(sessionId)
      setSessions((current) => current.filter((session) => session.id !== sessionId))
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusySessionId(null)
    }
  }

  return (
    <div className="panel session-search-panel">
      <h2>{tr("Sessions")}</h2>

      {error && <p className="session-search-error">{error}</p>}

      <div className="session-search-toolbar">
        <input
          className="session-search-input"
          type="text"
          placeholder={tr("Filter sessions by title or path...")}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <button type="button" className="btn-sm" onClick={() => void refreshSessions()}>{tr("Refresh")}</button>
      </div>

      {query.trim() && (
        <div className="session-search-count">
          {tr("Matching sessions")}: {filteredSessions.length} "{query}"
        </div>
      )}

      {loading ? (
        <p className="panel-empty">{tr("Loading...")}</p>
      ) : filteredSessions.length === 0 ? (
        <p className="panel-empty">{query.trim() ? tr('No matching sessions') : tr('No sessions yet')}</p>
      ) : (
        <div className="session-search-list">
          {filteredSessions.map((session) => (
            <div key={session.id} className="card session-search-card">
              <div className="session-search-card-row">
                <div className="session-search-main">
                  <div className="session-search-title">{session.title}</div>
                  <div className="session-search-meta">
                    <span>{new Date(session.updatedAt).toLocaleString(i18n.resolvedLanguage ?? i18n.language ?? 'en')}</span>
                    <span>{session.messageCount} {tr("Messages")}</span>
                    {currentSessionId === session.id && <span className="session-search-active">{tr("loaded as active")}</span>}
                  </div>
                  <div className="session-search-cwd">{session.cwd || tr('No working directory saved')}</div>
                </div>
                <div className="session-search-actions">
                  <button
                    type="button"
                    className="btn-sm"
                    onClick={() => void handleLoadSession(session.id)}
                    disabled={busySessionId === session.id}
                  >{tr("Load")}</button>
                  <button
                    type="button"
                    className="btn-sm"
                    onClick={() => void handleDeleteSession(session.id)}
                    disabled={busySessionId === session.id}
                  >{tr("Delete")}</button>
                </div>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
