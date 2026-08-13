import type { Update } from '@tauri-apps/plugin-updater'
import { hasTauriRuntime, safeInvoke } from './safeInvoke'

export type AppUpdatePhase =
  | 'idle'
  | 'unsupported'
  | 'checking'
  | 'up-to-date'
  | 'available'
  | 'backing-up'
  | 'downloading'
  | 'installing'
  | 'restarting'
  | 'error'

export type AppUpdateState = {
  phase: AppUpdatePhase
  currentVersion: string | null
  availableVersion: string | null
  downloadedBytes: number
  contentLength: number | null
  backupPath: string | null
  error: string | null
}

type UpdateBackupResponse = {
  path: string
  databaseBackup: string
  localStorageBackup: string
  itemCount: number
  createdAt: string
}

const initialState: AppUpdateState = {
  phase: 'idle',
  currentVersion: null,
  availableVersion: null,
  downloadedBytes: 0,
  contentLength: null,
  backupPath: null,
  error: null,
}

const listeners = new Set<() => void>()
let state = initialState
let pendingUpdate: Update | null = null
let automaticCheckStarted = false

function setState(patch: Partial<AppUpdateState>): void {
  state = { ...state, ...patch }
  listeners.forEach((listener) => listener())
}

function errorMessage(error: unknown): string {
  if (error instanceof Error && error.message.trim()) return error.message.trim()
  if (typeof error === 'string' && error.trim()) return error.trim()
  return 'Unknown updater error'
}

function localStorageSnapshot(): Record<string, string> {
  const snapshot: Record<string, string> = {}
  if (typeof window === 'undefined') return snapshot

  try {
    for (let index = 0; index < window.localStorage.length; index += 1) {
      const key = window.localStorage.key(index)
      if (!key) continue
      if (!(key.startsWith('open-cowork') || key.startsWith('localai-cowork'))) continue
      if (key === 'open-cowork-providers-local' || key === 'open-cowork-gateway') continue
      const value = window.localStorage.getItem(key)
      if (value !== null) snapshot[key] = value
    }
  } catch {
    // The SQLite backup still protects the workspace if WebView storage is unavailable.
  }

  return snapshot
}

export function subscribeAppUpdater(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

export function getAppUpdateSnapshot(): AppUpdateState {
  return state
}

export async function checkForAppUpdate(): Promise<AppUpdateState> {
  if (!hasTauriRuntime()) {
    setState({ phase: 'unsupported', error: null })
    return state
  }
  if (['checking', 'backing-up', 'downloading', 'installing', 'restarting'].includes(state.phase)) {
    return state
  }

  setState({
    phase: 'checking',
    availableVersion: null,
    downloadedBytes: 0,
    contentLength: null,
    backupPath: null,
    error: null,
  })

  try {
    if (pendingUpdate) {
      await pendingUpdate.close()
      pendingUpdate = null
    }
    const [{ getVersion }, { check }] = await Promise.all([
      import('@tauri-apps/api/app'),
      import('@tauri-apps/plugin-updater'),
    ])
    const currentVersion = await getVersion()
    const update = await check({ timeout: 30_000 })
    pendingUpdate = update
    setState({
      phase: update ? 'available' : 'up-to-date',
      currentVersion,
      availableVersion: update?.version ?? null,
      error: null,
    })
  } catch (error) {
    setState({ phase: 'error', error: errorMessage(error) })
  }

  return state
}

export function startAutomaticUpdateCheck(): void {
  if (automaticCheckStarted) return
  automaticCheckStarted = true
  void checkForAppUpdate()
}

export async function installAvailableAppUpdate(): Promise<AppUpdateState> {
  if (!pendingUpdate || state.phase !== 'available') return checkForAppUpdate()

  const update = pendingUpdate
  setState({
    phase: 'backing-up',
    downloadedBytes: 0,
    contentLength: null,
    backupPath: null,
    error: null,
  })

  try {
    const backup = await safeInvoke<UpdateBackupResponse>('update_backup_create', {
      request: {
        targetVersion: update.version,
        localStorage: localStorageSnapshot(),
      },
    })
    setState({ phase: 'downloading', backupPath: backup.path })

    let downloadedBytes = 0
    let contentLength: number | null = null
    await update.downloadAndInstall((event) => {
      if (event.event === 'Started') {
        contentLength = event.data.contentLength ?? null
        setState({ phase: 'downloading', contentLength, downloadedBytes: 0 })
        return
      }
      if (event.event === 'Progress') {
        downloadedBytes += event.data.chunkLength
        setState({ phase: 'downloading', contentLength, downloadedBytes })
        return
      }
      setState({ phase: 'installing', contentLength, downloadedBytes })
    }, { timeout: 15 * 60_000 })

    setState({ phase: 'restarting' })
    const { relaunch } = await import('@tauri-apps/plugin-process')
    await relaunch()
  } catch (error) {
    setState({ phase: 'error', error: errorMessage(error) })
  }

  return state
}

export function appUpdateProgressPercent(snapshot: AppUpdateState): number | null {
  if (!snapshot.contentLength || snapshot.contentLength <= 0) return null
  return Math.max(0, Math.min(100, Math.round((snapshot.downloadedBytes / snapshot.contentLength) * 100)))
}

export async function resetAppUpdaterForTests(): Promise<void> {
  if (pendingUpdate) await pendingUpdate.close().catch(() => {})
  pendingUpdate = null
  automaticCheckStarted = false
  state = initialState
  listeners.forEach((listener) => listener())
}
