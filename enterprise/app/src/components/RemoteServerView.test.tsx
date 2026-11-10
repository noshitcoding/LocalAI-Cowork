import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { MemoryRouter } from 'react-router-dom'
import type { ReactNode } from 'react'
import i18n from '../i18n'

import type { DesktopSession, RunRecord } from '../runtime/contracts'

const mocks = vi.hoisted(() => ({
  client: null as unknown as Record<string, ReturnType<typeof vi.fn>>,
  status: 'authenticated',
  kind: 'workspace',
  pendingPasswordLogin: null as null | { serverUrl: string; email: string; expiresAt: string },
  authError: null as string | null,
  login: vi.fn(), loginOidc: vi.fn(), oidcEnabled: vi.fn(), completePasswordLogin: vi.fn(), cancelPasswordLogin: vi.fn(),
  getBootstrapAvailability: vi.fn<(serverUrl: string, signal?: AbortSignal) => Promise<boolean>>(),
}))

vi.mock('../stores/remoteRuntimeStore', () => ({
  remoteDeviceId: () => 'device-1',
  remoteTokenProvider: () => async () => 'token',
  remoteRuntimeClient: () => mocks.client,
  useRemoteRuntimeStore: () => ({
    serverUrl: 'https://cowork.example.test',
    email: 'test@example.com',
    status: mocks.status,
    userId: 'user-1',
    error: mocks.authError,
    pendingPasswordLogin: mocks.pendingPasswordLogin,
    login: mocks.login,
    loginOidc: mocks.loginOidc,
    completePasswordLogin: mocks.completePasswordLogin,
    cancelPasswordLogin: mocks.cancelPasswordLogin,
    restore: vi.fn(),
    logout: vi.fn(),
    clearError: vi.fn(),
  }),
}))
vi.mock('../runtime/serverConnection', () => ({ detectServerKind: async () => mocks.kind }))
vi.mock('../runtime/authBootstrap', () => ({ getBootstrapAvailability: mocks.getBootstrapAvailability }))
vi.mock('../runtime/webPush', () => ({ enableWebPush: vi.fn() }))
vi.mock('../runtime/nativePasskey', () => ({ nativePasskeyAvailable: () => false }))
vi.mock('../runtime/webauthn', () => ({ webauthnAvailableForOrigin: () => false }))
vi.mock('../runtime/oidc', () => ({ oidcEnabled: mocks.oidcEnabled }))
vi.mock('./ManagementMfaGate', () => ({ default: ({ children }: { children: ReactNode }) => children }))
vi.mock('./SignInDialog', () => ({ default: ({ children, labelledBy }: { children: ReactNode; labelledBy: string }) => <div role="dialog" aria-modal="true" aria-labelledby={labelledBy}>{children}</div> }))
vi.mock('./ServerProfiles', () => ({ default: () => null }))

vi.mock('./RemoteDesktopViewer', () => ({
  default: ({ session, onStop }: { session: DesktopSession; onStop: () => void }) => (
    <div data-testid="desktop-session">
      {session.id}
      <button type="button" onClick={onStop}>Stop desktop</button>
    </div>
  ),
}))

vi.mock('./RemoteTerminal', () => ({ default: () => null }))
vi.mock('./RunArtifactPanel', () => ({ default: () => null }))
vi.mock('./RunInterventionPanel', () => ({ default: () => null }))
vi.mock('./RemoteRunComposer', () => ({ default: () => null }))
vi.mock('./RemoteThreadMessages', () => ({ default: () => null }))
vi.mock('./RemoteScheduleManager', () => ({ default: () => null }))
vi.mock('./RemoteTaskManager', () => ({ default: () => null }))
vi.mock('./RemoteProviderProfileManager', () => ({ default: () => null }))
vi.mock('./RemoteSecuritySettings', () => ({ default: () => null }))
vi.mock('./RemoteGovernancePanel', () => ({ default: () => null }))
vi.mock('./RemoteDeviceSettings', () => ({ default: () => null }))
vi.mock('./RemoteOrganizationManager', () => ({ default: () => null }))
vi.mock('./RemoteProjectVersionManager', () => ({ default: () => null }))
vi.mock('./RemoteMetadataManager', () => ({ default: () => null }))
vi.mock('./ServerMcpBindingManager', () => ({ default: () => null }))
vi.mock('./LocalSyncConflicts', () => ({ default: () => null }))

