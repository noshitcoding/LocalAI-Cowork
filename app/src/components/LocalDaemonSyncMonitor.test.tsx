import { act, render } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { useRemoteRuntimeStore } from '../stores/remoteRuntimeStore'
import LocalDaemonSyncMonitor from './LocalDaemonSyncMonitor'

const { reconcileDurableLocalEntities, readLocalDaemonSyncSnapshot } = vi.hoisted(() => ({
  reconcileDurableLocalEntities: vi.fn(async () => undefined),
  readLocalDaemonSyncSnapshot: vi.fn(),
}))

vi.mock('../runtime/localDaemonExecution', () => ({
  createLocalDaemonRuntimeClient: () => ({ health: vi.fn() }),
}))
vi.mock('../runtime/localDaemonEntities', () => ({
  reconcileDurableLocalEntities,
}))
vi.mock('../runtime/localDaemonSync', () => ({
  readLocalDaemonSyncSnapshot,
}))

describe('LocalDaemonSyncMonitor', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.clearAllMocks()
    Object.defineProperty(window, '__TAURI_INTERNALS__', {
      configurable: true,
      value: {},
    })
    useRemoteRuntimeStore.setState({ serverUrl: 'https://cowork.example.test' })
  })

  afterEach(() => {
    vi.useRealTimers()
    delete (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__
  })

  it('reconciles once per newly downloaded remote cursor', async () => {
    readLocalDaemonSyncSnapshot
      .mockResolvedValueOnce({ state: { remote_cursor: 7 } })
      .mockResolvedValueOnce({ state: { remote_cursor: 7 } })
      .mockResolvedValueOnce({ state: { remote_cursor: 8 } })

    const view = render(<LocalDaemonSyncMonitor />)
    await act(async () => { await Promise.resolve() })
    expect(reconcileDurableLocalEntities).toHaveBeenCalledTimes(1)

    await act(async () => { await vi.advanceTimersByTimeAsync(2_000) })
    expect(reconcileDurableLocalEntities).toHaveBeenCalledTimes(1)

    await act(async () => { await vi.advanceTimersByTimeAsync(2_000) })
    expect(reconcileDurableLocalEntities).toHaveBeenCalledTimes(2)
    view.unmount()
  })
})
