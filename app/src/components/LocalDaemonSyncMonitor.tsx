import { useEffect, useRef } from 'react'

import { createLocalDaemonRuntimeClient } from '../runtime/localDaemonExecution'
import { reconcileDurableLocalEntities } from '../runtime/localDaemonEntities'
import { readLocalDaemonSyncSnapshot } from '../runtime/localDaemonSync'
import { useRemoteRuntimeStore } from '../stores/remoteRuntimeStore'
import { hasTauriRuntime } from '../utils/safeInvoke'

const SYNC_UI_POLL_INTERVAL_MS = 2_000

export default function LocalDaemonSyncMonitor() {
  const serverUrl = useRemoteRuntimeStore((state) => state.serverUrl)
  const lastAppliedRemoteCursor = useRef<number | null>(null)

  useEffect(() => {
    if (!hasTauriRuntime() || !serverUrl) return

    let canceled = false
    let timer: number | undefined
    const client = createLocalDaemonRuntimeClient()

    const schedule = () => {
      if (!canceled) timer = window.setTimeout(() => { void poll() }, SYNC_UI_POLL_INTERVAL_MS)
    }
    const poll = async () => {
      try {
        const snapshot = await readLocalDaemonSyncSnapshot(client, serverUrl)
        if (lastAppliedRemoteCursor.current !== snapshot.state.remote_cursor) {
          await reconcileDurableLocalEntities(client)
          lastAppliedRemoteCursor.current = snapshot.state.remote_cursor
        }
      } catch (error) {
        console.warn('[sync] Local daemon UI reconciliation failed', error)
      } finally {
        schedule()
      }
    }

    lastAppliedRemoteCursor.current = null
    void poll()
    return () => {
      canceled = true
      if (timer !== undefined) window.clearTimeout(timer)
    }
  }, [serverUrl])

  return null
}