import RemoteServerView from './RemoteServerView'

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => { resolve = done })
  return { promise, resolve }
}

function run(id: string, prompt: string): RunRecord {
  return {
    spec: {
      id,
      thread_id: `thread-${id}`,
      created_at: '2026-08-10T12:00:00Z',
      input: { prompt },
      executor_target: { kind: 'server_linux', pool_id: null },
    },
    state: 'running',
  } as unknown as RunRecord
}

function session(id: string, runId: string): DesktopSession {
  return { id, run_id: runId, state: 'running' } as unknown as DesktopSession
}

beforeEach(async () => {
  mocks.kind = 'workspace'
  mocks.status = 'authenticated'
  mocks.pendingPasswordLogin = null
  mocks.authError = null
  mocks.login.mockReset().mockResolvedValue(undefined)
  mocks.loginOidc.mockReset().mockResolvedValue(undefined)
  mocks.oidcEnabled.mockReset().mockResolvedValue(false)
  mocks.completePasswordLogin.mockReset().mockResolvedValue(undefined)
  mocks.cancelPasswordLogin.mockReset().mockResolvedValue(undefined)
  mocks.getBootstrapAvailability.mockReset().mockResolvedValue(false)
  await i18n.changeLanguage('en')
})

describe('RemoteServerView first administrator setup', () => {
  beforeEach(() => { mocks.status = 'signed_out' })

  it('only shows first-admin setup after a confirmed available response', async () => {
    const availability = deferred<boolean>()
    mocks.getBootstrapAvailability.mockReturnValue(availability.promise)
    render(<RemoteServerView />, { wrapper: MemoryRouter })
    expect(screen.queryByRole('button', { name: 'First admin' })).not.toBeInTheDocument()
    expect(mocks.getBootstrapAvailability).toHaveBeenCalledWith('https://cowork.example.test', expect.any(AbortSignal))
    await act(async () => { availability.resolve(true) })
    expect(await screen.findByRole('button', { name: 'First admin' })).toBeInTheDocument()
  })

  it('keeps only sign-in and invitation modes when an administrator already exists', async () => {
    render(<RemoteServerView />, { wrapper: MemoryRouter })
    await waitFor(() => expect(mocks.getBootstrapAvailability).toHaveBeenCalledTimes(1))
    expect(screen.queryByRole('button', { name: 'First admin' })).not.toBeInTheDocument()
    const modes = screen.getByRole('button', { name: 'Use invitation' }).parentElement!
    expect(within(modes).getAllByRole('button').map(button => button.textContent)).toEqual(['Sign in', 'Use invitation'])
  })

  it('removes setup and returns an open setup form to sign-in when an administrator is created', async () => {
    mocks.getBootstrapAvailability.mockResolvedValueOnce(true).mockResolvedValue(false)
    render(<RemoteServerView />, { wrapper: MemoryRouter })
    fireEvent.click(await screen.findByRole('button', { name: 'First admin' }))
    expect(screen.getByRole('heading', { name: 'Set up the first administrator' })).toBeInTheDocument()
    fireEvent.change(screen.getByLabelText('Password'), { target: { value: 'new-password-123' } })
    fireEvent.change(screen.getByLabelText('Bootstrap token'), { target: { value: 'private-bootstrap-secret' } })
    await act(async () => { window.dispatchEvent(new Event('focus')) })
    await waitFor(() => expect(screen.getByRole('heading', { name: 'Sign in' })).toBeInTheDocument())
    expect(screen.queryByRole('button', { name: 'First admin' })).not.toBeInTheDocument()
    expect(screen.queryByLabelText('Bootstrap token')).not.toBeInTheDocument()
    expect(screen.getByLabelText('Password')).toHaveValue('')
    fireEvent.click(screen.getByRole('button', { name: 'Use invitation' }))
    expect(screen.getByRole('heading', { name: 'Accept an Open Cowork invitation' })).toBeInTheDocument()
  })

  it('does not expose setup when the status request fails', async () => {
    mocks.getBootstrapAvailability.mockRejectedValue(new Error('Server unreachable'))
    render(<RemoteServerView />, { wrapper: MemoryRouter })
    await act(async () => { await Promise.resolve() })
    expect(screen.queryByRole('button', { name: 'First admin' })).not.toBeInTheDocument()
    expect(screen.getByRole('heading', { name: 'Sign in' })).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Use invitation' }))
    expect(screen.getByLabelText('Invitation token')).toBeInTheDocument()
  })

  it('does not reuse availability or a late response after changing servers', async () => {
    const oldServer = deferred<boolean>()
    mocks.getBootstrapAvailability.mockReturnValueOnce(oldServer.promise).mockResolvedValue(false)
    render(<RemoteServerView />, { wrapper: MemoryRouter })
    fireEvent.change(screen.getByLabelText('Server URL'), { target: { value: 'https://other.example.test' } })
    await act(async () => { oldServer.resolve(true) })
    await waitFor(() => expect(mocks.getBootstrapAvailability).toHaveBeenCalledWith('https://other.example.test', expect.any(AbortSignal)))
    expect(screen.queryByRole('button', { name: 'First admin' })).not.toBeInTheDocument()
  })

  it('hides confirmed setup immediately when changing to an unreachable server', async () => {
    mocks.getBootstrapAvailability.mockResolvedValueOnce(true).mockRejectedValue(new Error('Server unreachable'))
    render(<RemoteServerView />, { wrapper: MemoryRouter })
    expect(await screen.findByRole('button', { name: 'First admin' })).toBeInTheDocument()
    fireEvent.change(screen.getByLabelText('Server URL'), { target: { value: 'https://unreachable.example.test' } })
    expect(screen.queryByRole('button', { name: 'First admin' })).not.toBeInTheDocument()
    await act(async () => { await Promise.resolve() })
    expect(screen.queryByRole('button', { name: 'First admin' })).not.toBeInTheDocument()
  })
})

