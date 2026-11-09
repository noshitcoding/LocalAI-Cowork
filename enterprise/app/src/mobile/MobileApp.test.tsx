import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { DesktopSession, RunRecord } from '../runtime/contracts'
import MobileApp from './MobileApp'
import { mobileCacheBinding, type MobileOfflineState } from './mobileOfflineStore'

const mocks = vi.hoisted(() => ({
  account: {
    status: 'authenticated',
    serverUrl: 'https://server.example.test',
    email: 'user@example.test',
    userId: '10000000-0000-4000-8000-000000000001',
    error: null,
    restore: vi.fn(),
    logout: vi.fn(),
  },
  client: {
    listRuns: vi.fn(),
    listDesktopSessions: vi.fn(),
    cancelRun: vi.fn(),
    subscribeRunEvents: vi.fn(() => () => undefined),
  },
  loadMobileOfflineState: vi.fn(),
}))

vi.mock('../stores/remoteRuntimeStore', () => ({
  remoteDeviceId: () => 'device-1',
  remoteRuntimeClient: () => mocks.client,
  useRemoteRuntimeStore: () => mocks.account,
}))
vi.mock('./mobileSecure', () => ({
  hasMobilePin: vi.fn(async () => true),
  setMobilePin: vi.fn(),
  unlockWithBiometrics: vi.fn(async () => 'unlocked'),
  verifyMobilePin: vi.fn(async () => true),
}))
vi.mock('./mobileOfflineStore', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./mobileOfflineStore')>()
  return {
    ...actual,
    loadMobileOfflineState: mocks.loadMobileOfflineState,
    saveMobileOfflineState: vi.fn(async () => undefined),
  }
})
vi.mock('./mobilePush', () => ({
  androidPushBuildConfigured: () => false,
  consumeAndroidPushEvents: vi.fn(async () => []),
  enableAndroidPush: vi.fn(),
}))
vi.mock('../runtime/oidc', () => ({ oidcEnabled: vi.fn(async () => false) }))
vi.mock('../runtime/nativePasskey', () => ({ nativePasskeyAvailable: () => false }))
vi.mock('../runtime/webauthn', () => ({ webauthnAvailableForOrigin: () => false }))

vi.mock('../components/RemoteDesktopViewer', () => ({
  default: ({ session }: { session: DesktopSession }) => <div>desktop:{session.id}</div>,
}))
vi.mock('../components/RemoteTerminal', () => ({ default: () => null }))
vi.mock('../components/RunArtifactPanel', () => ({ default: () => null }))
vi.mock('../components/RunInterventionPanel', () => ({ default: () => null }))
vi.mock('../components/RemoteRunComposer', () => ({ default: () => null }))
vi.mock('../components/RemoteThreadMessages', () => ({ default: () => null }))
vi.mock('../components/RemoteScheduleManager', () => ({ default: () => null }))
vi.mock('../components/RemoteTaskManager', () => ({ default: () => null }))
vi.mock('../components/RemoteProviderProfileManager', () => ({ default: () => null }))
vi.mock('../components/RemoteOrganizationManager', () => ({ default: () => null }))
vi.mock('../components/RemoteProjectVersionManager', () => ({ default: () => null }))
vi.mock('../components/RemoteSecuritySettings', () => ({ default: () => null }))
vi.mock('../components/RemoteGovernancePanel', () => ({ default: () => null }))
vi.mock('../components/RemoteDeviceSettings', () => ({ default: () => null }))
vi.mock('../components/RemoteMetadataManager', () => ({ default: () => null }))
vi.mock('../components/ServerMcpBindingManager', () => ({ default: () => null }))

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return { promise, resolve, reject }
}

function cache(runs: RunRecord[]): MobileOfflineState {
  return {
    schemaVersion: 2,
    binding: mobileCacheBinding(mocks.account.serverUrl, mocks.account.userId),
    runs, events: {}, messages: {}, outbox: [], updatedAt: '2026-11-01T00:00:00Z',
  }
}

function run(id: string, prompt: string): RunRecord {
  return {
    revision: 1,
    etag: `W/"${id}:1"`,
    state: 'running',
    spec: {
      schema_version: 2,
      id,
      thread_id: `${id}-thread`,
      project_id: '10000000-0000-4000-8000-000000000010',
      project: { id: '10000000-0000-4000-8000-000000000010', revision: 1 },
      project_privacy: 'private_local',
      task: null,
      creator_user_id: '10000000-0000-4000-8000-000000000001',
      executor_target: { kind: 'server_linux', pool_id: null },
      required_capabilities: [],
      input: { prompt },
      model_profile_id: null,
      snapshot_id: null,
      idempotency_key: `key-${id}`,
      created_at: '2026-08-30T12:00:00Z',
    },
    assigned_executor_id: null,
    lease_expires_at: null,
    started_at: '2026-08-30T12:00:01Z',
    finished_at: null,
    result: null,
    error: null,
    updated_at: '2026-08-30T12:00:01Z',
  }
}

