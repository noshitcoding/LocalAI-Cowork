import { beforeEach, describe, expect, it, vi } from 'vitest'
import { safeInvoke } from '../utils/safeInvoke'
vi.mock('../utils/safeInvoke', () => ({ safeInvoke: vi.fn() }))
const invoke = vi.mocked(safeInvoke)
const events = vi.hoisted(() => ({ handlers: new Map<string, (event: { payload: { state?: string } }) => void>(), unlisten: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async (name, handler) => {
  events.handlers.set(name, handler)
  return events.unlisten
}) }))

describe('Crew runtime request controller', () => {
  beforeEach(() => { vi.resetModules(); invoke.mockReset(); events.handlers.clear(); events.unlisten.mockReset() })

  it('coalesces parallel status requests and does not retry errors automatically', async () => {
    const { useCrewRuntimeStore } = await import('./crewRuntimeStore')
    let reject!: (error: Error) => void
    invoke.mockImplementation(() => new Promise((_, failure) => { reject = failure }))
    const one = useCrewRuntimeStore.getState().loadStatus()
    const two = useCrewRuntimeStore.getState().loadStatus()
    expect(one).toBe(two)
    reject(new Error('CREW_MANIFEST_UNREADABLE'))
    await one
    expect(invoke).toHaveBeenCalledTimes(1)
    expect(useCrewRuntimeStore.getState().snapshot.state).toBe('error')
    expect(useCrewRuntimeStore.getState().loading).toBe(false)
  })

  it('automatically initializes only once after setup-required status', async () => {
    const { useCrewRuntimeStore } = await import('./crewRuntimeStore')
    invoke.mockResolvedValueOnce({ ready: false, bootstrapRequired: true })
      .mockRejectedValueOnce(new Error('offline initialization failed'))
    await Promise.all([useCrewRuntimeStore.getState().ensureReady(), useCrewRuntimeStore.getState().ensureReady()])
    await useCrewRuntimeStore.getState().ensureReady()
    expect(invoke.mock.calls.map(([command]) => command)).toEqual(['crew_runtime_status', 'crew_runtime_bootstrap'])
    expect(useCrewRuntimeStore.getState().snapshot.state).toBe('error')
  })

  it('ignores an older status result after a new initialization generation', async () => {
    const { useCrewRuntimeStore } = await import('./crewRuntimeStore')
    let finishStatus!: (value: unknown) => void
    invoke.mockImplementationOnce(() => new Promise((resolve) => { finishStatus = resolve }))
      .mockResolvedValueOnce({ ok: true, status: { ready: true, bootstrapRequired: false } })
    const loading = useCrewRuntimeStore.getState().loadStatus()
    await useCrewRuntimeStore.getState().bootstrap()
    finishStatus({ ready: false, bootstrapRequired: true })
    await loading
    expect(useCrewRuntimeStore.getState().snapshot.state).toBe('ready')
    expect(useCrewRuntimeStore.getState().status?.ready).toBe(true)
  })

  it('refreshes on stale focus and disposes its app-owned event listeners', async () => {
    const { useCrewRuntimeStore, startCrewRuntimeController } = await import('./crewRuntimeStore')
    const loadStatus = vi.fn().mockResolvedValue(undefined)
    useCrewRuntimeStore.setState({ loadStatus })
    const stop = startCrewRuntimeController()
    await Promise.resolve()
    window.dispatchEvent(new Event('focus'))
    expect(loadStatus).toHaveBeenCalledTimes(1)
    useCrewRuntimeStore.setState((state) => ({ snapshot: { ...state.snapshot, checkedAt: new Date().toISOString() } }))
    window.dispatchEvent(new Event('focus'))
    expect(loadStatus).toHaveBeenCalledTimes(1)
    events.handlers.get('crew-runtime-status-changed')?.({ payload: { state: 'ready' } })
    expect(loadStatus).toHaveBeenCalledTimes(2)
    stop()
    expect(events.unlisten).toHaveBeenCalledTimes(2)
    window.dispatchEvent(new Event('focus'))
    expect(loadStatus).toHaveBeenCalledTimes(2)
  })
})