describe('password-first sign-in', () => {
  beforeEach(() => { mocks.status = 'signed_out' })

  it('only submits the password and keeps the factor hidden when sign-in is rejected', async () => {
    mocks.login.mockRejectedValueOnce(new Error('Invalid email or password'))
    render(<RemoteServerView />, { wrapper: MemoryRouter })
    expect(screen.queryByLabelText('Authenticator or recovery code')).not.toBeInTheDocument()
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
    fireEvent.change(screen.getByLabelText('Password'), { target: { value: 'wrong-password' } })
    fireEvent.submit(screen.getByRole('heading', { name: 'Sign in' }).closest('form')!)
    expect(await screen.findByRole('alert')).toHaveTextContent('Invalid email or password')
    expect(mocks.login).toHaveBeenCalledWith('https://cowork.example.test', 'test@example.com', 'wrong-password')
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
    expect(screen.getByLabelText('Password')).toHaveValue('')
  })

  it('opens a factor dialog only for a pending password login, supports recovery codes and cancel', async () => {
    mocks.login.mockImplementation(async () => {
      mocks.status = 'second_factor_required'
      mocks.pendingPasswordLogin = { serverUrl: 'https://cowork.example.test', email: 'test@example.com', expiresAt: '2099-01-01T00:00:00Z' }
    })
    mocks.completePasswordLogin.mockImplementation(() => {
      mocks.authError = 'Code already used'
      return Promise.reject(new Error('Code already used'))
    })
    render(<RemoteServerView />, { wrapper: MemoryRouter })
    fireEvent.change(screen.getByLabelText('Password'), { target: { value: 'correct-password' } })
    fireEvent.submit(screen.getByRole('heading', { name: 'Sign in' }).closest('form')!)
    const dialog = await screen.findByRole('dialog', { name: 'Two-factor authentication' })
    expect(dialog).toHaveAttribute('aria-modal', 'true')
    expect(screen.getByLabelText('Password')).toHaveValue('')
    expect(within(dialog).getByText(/Your password was confirmed/)).toBeInTheDocument()
    const code = within(dialog).getByLabelText('Authenticator or recovery code')
    fireEvent.change(code, { target: { value: 'unused-recovery-code' } })
    fireEvent.click(within(dialog).getByRole('button', { name: 'Confirm sign-in' }))
    expect(mocks.completePasswordLogin).toHaveBeenCalledWith('unused-recovery-code')
    expect(await within(dialog).findByRole('alert')).toHaveTextContent('Code already used')
    expect(code).toHaveValue('')
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }))
    expect(mocks.cancelPasswordLogin).toHaveBeenCalled()
  })
})

