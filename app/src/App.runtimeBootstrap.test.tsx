import { render, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import App from './App'
import { useCrewRuntimeStore } from './stores/crewRuntimeStore'

const mocks = vi.hoisted(() => ({
  initializeCredentialVault: vi.fn(),
}))

vi.mock('./security/credentialMigration', () => ({
  initializeCredentialVault: mocks.initializeCredentialVault,
}))

vi.mock('./utils/safeInvoke', () => ({
  hasTauriRuntime: () => true,
  safeInvoke: vi.fn(async (_command: string, _args?: unknown, fallback?: unknown) => fallback ?? null),
  safeInvokeVoid: vi.fn(async () => undefined),
}))

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    onCloseRequested: vi.fn(async () => vi.fn()),
    destroy: vi.fn(async () => undefined),
  }),
}))

describe('desktop runtime startup', () => {
  beforeEach(() => {
    window.history.pushState({}, '', '/')
    Object.defineProperty(window, 'matchMedia', {
      configurable: true,
      value: vi.fn().mockImplementation((query: string) => ({
        matches: false,
        media: query,
        onchange: null,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        addListener: vi.fn(),
        removeListener: vi.fn(),
        dispatchEvent: vi.fn(),
      })),
    })
  })

  it('starts Crew runtime provisioning while credential migration is still pending', async () => {
    mocks.initializeCredentialVault.mockReturnValue(new Promise<void>(() => {}))
    const ensureReady = vi.fn().mockResolvedValue(undefined)
    useCrewRuntimeStore.setState({ ensureReady })

    render(<App />)

    await waitFor(() => expect(ensureReady).toHaveBeenCalledTimes(1))
    expect(mocks.initializeCredentialVault).toHaveBeenCalledTimes(1)
  })
})