function session(id: string, runId: string): DesktopSession {
  return {
    schema_version: 2,
    id,
    run_id: runId,
    executor_id: '10000000-0000-4000-8000-000000000020',
    state: 'user_controlled',
    stream_protocol: 'rfb.binary.v1',
    dimensions: { width: 1280, height: 720, scale_factor: 1 },
    controller_user_id: null,
    created_at: '2026-08-30T12:00:00Z',
    ended_at: null,
  }
}

describe('MobileApp desktop sessions', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.account.status = 'authenticated'
    mocks.account.serverUrl = 'https://server.example.test'
    mocks.account.userId = '10000000-0000-4000-8000-000000000001'
    mocks.loadMobileOfflineState.mockReset()
    mocks.client.listRuns.mockReset()
    mocks.client.cancelRun.mockReset()
    mocks.client.listDesktopSessions.mockResolvedValue([])
    Object.defineProperty(navigator, 'onLine', { configurable: true, value: true })
  })

  it('ignores a desktop-session response from the previously selected run', async () => {
    const runA = run('10000000-0000-4000-8000-0000000000a1', 'Run A')
    const runB = run('10000000-0000-4000-8000-0000000000b1', 'Run B')
    const sessionsA = deferred<DesktopSession[]>()
    const sessionsB = deferred<DesktopSession[]>()
    mocks.loadMobileOfflineState.mockResolvedValue(cache([runA, runB]))
    mocks.client.listRuns.mockResolvedValue([runA, runB])
    mocks.client.listDesktopSessions.mockImplementation((runId: string) => (
      runId === runA.spec.id ? sessionsA.promise : sessionsB.promise
    ))

    render(<MobileApp />)
    await screen.findByText('Run A')
    await waitFor(() => expect(mocks.client.listDesktopSessions).toHaveBeenCalledWith(runA.spec.id))
    fireEvent.click(screen.getByRole('button', { name: 'Back to runs' }))
    fireEvent.click(await screen.findByRole('button', { name: /Run B/ }))
    await waitFor(() => expect(mocks.client.listDesktopSessions).toHaveBeenCalledWith(runB.spec.id))

    sessionsB.resolve([session('session-b', runB.spec.id)])
    expect(await screen.findByText('desktop:session-b')).toBeInTheDocument()

    sessionsA.resolve([session('session-a', runA.spec.id)])
    await waitFor(() => expect(screen.queryByText('desktop:session-a')).not.toBeInTheDocument())
    expect(screen.getByText('desktop:session-b')).toBeInTheDocument()
  })

  it('shows the desktop-session error for the current run', async () => {
    const currentRun = run('10000000-0000-4000-8000-0000000000c1', 'Current run')
    mocks.loadMobileOfflineState.mockResolvedValue(cache([currentRun]))
    mocks.client.listRuns.mockResolvedValue([currentRun])
    mocks.client.listDesktopSessions.mockRejectedValue(new Error('desktop service unavailable'))

    render(<MobileApp />)

    expect(await screen.findByRole('alert')).toHaveTextContent('desktop service unavailable')
  })

  it.each(['account', 'server'] as const)('hides the previous %s data while its replacement cache opens', async (change) => {
    const oldRun = run('10000000-0000-4000-8000-0000000000d1', 'Private previous account run')
    const replacement = deferred<MobileOfflineState>()
    mocks.loadMobileOfflineState.mockResolvedValueOnce(cache([oldRun])).mockReturnValueOnce(replacement.promise)
    mocks.client.listRuns.mockResolvedValue([oldRun])
    const view = render(<MobileApp />)
    await screen.findByText('Private previous account run')
    if (change === 'account') mocks.account.userId = '10000000-0000-4000-8000-000000000002'
    else mocks.account.serverUrl = 'https://other.example.test'
    mocks.client.listRuns.mockResolvedValue([])
    view.rerender(<MobileApp />)
    await waitFor(() => expect(mocks.loadMobileOfflineState).toHaveBeenLastCalledWith(
      mobileCacheBinding(mocks.account.serverUrl, mocks.account.userId),
    ))
    expect(screen.queryByText('Private previous account run')).not.toBeInTheDocument()
    expect(screen.getByRole('status')).toHaveTextContent('Opening account data')
    await act(async () => { replacement.resolve(cache([])) })
    expect(await screen.findByText('No cached runs yet.')).toBeInTheDocument()
    expect(screen.queryByText('Private previous account run')).not.toBeInTheDocument()
  })

  it('ignores a failed cancellation from the previous account after its replacement cache opens', async () => {
    const oldRun = run('10000000-0000-4000-8000-0000000000e1', 'Previous account cancellation')
    const newRun = run('10000000-0000-4000-8000-0000000000e2', 'Current account run')
    const cancellation = deferred<void>()
    mocks.loadMobileOfflineState.mockResolvedValueOnce(cache([oldRun]))
    mocks.client.listRuns.mockResolvedValue([oldRun])
    mocks.client.cancelRun.mockReturnValue(cancellation.promise)
    const view = render(<MobileApp />)
    await screen.findByText('Previous account cancellation')
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(mocks.client.cancelRun).toHaveBeenCalledWith(oldRun.spec.id)
    mocks.account.userId = '10000000-0000-4000-8000-000000000002'
    mocks.loadMobileOfflineState.mockResolvedValue(cache([newRun]))
    mocks.client.listRuns.mockResolvedValue([newRun])
    view.rerender(<MobileApp />)
    await screen.findByText('Current account run')
    await act(async () => { cancellation.reject(new Error('Old account cancellation failed')) })
    expect(screen.getByText('Current account run')).toBeInTheDocument()
    expect(screen.queryByText('Old account cancellation failed')).not.toBeInTheDocument()
    expect(screen.queryByText('Opening account data…')).not.toBeInTheDocument()
  })

  it('ignores a late run-refresh error from the previous account', async () => {
    const oldRun = run('10000000-0000-4000-8000-0000000000f1', 'Previous refresh account')
    const newRun = run('10000000-0000-4000-8000-0000000000f2', 'Current refresh account')
    const refresh = deferred<RunRecord[]>()
    mocks.loadMobileOfflineState.mockResolvedValueOnce(cache([oldRun]))
    mocks.client.listRuns.mockReturnValueOnce(refresh.promise)
    const view = render(<MobileApp />)
    await screen.findByText('Previous refresh account')
    await waitFor(() => expect(mocks.client.listRuns).toHaveBeenCalled())
    mocks.account.userId = '10000000-0000-4000-8000-000000000002'
    mocks.loadMobileOfflineState.mockResolvedValue(cache([newRun]))
    mocks.client.listRuns.mockResolvedValue([newRun])
    view.rerender(<MobileApp />)
    await screen.findByText('Current refresh account')
    await act(async () => { refresh.reject(new Error('Previous account refresh failed')) })
    expect(screen.queryByText('Previous account refresh failed')).not.toBeInTheDocument()
    expect(screen.getByText('Current refresh account')).toBeInTheDocument()
  })

  it.each(['success', 'failure'] as const)('keeps login available after logout and an old refresh %s', async outcome => {
    const currentRun = run('10000000-0000-4000-8000-000000000071', 'Logout refresh account')
    const refresh = deferred<RunRecord[]>()
    mocks.loadMobileOfflineState.mockResolvedValue(cache([currentRun]))
    mocks.client.listRuns.mockReturnValue(refresh.promise)
    const view = render(<MobileApp />)
    await screen.findByText('Logout refresh account')
    await waitFor(() => expect(mocks.client.listRuns).toHaveBeenCalled())
    fireEvent.click(screen.getByRole('button', { name: 'Back to runs' }))
    fireEvent.click(screen.getByRole('button', { name: 'Sign out' }))
    expect(mocks.account.logout).toHaveBeenCalledOnce()
    mocks.account.status = 'signed_out'
    mocks.account.userId = ''
    view.rerender(<MobileApp />)
    const connect = await screen.findByRole('button', { name: 'Connect' })
    expect(connect).toBeEnabled()
    await act(async () => {
      if (outcome === 'success') refresh.resolve([])
      else refresh.reject(new Error('Old signed-out refresh failed'))
    })
    expect(connect).toBeEnabled()
    expect(screen.queryByText('Old signed-out refresh failed')).not.toBeInTheDocument()
  })

  it('flushes the current account outbox while the previous account flush is still pending', async () => {
    const oldRun = run('10000000-0000-4000-8000-000000000061', 'Previous outbox account')
    const newRun = run('10000000-0000-4000-8000-000000000062', 'Current outbox account')
    const oldCancellation = deferred<void>()
    const queued = (runId: string) => ({ id: crypto.randomUUID(), kind: 'cancel_run' as const, runId, attempts: 0, createdAt: '2026-11-01T00:00:00Z' })
    mocks.loadMobileOfflineState.mockResolvedValueOnce({ ...cache([oldRun]), outbox: [queued(oldRun.spec.id)] })
    mocks.client.listRuns.mockResolvedValue([oldRun])
    mocks.client.cancelRun.mockReturnValueOnce(oldCancellation.promise).mockResolvedValue(undefined)
    const view = render(<MobileApp />)
    try {
      await waitFor(() => expect(mocks.client.cancelRun).toHaveBeenCalledWith(oldRun.spec.id))
      mocks.account.userId = '10000000-0000-4000-8000-000000000002'
      mocks.loadMobileOfflineState.mockResolvedValue({ ...cache([newRun]), outbox: [queued(newRun.spec.id)] })
      mocks.client.listRuns.mockResolvedValue([newRun])
      view.rerender(<MobileApp />)
      await screen.findByText('Current outbox account')
      await waitFor(() => expect(mocks.client.cancelRun).toHaveBeenCalledWith(newRun.spec.id))
      await waitFor(() => expect(screen.queryByText('1 queued action')).not.toBeInTheDocument())
    } finally {
      await act(async () => { oldCancellation.resolve() })
    }
  })
})