describe('administrator-configured SSO', () => {
  const org = '11111111-1111-4111-8111-111111111111'
  beforeEach(() => { mocks.status = 'signed_out' })

  it('keeps SSO configuration out of the normal password login', async () => {
    render(<RemoteServerView />, { wrapper: MemoryRouter })
    await waitFor(() => expect(mocks.oidcEnabled).toHaveBeenCalledWith('https://cowork.example.test', undefined))
    expect(screen.queryByLabelText(/Organisation ID/)).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Sign in with SSO' })).not.toBeInTheDocument()
  })

  it('preserves an explicitly enabled standalone workspace provider', async () => {
    mocks.oidcEnabled.mockResolvedValue(true)
    render(<RemoteServerView />, { wrapper: MemoryRouter })
    fireEvent.click(await screen.findByRole('button', { name: 'Sign in with SSO' }))
    expect(mocks.loginOidc).toHaveBeenCalledWith('https://cowork.example.test', undefined)
  })

  it('offers SSO only for an enabled organisation sign-in link', async () => {
    mocks.oidcEnabled.mockResolvedValue(true)
    render(<MemoryRouter initialEntries={[`/management?organisation=${org}`]}><RemoteServerView /></MemoryRouter>)
    fireEvent.click(await screen.findByRole('button', { name: 'Sign in with SSO' }))
    expect(mocks.oidcEnabled).toHaveBeenCalledWith('https://cowork.example.test', org)
    expect(mocks.loginOidc).toHaveBeenCalledWith('https://cowork.example.test', org)
    expect(screen.queryByLabelText(/Organisation ID/)).not.toBeInTheDocument()
  })

  it('does not offer disabled organisation SSO', async () => {
    render(<MemoryRouter initialEntries={[`/management?organisation=${org}`]}><RemoteServerView /></MemoryRouter>)
    expect(await screen.findByText(/Your administrator has not enabled SSO/)).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Sign in with SSO' })).not.toBeInTheDocument()
  })

  it('keeps SSO hidden during first-admin setup and explains the required order', async () => {
    mocks.getBootstrapAvailability.mockResolvedValue(true)
    mocks.oidcEnabled.mockResolvedValue(true)
    render(<MemoryRouter initialEntries={[`/management?organisation=${org}`]}><RemoteServerView /></MemoryRouter>)
    fireEvent.click(await screen.findByRole('button', { name: 'First admin' }))
    expect(screen.getByText(/Create your administrator account first, then complete two-factor authentication/)).toBeInTheDocument()
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 300)) })
    expect(mocks.oidcEnabled).not.toHaveBeenCalled()
    expect(screen.queryByRole('button', { name: 'Sign in with SSO' })).not.toBeInTheDocument()
  })

  it('rejects an invalid organisation link without falling back to global SSO', async () => {
    mocks.oidcEnabled.mockResolvedValue(true)
    render(<MemoryRouter initialEntries={['/management?organisation=invalid']}><RemoteServerView /></MemoryRouter>)
    expect(screen.getByText(/This organisation sign-in link is invalid/)).toBeInTheDocument()
    expect(mocks.oidcEnabled).not.toHaveBeenCalled()
    expect(screen.queryByRole('button', { name: 'Sign in with SSO' })).not.toBeInTheDocument()
  })

  it('waits for confirmed completed setup before checking organisation SSO', async () => {
    const availability = deferred<boolean>()
    mocks.getBootstrapAvailability.mockReturnValue(availability.promise)
    mocks.oidcEnabled.mockResolvedValue(true)
    render(<MemoryRouter initialEntries={[`/management?organisation=${org}`]}><RemoteServerView /></MemoryRouter>)
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 300)) })
    expect(mocks.oidcEnabled).not.toHaveBeenCalled()
    expect(screen.queryByRole('button', { name: 'Sign in with SSO' })).not.toBeInTheDocument()
    await act(async () => { availability.resolve(false) })
    expect(await screen.findByRole('button', { name: 'Sign in with SSO' })).toBeInTheDocument()
  })

  it('keeps SSO hidden when setup status cannot be verified', async () => {
    mocks.getBootstrapAvailability.mockRejectedValue(new Error('Server unreachable'))
    mocks.oidcEnabled.mockResolvedValue(true)
    render(<MemoryRouter initialEntries={[`/management?organisation=${org}`]}><RemoteServerView /></MemoryRouter>)
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 300)) })
    expect(mocks.oidcEnabled).not.toHaveBeenCalled()
    expect(screen.queryByRole('button', { name: 'Sign in with SSO' })).not.toBeInTheDocument()
  })
})

