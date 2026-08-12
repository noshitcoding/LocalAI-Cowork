import { GitCompareArrows, RefreshCw, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useState } from 'react'

import type {
  LocalDaemonSyncConflict,
  LocalDaemonSyncResolution,
  LocalDaemonSyncState,
} from '../runtime/localDaemonClient'
import { reconcileDurableLocalEntities } from '../runtime/localDaemonEntities'
import { createLocalDaemonRuntimeClient } from '../runtime/localDaemonExecution'
import { readLocalDaemonSyncSnapshot } from '../runtime/localDaemonSync'
import { hasTauriRuntime } from '../utils/safeInvoke'

type Props = { serverUrl: string }

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function entityJson(entity: unknown): string {
  return JSON.stringify(entity, null, 2)
}

export default function LocalSyncConflicts({ serverUrl }: Props) {
  const client = useMemo(() => createLocalDaemonRuntimeClient(), [])
  const [open, setOpen] = useState(false)
  const [peerId, setPeerId] = useState('')
  const [state, setState] = useState<LocalDaemonSyncState | null>(null)
  const [conflicts, setConflicts] = useState<LocalDaemonSyncConflict[]>([])
  const [busyId, setBusyId] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      const snapshot = await readLocalDaemonSyncSnapshot(client, serverUrl)
      setPeerId(snapshot.peerId)
      setState(snapshot.state)
      setConflicts(snapshot.conflicts)
      setError(null)
    } catch (cause) {
      setError(messageOf(cause))
    }
  }, [client, serverUrl])

  useEffect(() => {
    void load()
    const timer = window.setInterval(() => { void load() }, 2_000)
    return () => window.clearInterval(timer)
  }, [load])

  const resolve = async (
    conflict: LocalDaemonSyncConflict,
    resolution: LocalDaemonSyncResolution,
  ) => {
    setBusyId(conflict.id)
    setError(null)
    try {
      await client.resolveSyncConflict(peerId, conflict.id, resolution)
      if (resolution === 'use_remote') await reconcileDurableLocalEntities(client)
      await load()
    } catch (cause) {
      setError(messageOf(cause))
    } finally {
      setBusyId(null)
    }
  }

  if (!hasTauriRuntime()) return null

  if (!open) {
    return (
      <button
        className="ui-button ui-button--secondary ui-button--sm"
        type="button"
        aria-label="Open local sync status"
        onClick={() => setOpen(true)}
      >
        <GitCompareArrows size={14} />
        Local sync{state?.open_conflicts ? ` (${state.open_conflicts})` : ''}
      </button>
    )
  }

  return (
    <section
      className="remote-governance-panel"
      style={{ right: 12 }}
      aria-label="Local sync conflicts"
    >
      <header>
        <div><GitCompareArrows size={16} /><strong>Local metadata sync</strong></div>
        <button type="button" aria-label="Close local sync status" onClick={() => setOpen(false)}><X size={15} /></button>
      </header>
      <div className="remote-section-header">
        <div>
          <h2>{conflicts.length
            ? conflicts.length === 1
              ? '1 conflict needs a decision'
              : `${conflicts.length} conflicts need a decision`
            : 'Device is synchronized'}</h2>
          <p>Uploaded cursor {state?.local_cursor ?? 0}; downloaded cursor {state?.remote_cursor ?? 0}.</p>
        </div>
        <button className="ui-button ui-button--secondary ui-button--sm" type="button" onClick={() => { void load() }}>
          <RefreshCw size={13} /> Refresh
        </button>
      </div>
      {error ? <div className="remote-inline-error" role="alert">{error}</div> : null}
      {conflicts.map((conflict) => (
        <article key={conflict.id} style={{ display: 'grid', gap: 8, borderTop: '1px solid var(--border-color)', paddingTop: 10 }}>
          <div>
            <strong>{conflict.entity_type.replaceAll('_', ' ')}</strong>{' '}
            <code>{conflict.entity_id}</code>
          </div>
          <details>
            <summary>Compare local and server metadata</summary>
            <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(220px, 1fr))', gap: 8 }}>
              <label>Local<pre style={{ maxHeight: 180, overflow: 'auto' }}>{entityJson(conflict.local_entity)}</pre></label>
              <label>Server<pre style={{ maxHeight: 180, overflow: 'auto' }}>{entityJson(conflict.remote_entity)}</pre></label>
            </div>
          </details>
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8 }}>
            <button className="ui-button ui-button--primary ui-button--sm" type="button" disabled={busyId === conflict.id} onClick={() => { void resolve(conflict, 'use_remote') }}>
              Use server version
            </button>
            <button className="ui-button ui-button--secondary ui-button--sm" type="button" disabled={busyId === conflict.id} onClick={() => { void resolve(conflict, 'keep_local') }}>
              Keep local version
            </button>
          </div>
        </article>
      ))}
    </section>
  )
}
