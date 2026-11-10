import type {
  LocalDaemonRuntimeClient,
  LocalDaemonSyncConflict,
  LocalDaemonSyncState,
} from './localDaemonClient'

export type LocalDaemonSyncSnapshot = {
  peerId: string
  state: LocalDaemonSyncState
  conflicts: LocalDaemonSyncConflict[]
}

export function localDaemonSyncPeerId(serverUrl: string, deviceId: string): string {
  const canonicalServerUrl = serverUrl.trim().replace(/\/+$/, '')
  const canonicalDeviceId = deviceId.trim()
  if (!canonicalServerUrl || !canonicalDeviceId) {
    throw new Error('A server URL and local device ID are required for metadata sync')
  }
  return `${canonicalServerUrl}#${canonicalDeviceId}`
}

export async function readLocalDaemonSyncSnapshot(
  client: LocalDaemonRuntimeClient,
  serverUrl: string,
): Promise<LocalDaemonSyncSnapshot> {
  const health = await client.health()
  const peerId = localDaemonSyncPeerId(serverUrl, health.device_id)
  const state = await client.syncState(peerId)
  const conflicts = state.open_conflicts > 0
    ? await client.listSyncConflicts(peerId)
    : []
  return { peerId, state, conflicts }
}
