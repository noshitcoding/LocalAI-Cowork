import { create } from 'zustand'
import { safeInvoke } from '../utils/safeInvoke'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

export type CrewRuntimeStatus = {
  supported?: boolean
  ready: boolean
  bootstrapRequired: boolean
  embeddedPythonAvailable: boolean
  crewaiInstalled: boolean
  runtimeRoot: string
  runtimeScriptsPath: string
  requirementsPath: string
  embeddedPythonPath: string | null
  detectedPythonPath: string | null
  venvPythonPath: string | null
  pythonVersion: string | null
  crewaiVersion: string | null
  expectedCrewaiVersion: string | null
  toolDependenciesInstalled: boolean
  runtimeCompatible: boolean
  runtimeSchemaVersion: number | null
  lastBootstrapAt: string | null
  message: string
}

export type CrewRuntimeBootstrapResponse = {
  ok: boolean
  runtimeRoot: string
  venvPythonPath: string | null
  installedRequirements: boolean
  message: string
  status: CrewRuntimeStatus
}

export type CrewRuntimeSnapshot = {
  state: 'unknown' | 'checking' | 'setup_required' | 'installing' | 'ready' | 'error' | 'unsupported'
  reasonCode: string | null
  checkedAt: string | null
  generation: number
  operationId: string | null
  recoverable: boolean
}

type CrewRuntimeState = {
  status: CrewRuntimeStatus | null
  snapshot: CrewRuntimeSnapshot
  loading: boolean
  bootstrapping: boolean
  error: string | null
  loadStatus: () => Promise<void>
  bootstrap: (forceReinstall?: boolean) => Promise<void>
  ensureReady: () => Promise<void>
}

// One app controller owns requests; panels subscribe and never retry failed loads automatically.
let statusRequest: Promise<void> | null = null
let bootstrapRequest: Promise<void> | null = null
let startupRequest: Promise<void> | null = null
let startupAttempted = false
let generation = 0

export const useCrewRuntimeStore = create<CrewRuntimeState>()((set, get) => {
  const settle = (status: CrewRuntimeStatus, requestGeneration: number) => {
    if (requestGeneration !== generation) return
    set({ status, loading: false, bootstrapping: false, error: null, snapshot: {
      state: status.supported === false ? 'unsupported' : status.ready ? 'ready' : 'setup_required',
      reasonCode: status.supported === false ? 'runtime.unsupported' : status.ready ? null : 'runtime.not_ready',
      checkedAt: new Date().toISOString(), generation: requestGeneration, operationId: null, recoverable: true,
    } })
  }
  const fail = (error: unknown, requestGeneration: number) => {
    if (requestGeneration !== generation) return
    set({ error: error instanceof Error ? error.message : String(error), loading: false, bootstrapping: false,
      snapshot: { state: 'error', reasonCode: 'runtime.request_failed', checkedAt: new Date().toISOString(),
        generation: requestGeneration, operationId: null, recoverable: true } })
  }
  return {
  status: null,
  snapshot: { state: 'unknown', reasonCode: null, checkedAt: null, generation: 0, operationId: null, recoverable: true },
  loading: false,
  bootstrapping: false,
  error: null,

  loadStatus: () => {
    if (bootstrapRequest) return bootstrapRequest
    if (statusRequest) return statusRequest
    const currentGeneration = ++generation
    set({ loading: true, error: null, snapshot: { ...get().snapshot, state: 'checking',
      generation: currentGeneration, operationId: `status-${currentGeneration}` } })
    statusRequest = (async () => {
      try {
        const status = await safeInvoke<CrewRuntimeStatus>('crew_runtime_status', undefined, undefined)
        if (!status) throw new Error('Crew runtime status is unavailable.')
        settle(status, currentGeneration)
      } catch (error) { fail(error, currentGeneration) }
      finally { statusRequest = null }
    })()
    return statusRequest
  },

  bootstrap: (forceReinstall = false) => {
    if (bootstrapRequest) return bootstrapRequest
    const currentGeneration = ++generation
    set({ loading: false, bootstrapping: true, error: null, snapshot: { ...get().snapshot, state: 'installing',
      generation: currentGeneration, operationId: `bootstrap-${currentGeneration}` } })
    bootstrapRequest = (async () => {
      try {
        const response = await safeInvoke<CrewRuntimeBootstrapResponse>('crew_runtime_bootstrap', {
          request: { forceReinstall },
        }, undefined)
        if (!response?.ok) throw new Error(response?.message ?? 'Crew runtime initialization failed.')
        settle(response.status, currentGeneration)
      } catch (error) { fail(error, currentGeneration) }
      finally { bootstrapRequest = null }
    })()
    return bootstrapRequest
  },

  ensureReady: () => {
    if (startupRequest) return startupRequest
    if (startupAttempted) return Promise.resolve()
    startupAttempted = true
    startupRequest = (async () => {
      await get().loadStatus()
      if (get().snapshot.state === 'setup_required' && get().status?.bootstrapRequired) {
        await get().bootstrap(false)
      }
    })().finally(() => { startupRequest = null })
    return startupRequest
  },
  }
})

/** App-owned subscription; no interval and no panel-owned retries. */
export function startCrewRuntimeController(): () => void {
  let stopped = false
  const unlisteners: UnlistenFn[] = []
  const register = (event: string, handler: (payload: { state?: string }) => void) => {
    void listen<{ state?: string }>(event, ({ payload }) => {
      if (!stopped) handler(payload)
    }).then((unlisten) => { if (stopped) unlisten(); else unlisteners.push(unlisten) }).catch(() => {})
  }
  register('crew-runtime-bootstrap-progress', ({ state }) => {
    if (state !== 'installing' || bootstrapRequest) return
    useCrewRuntimeStore.setState((current) => ({ bootstrapping: true, snapshot: {
      ...current.snapshot, state: 'installing', generation: ++generation,
      operationId: `external-${generation}`,
    } }))
  })
  register('crew-runtime-status-changed', ({ state }) => {
    if (bootstrapRequest) return // The in-flight command delivers the authoritative response.
    if (state === 'error') {
      useCrewRuntimeStore.setState((current) => ({ bootstrapping: false, loading: false,
        error: 'Crew runtime initialization failed. Retry initialization to inspect the error.',
        snapshot: { ...current.snapshot, state: 'error', reasonCode: 'runtime.bootstrap_failed',
          checkedAt: new Date().toISOString(), generation: ++generation, operationId: null } }))
    } else { void useCrewRuntimeStore.getState().loadStatus() }
  })
  const onFocus = () => {
    const current = useCrewRuntimeStore.getState()
    const checkedAt = current.snapshot.checkedAt ? Date.parse(current.snapshot.checkedAt) : 0
    if (!current.loading && !current.bootstrapping && Date.now() - checkedAt > 60_000) void current.loadStatus()
  }
  window.addEventListener('focus', onFocus)
  return () => {
    stopped = true
    window.removeEventListener('focus', onFocus)
    unlisteners.splice(0).forEach((unlisten) => unlisten())
  }
}
