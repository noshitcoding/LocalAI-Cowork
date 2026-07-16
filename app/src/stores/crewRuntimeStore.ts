import { create } from 'zustand'
import { safeInvoke } from '../utils/safeInvoke'

export type CrewRuntimeStatus = {
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

type CrewRuntimeState = {
  status: CrewRuntimeStatus | null
  loading: boolean
  bootstrapping: boolean
  error: string | null
  loadStatus: () => Promise<void>
  bootstrap: (forceReinstall?: boolean) => Promise<void>
  ensureReady: () => Promise<void>
}

export const useCrewRuntimeStore = create<CrewRuntimeState>()((set) => ({
  status: null,
  loading: false,
  bootstrapping: false,
  error: null,

  loadStatus: async () => {
    set({ loading: true, error: null })
    try {
      const status = await safeInvoke<CrewRuntimeStatus>('crew_runtime_status', undefined, undefined)
      set({ status, loading: false })
    } catch (error) {
      set({ error: error instanceof Error ? error.message : String(error), loading: false })
    }
  },

  bootstrap: async (forceReinstall = false) => {
    set({ bootstrapping: true, error: null })
    try {
      const response = await safeInvoke<CrewRuntimeBootstrapResponse>('crew_runtime_bootstrap', {
        request: {
          forceReinstall,
        },
      }, undefined)
      set({ status: response?.status ?? null, bootstrapping: false })
    } catch (error) {
      set({ error: error instanceof Error ? error.message : String(error), bootstrapping: false })
    }
  },

  ensureReady: async () => {
    set({ loading: true, error: null })
    try {
      const status = await safeInvoke<CrewRuntimeStatus>('crew_runtime_status', undefined, undefined)
      set({ status, loading: false })

      if (!status?.bootstrapRequired) {
        return
      }

      set({ bootstrapping: true, error: null })
      try {
        const response = await safeInvoke<CrewRuntimeBootstrapResponse>('crew_runtime_bootstrap', {
          request: {
            forceReinstall: false,
          },
        }, undefined)
        set({ status: response?.status ?? status, bootstrapping: false })
      } catch (error) {
        set({ status, error: error instanceof Error ? error.message : String(error), bootstrapping: false })
      }
    } catch (error) {
      set({ error: error instanceof Error ? error.message : String(error), loading: false })
    }
  },
}))
