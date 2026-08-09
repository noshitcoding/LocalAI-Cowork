import { create } from 'zustand'
import { useConfigStore, type BackendKind } from './configStore'
import { useEngineStore } from './engineStore'
import { hasTauriRuntime, safeInvoke } from '../utils/safeInvoke'

type BackendDefaultsRow = {
  backend: BackendKind | null
  apiProfileId: string | null
  setupCompleted: boolean
  updatedAt: string
}

type BackendDefaultsState = {
  loaded: boolean
  saving: boolean
  backend: BackendKind | null
  apiProfileId: string | null
  setupCompleted: boolean
  error: string | null
  load: (existingInstallation: boolean) => Promise<void>
  complete: (backend: BackendKind, apiProfileId?: string) => Promise<void>
}

function defaultApiProfileId(): string {
  const config = useConfigStore.getState()
  return config.defaultLlmProfileIds.api
    ?? config.defaultLlmProfileIds.ollama
    ?? config.llmProfiles[0]?.id
    ?? ''
}

function applyDefaults(backend: BackendKind, apiProfileId: string | null): void {
  useEngineStore.getState().setActiveProvider(backend)
  if (apiProfileId && useConfigStore.getState().llmProfiles.some((profile) => profile.id === apiProfileId)) {
    useConfigStore.getState().setDefaultLlmProfile('openai-compatible', apiProfileId)
  }
}

export const useBackendDefaultsStore = create<BackendDefaultsState>((set, get) => ({
  loaded: false,
  saving: false,
  backend: null,
  apiProfileId: null,
  setupCompleted: false,
  error: null,

  load: async (existingInstallation) => {
    if (get().loaded) return
    if (!hasTauriRuntime()) {
      const backend = useEngineStore.getState().activeProvider
      set({ loaded: true, backend, apiProfileId: defaultApiProfileId(), setupCompleted: true })
      return
    }

    try {
      const row = await safeInvoke<BackendDefaultsRow>('backend_defaults_read')
      if (!row.setupCompleted && existingInstallation) {
        const backend = useEngineStore.getState().activeProvider
        await get().complete(backend, defaultApiProfileId())
        return
      }
      if (row.backend) applyDefaults(row.backend, row.apiProfileId)
      set({
        loaded: true,
        backend: row.backend,
        apiProfileId: row.apiProfileId,
        setupCompleted: row.setupCompleted,
        error: null,
      })
    } catch (error) {
      set({
        loaded: true,
        error: error instanceof Error ? error.message : String(error),
      })
    }
  },

  complete: async (backend, selectedProfileId) => {
    const apiProfileId = selectedProfileId?.trim() || defaultApiProfileId()
    set({ saving: true, error: null })
    try {
      await safeInvoke('backend_defaults_write', {
        defaults: {
          backend,
          apiProfileId,
          setupCompleted: true,
          updatedAt: new Date().toISOString(),
        },
      })
      applyDefaults(backend, apiProfileId)
      set({
        loaded: true,
        saving: false,
        backend,
        apiProfileId,
        setupCompleted: true,
      })
    } catch (error) {
      set({
        saving: false,
        error: error instanceof Error ? error.message : String(error),
      })
    }
  },
}))