describe('RemoteServerView field explanations', () => {
  beforeEach(() => { mocks.status = 'signed_out'; mocks.getBootstrapAvailability.mockResolvedValue(true) })

  it.each([
    { mode: 'login', fields: ['Server URL', 'Email', 'Password'] },
    { mode: 'invitation', fields: ['Server URL', 'Email', 'Display name', 'Password', 'Confirm password', 'Invitation token'] },
    { mode: 'bootstrap', fields: ['Server URL', 'Email', 'Display name', 'Password', 'Confirm password', 'Bootstrap token'] },
  ])('provides an accessible explanation for every $mode input', async ({ mode, fields }) => {
    const { container } = render(<RemoteServerView />, { wrapper: MemoryRouter })
    if (mode === 'invitation') fireEvent.click(screen.getByRole('button', { name: 'Use invitation' }))
    if (mode === 'bootstrap') fireEvent.click(await screen.findByRole('button', { name: 'First admin' }))
    expect(container.querySelectorAll('form input')).toHaveLength(fields.length)
    for (const label of fields) {
      const input = screen.getByLabelText(label)
      const info = screen.getByRole('button', { name: `Help for ${label}` })
      expect(info).toHaveAttribute('type', 'button')
      expect(info).toHaveAttribute('aria-expanded', 'false')
      expect(input).not.toHaveAttribute('aria-describedby')
      fireEvent.click(info)
      expect(info).toHaveAttribute('aria-expanded', 'true')
      expect(input).toHaveAttribute('aria-describedby', info.getAttribute('aria-controls'))
      expect(input).toHaveAccessibleDescription(/\S+/)
      fireEvent.keyDown(info, { key: 'Escape' })
      expect(info).toHaveAttribute('aria-expanded', 'false')
      expect(input).not.toHaveAttribute('aria-describedby')
    }
  })

  it('shows German explanations and localized accessible info labels', async () => {
    await i18n.changeLanguage('de')
    render(<RemoteServerView />, { wrapper: MemoryRouter })
    const info = screen.getByRole('button', { name: 'Hilfe zu Passwort' })
    fireEvent.click(info)
    expect(screen.getByLabelText('Passwort')).toHaveAccessibleDescription('Gib dein Kontopasswort ein. Wenn du ein Konto erstellst, wähle ein neues Passwort mit mindestens 12 Zeichen.')
    expect(screen.queryByRole('button', { name: 'Help for Password' })).not.toBeInTheDocument()
  })
})

describe('RemoteServerView desktop session scoping', () => {
  beforeEach(async () => { mocks.kind = 'workspace'; mocks.status = 'authenticated'; await i18n.changeLanguage('en') })

  it('provides an explicit menu close action that returns keyboard focus to its toggle',async()=>{
    mocks.client={getContentAccess:vi.fn(async()=>({managed:false,read_only:false})),listRuns:vi.fn(async()=>[])}
    render(<MemoryRouter><RemoteServerView /></MemoryRouter>)
    const summary=await screen.findByText('Manage & tools')
    fireEvent.click(summary)
    await waitFor(()=>expect(summary.closest('details')).toHaveAttribute('open'))
    fireEvent.click(screen.getByRole('button',{name:'Close tools menu'}))
    expect(summary.closest('details')).not.toHaveAttribute('open')
    expect(summary).toHaveFocus()
  })

  it('uses h2 when embedded and one h1 when standalone, including the localized sign-in form', async () => {
    mocks.status = 'signed_out'
    const { rerender } = render(<RemoteServerView />, { wrapper: MemoryRouter })
    expect(screen.queryByRole('heading', { level: 1 })).not.toBeInTheDocument()
    expect(screen.getByRole('heading', { level: 2, name: 'Sign in' })).toBeInTheDocument()
    rerender(<RemoteServerView headingLevel={1} />)
    expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1)
    await act(async () => { await i18n.changeLanguage('de') })
    await waitFor(() => expect(screen.getByRole('heading', { level: 1, name: 'Anmelden' })).toBeInTheDocument())
    expect(screen.getAllByRole('button', { name: 'Anmelden' })).toHaveLength(2)
  })

  it('clears a recovered stream error without hiding an unrelated action failure', async () => {
    let reportError!: (error: Error) => void
    let connected!: () => void
    mocks.client = {
      getContentAccess: vi.fn(async () => ({ managed: false, read_only: false })),
      listRuns: vi.fn(async () => [run('run-a', 'Run A')]),
      listDesktopSessions: vi.fn(async () => { throw new Error('Desktop access denied') }),
      subscribeRunEvents: vi.fn((_id, _sequence, _event, onError, onConnected) => {
        reportError = onError; connected = onConnected
        return () => undefined
      }),
    }
    render(<MemoryRouter><RemoteServerView /></MemoryRouter>)
    await screen.findByText('Desktop access denied')
    await act(async () => { reportError(new Error('Failed to fetch')) })
    expect(screen.getByRole('status')).toHaveTextContent('Failed to fetch')
    await act(async () => { connected() })
    expect(screen.queryByText(/Failed to fetch/)).not.toBeInTheDocument()
    expect(screen.getByRole('alert')).toHaveTextContent('Desktop access denied')
  })

  it('ignores a late response from run A and stops only run B session state', async () => {
    const runA = run('run-a', 'Run A')
    const runB = run('run-b', 'Run B')
    const sessionsA = deferred<DesktopSession[]>()
    const sessionB = session('session-b', 'run-b')
    const listDesktopSessions = vi.fn((runId: string) => (
      runId === 'run-a' ? sessionsA.promise : Promise.resolve([sessionB])
    ))
    const stopDesktopSession = vi.fn(async () => undefined)
    mocks.client = {
      getContentAccess: vi.fn(async () => ({ managed: false, read_only: false })),
      listRuns: vi.fn(async () => [runA, runB]),
      listDesktopSessions,
      subscribeRunEvents: vi.fn(() => () => undefined),
      stopDesktopSession,
    }

    render(<MemoryRouter><RemoteServerView /></MemoryRouter>)
    await waitFor(() => expect(listDesktopSessions).toHaveBeenCalledWith('run-a'))
    fireEvent.click(screen.getByRole('button', { name: /Run B/ }))
    await waitFor(() => expect(screen.getByTestId('desktop-session')).toHaveTextContent('session-b'))

    sessionsA.resolve([session('session-a', 'run-a')])
    await Promise.resolve()
    expect(screen.getByTestId('desktop-session')).toHaveTextContent('session-b')

    fireEvent.click(screen.getByRole('button', { name: 'Stop desktop' }))
    await waitFor(() => expect(stopDesktopSession).toHaveBeenCalledWith('run-b', 'session-b'))
  })
})
