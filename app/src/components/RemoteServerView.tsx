import { Bell, CircleStop, Fingerprint, KeyRound, LogOut, MonitorPlay, RefreshCw, Server, SquareTerminal, WifiOff } from 'lucide-react'
import { lazy, Suspense, useCallback, useEffect, useMemo, useState, type FormEvent } from 'react'

import type { DesktopSession, RunEvent, RunRecord } from '../runtime/contracts'
import { remoteDeviceId, remoteRuntimeClient, useRemoteRuntimeStore } from '../stores/remoteRuntimeStore'
import { enableWebPush } from '../runtime/webPush'
import { nativePasskeyAvailable } from '../runtime/nativePasskey'
import { webauthnAvailableForOrigin } from '../runtime/webauthn'
import { oidcEnabled } from '../runtime/oidc'
import RemoteDesktopViewer from './RemoteDesktopViewer'
import RemoteTerminal from './RemoteTerminal'
import RunArtifactPanel from './RunArtifactPanel'
import RunInterventionPanel from './RunInterventionPanel'
import RemoteRunComposer from './RemoteRunComposer'
import RemoteThreadMessages from './RemoteThreadMessages'
import RemoteScheduleManager from './RemoteScheduleManager'
import RemoteTaskManager from './RemoteTaskManager'
import RemoteProviderProfileManager from './RemoteProviderProfileManager'
import RemoteSecuritySettings from './RemoteSecuritySettings'
import RemoteGovernancePanel from './RemoteGovernancePanel'
import RemoteDeviceSettings from './RemoteDeviceSettings'
import RemoteOrganizationManager from './RemoteOrganizationManager'
import RemoteProjectVersionManager from './RemoteProjectVersionManager'

const LocalSyncConflicts = lazy(() => import('./LocalSyncConflicts'))

const IS_WEB_APP = import.meta.env.MODE === 'web' || import.meta.env.VITE_COWORK_WEB === 'true'

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function runLabel(run: RunRecord): string {
  const input = run.spec.input
  if (typeof input === 'object' && input !== null) {
    const record = input as Record<string, unknown>
    const candidate = record.prompt ?? record.message ?? record.task
    if (typeof candidate === 'string' && candidate.trim()) return candidate.trim().slice(0, 90)
  }
  return `Run ${run.spec.id.slice(0, 8)}`
}

function activeSession(sessions: DesktopSession[]): DesktopSession | undefined {
  return sessions.find((session) => !['ended', 'failed'].includes(session.state))
}

function invitationFromLocation(): string {
  if (typeof window === 'undefined') return ''
  return new URLSearchParams(window.location.search).get('invitation')?.trim() ?? ''
}

export default function RemoteServerView() {
  const account = useRemoteRuntimeStore()
  const [serverUrl, setServerUrl] = useState(account.serverUrl)
  const [email, setEmail] = useState(account.email)
  const [password, setPassword] = useState('')
  const [passwordConfirmation, setPasswordConfirmation] = useState('')
  const [secondFactor, setSecondFactor] = useState('')
  const initialInvitation = useMemo(() => invitationFromLocation(), [])
  const [authMode, setAuthMode] = useState<'login' | 'bootstrap' | 'invitation'>(initialInvitation ? 'invitation' : 'login')
  const [displayName, setDisplayName] = useState('')
  const [bootstrapToken, setBootstrapToken] = useState('')
  const [invitationToken, setInvitationToken] = useState(initialInvitation)
  const [runs, setRuns] = useState<RunRecord[]>([])
  const [selectedRunId, setSelectedRunId] = useState<string | null>(null)
  const [events, setEvents] = useState<RunEvent[]>([])
  const [sessions, setSessions] = useState<DesktopSession[]>([])
  const [loadingRuns, setLoadingRuns] = useState(false)
  const [desktopBusy, setDesktopBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [reloadKey, setReloadKey] = useState(0)
  const [pushEnabled, setPushEnabled] = useState(false)
  const [terminalOpen, setTerminalOpen] = useState(false)
  const [interventionReloadKey, setInterventionReloadKey] = useState(0)
  const [messageReloadKey, setMessageReloadKey] = useState(0)
  const [ssoEnabled, setSsoEnabled] = useState(false)

  const client = useMemo(
    () => account.status === 'authenticated' ? remoteRuntimeClient() : null,
    [account.status],
  )
  const selectedRun = runs.find((run) => run.spec.id === selectedRunId)
  const desktopSession = activeSession(sessions)

  useEffect(() => {
    if (account.serverUrl && account.status === 'signed_out') void account.restore()
    // Restore is intentionally attempted once for the configured account. The
    // action itself coalesces StrictMode and concurrent callers.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])
  useEffect(() => {
    if (!initialInvitation || typeof window === 'undefined') return
    const url = new URL(window.location.href)
    url.searchParams.delete('invitation')
    window.history.replaceState(window.history.state, '', `${url.pathname}${url.search}${url.hash}`)
  }, [initialInvitation])
  useEffect(() => {
    let canceled = false
    const timer = window.setTimeout(() => {
      void oidcEnabled(serverUrl).then((enabled) => { if (!canceled) setSsoEnabled(enabled) })
    }, 250)
    return () => { canceled = true; window.clearTimeout(timer) }
  }, [serverUrl])

  const loadRuns = useCallback(async () => {
    if (!client) return
    setLoadingRuns(true)
    setError(null)
    try {
      const loaded = await client.listRuns(100)
      setRuns(loaded)
      setSelectedRunId((current) => current && loaded.some((run) => run.spec.id === current)
        ? current
        : loaded[0]?.spec.id ?? null)
    } catch (cause) {
      setError(messageOf(cause))
    } finally {
      setLoadingRuns(false)
    }
  }, [client])

  const loadSessions = useCallback(async () => {
    if (!client || !selectedRunId) return
    try {
      setSessions(await client.listDesktopSessions(selectedRunId))
    } catch (cause) {
      setError(messageOf(cause))
    }
  }, [client, selectedRunId])

  useEffect(() => {
    if (!client) return
    void loadRuns()
  }, [client, loadRuns])

  useEffect(() => {
    setEvents([])
    setSessions([])
    setTerminalOpen(false)
    if (!client || !selectedRunId) return
    void loadSessions()
    return client.subscribeRunEvents(selectedRunId, 0, (event) => {
      setEvents((current) => [...current.filter((item) => item.event_id !== event.event_id), event].slice(-200))
      if (event.kind === 'artifact_created') setReloadKey((value) => value + 1)
      if (event.kind === 'completed') setMessageReloadKey((value) => value + 1)
      if (['approval_requested', 'approval_resolved', 'input_requested', 'input_received'].includes(event.kind)) setInterventionReloadKey((value) => value + 1)
      if (event.kind === 'desktop_session_changed') void loadSessions()
      if (event.kind === 'state_changed' || event.kind === 'completed' || event.kind === 'failed') {
        void client.getRun(selectedRunId).then((updated) => {
          setRuns((current) => current.map((run) => run.spec.id === updated.spec.id ? updated : run))
        }).catch(() => undefined)
      }
    }, (eventError) => setError(eventError.message))
  }, [client, loadSessions, selectedRunId])

  const login = async (event: FormEvent) => {
    event.preventDefault()
    setError(null)
    try {
      if (authMode !== 'login' && password !== passwordConfirmation) {
        throw new Error('The password confirmation does not match')
      }
      if (authMode === 'bootstrap') {
        await account.bootstrap(serverUrl, email, displayName, password, bootstrapToken)
      } else if (authMode === 'invitation') {
        await account.acceptInvitation(serverUrl, email, displayName, password, invitationToken)
      } else {
        await account.login(serverUrl, email, password, secondFactor)
      }
      setPassword('')
      setPasswordConfirmation('')
      setSecondFactor('')
      setBootstrapToken('')
      setInvitationToken('')
    } catch (cause) {
      setPassword('')
      setPasswordConfirmation('')
      setSecondFactor('')
      setError(messageOf(cause))
    }
  }

  const loginPasskey = async () => {
    setError(null)
    try { await account.loginPasskey(serverUrl, email) }
    catch (cause) { setError(messageOf(cause)) }
  }

  const loginOidc = async () => {
    setError(null)
    try { await account.loginOidc(serverUrl) }
    catch (cause) { setError(messageOf(cause)) }
  }

  const startDesktop = async () => {
    if (!client || !selectedRunId) return
    setDesktopBusy(true)
    setError(null)
    try {
      await client.startDesktopSession(selectedRunId, { width: 1440, height: 900 })
      await loadSessions()
    } catch (cause) {
      setError(messageOf(cause))
    } finally {
      setDesktopBusy(false)
    }
  }

  const stopDesktop = async () => {
    if (!client || !selectedRunId || !desktopSession) return
    setDesktopBusy(true)
    try {
      await client.stopDesktopSession(selectedRunId, desktopSession.id)
      await loadSessions()
    } catch (cause) {
      setError(messageOf(cause))
    } finally {
      setDesktopBusy(false)
    }
  }

  if (account.status !== 'authenticated' || !client) {
    return (
      <div className="remote-server-view remote-server-login">
        <form className="remote-login-card" onSubmit={login}>
          <div className="remote-login-mark"><Server size={28} /></div>
          <h1>{authMode === 'bootstrap' ? 'Set up the first administrator' : authMode === 'invitation' ? 'Accept an Open Cowork invitation' : 'Connect to Open Cowork Server'}</h1>
          <p>{authMode === 'bootstrap' ? 'Use the one-time bootstrap token configured on the server.' : authMode === 'invitation' ? 'Create your account with the private token sent by an administrator.' : 'Sign in to manage durable runs, desktops, browser sessions, and encrypted artifacts.'}</p>
          <label>{IS_WEB_APP ? 'Workspace server' : 'Server URL'}<input type="url" value={serverUrl} onChange={(event) => setServerUrl(event.target.value)} placeholder="https://cowork.example.com" readOnly={IS_WEB_APP} required /></label>
          <label>Email<input type="email" value={email} onChange={(event) => setEmail(event.target.value)} autoComplete="username" required /></label>
          {authMode !== 'login' ? <label>Display name<input value={displayName} onChange={(event) => setDisplayName(event.target.value)} autoComplete="name" maxLength={200} required /></label> : null}
          <label>Password<input type="password" value={password} onChange={(event) => setPassword(event.target.value)} autoComplete={authMode === 'login' ? 'current-password' : 'new-password'} minLength={authMode === 'login' ? undefined : 12} required /></label>
          {authMode !== 'login' ? <label>Confirm password<input type="password" value={passwordConfirmation} onChange={(event) => setPasswordConfirmation(event.target.value)} autoComplete="new-password" minLength={12} required /></label> : null}
          {authMode === 'login' ? <label>Authenticator or recovery code<input value={secondFactor} onChange={(event) => setSecondFactor(event.target.value)} inputMode="numeric" autoComplete="one-time-code" placeholder="Only if two-factor authentication is enabled" /></label> : null}
          {authMode === 'bootstrap' ? <label>Bootstrap token<input type="password" value={bootstrapToken} onChange={(event) => setBootstrapToken(event.target.value)} autoComplete="off" minLength={16} required /></label> : null}
          {authMode === 'invitation' ? <label>Invitation token<input value={invitationToken} onChange={(event) => setInvitationToken(event.target.value)} autoComplete="off" minLength={32} required /></label> : null}
          {(error || account.error) ? <div className="remote-inline-error" role="alert">{error ?? account.error}</div> : null}
          <button className="ui-button ui-button--primary ui-button--lg" type="submit" disabled={account.status === 'authenticating' || account.status === 'restoring'}>
            {account.status === 'restoring' ? 'Restoring session…' : account.status === 'authenticating' ? 'Working…' : authMode === 'bootstrap' ? 'Create administrator' : authMode === 'invitation' ? 'Create account' : 'Sign in'}
          </button>
          {authMode === 'login' && (webauthnAvailableForOrigin(serverUrl) || nativePasskeyAvailable()) ? <button className="ui-button ui-button--secondary ui-button--lg" type="button" disabled={account.status === 'authenticating' || !serverUrl || !email} onClick={() => { void loginPasskey() }}><Fingerprint size={16} /> Sign in with passkey</button> : null}
          {authMode === 'login' && ssoEnabled ? <button className="ui-button ui-button--secondary ui-button--lg" type="button" disabled={account.status === 'authenticating' || !serverUrl} onClick={() => { void loginOidc() }}><KeyRound size={16} /> Sign in with SSO</button> : null}
          <div className="remote-login-modes"><button type="button" aria-pressed={authMode === 'login'} onClick={() => setAuthMode('login')}>Sign in</button><button type="button" aria-pressed={authMode === 'invitation'} onClick={() => setAuthMode('invitation')}>Use invitation</button><button type="button" aria-pressed={authMode === 'bootstrap'} onClick={() => setAuthMode('bootstrap')}>First admin</button></div>
          {account.status === 'restoring' ? <button className="ui-button ui-button--ghost" type="button" onClick={() => account.clearError()}>Use another account</button> : null}
          <small>{IS_WEB_APP
            ? 'This web app is bound to its canonical origin. The refresh session stays in a secure HttpOnly cookie.'
            : 'HTTPS is required except for a loopback development server. Tokens are never stored in localStorage.'}</small>
        </form>
      </div>
    )
  }

  return (
    <div className="remote-server-view">
      <header className="remote-server-header">
        <div><Server size={20} /><span><strong>{account.serverUrl}</strong><small>{account.email}</small></span></div>
        <div>
          <RemoteOrganizationManager client={client} currentUserId={account.userId ?? ''} />
          <RemoteProjectVersionManager client={client} />
          <RemoteRunComposer client={client} onCreated={(run) => { setRuns((current) => [run, ...current.filter((item) => item.spec.id !== run.spec.id)]); setSelectedRunId(run.spec.id) }} />
          <RemoteTaskManager client={client} onRunCreated={(run) => { setRuns((current) => [run, ...current.filter((item) => item.spec.id !== run.spec.id)]); setSelectedRunId(run.spec.id) }} />
          <RemoteScheduleManager client={client} />
          <RemoteProviderProfileManager client={client} />
          <RemoteSecuritySettings client={client} />
          <RemoteGovernancePanel client={client} currentUserId={account.userId ?? ''} />
          <RemoteDeviceSettings client={client} />
          {!IS_WEB_APP ? (
            <Suspense fallback={null}>
              <LocalSyncConflicts serverUrl={account.serverUrl} />
            </Suspense>
          ) : null}
          <button className="ui-button ui-button--secondary ui-button--sm" type="button" disabled={pushEnabled} onClick={() => { void enableWebPush(client, remoteDeviceId()).then((result) => { if (result !== 'enabled') throw new Error(`WebPush ${result.replaceAll('_', ' ')}`); setPushEnabled(true) }).catch((cause) => setError(messageOf(cause))) }}><Bell size={14} /> {pushEnabled ? 'Notifications on' : 'Enable notifications'}</button>
          <button className="ui-button ui-button--secondary ui-button--sm" type="button" onClick={() => { void loadRuns() }} disabled={loadingRuns}><RefreshCw size={14} /> Refresh</button>
          <button className="ui-button ui-button--ghost ui-button--sm" type="button" onClick={() => { void account.logout() }}><LogOut size={14} /> Sign out</button>
        </div>
      </header>
      {error ? <div className="remote-inline-error remote-page-error" role="alert">{error}</div> : null}
      <div className="remote-runs-layout">
        <aside className="remote-run-list" aria-label="Server runs">
          <header><h2>Runs</h2><span>{runs.length}</span></header>
          {loadingRuns && runs.length === 0 ? <p className="remote-muted">Loading runs…</p> : null}
          {!loadingRuns && runs.length === 0 ? <div className="remote-empty"><WifiOff size={20} /><span>No accessible server runs.</span></div> : null}
          {runs.map((run) => (
            <button type="button" key={run.spec.id} className={selectedRunId === run.spec.id ? 'selected' : ''} onClick={() => setSelectedRunId(run.spec.id)}>
              <span><strong>{runLabel(run)}</strong><small>{new Date(run.spec.created_at).toLocaleString()}</small></span>
              <em className={`remote-run-state state-${run.state}`}>{run.state.replaceAll('_', ' ')}</em>
            </button>
          ))}
        </aside>

        <main className="remote-run-detail">
          {selectedRun ? (
            <>
              <section className="remote-run-summary">
                <div><span className="remote-kicker">{selectedRun.spec.executor_target.kind.replaceAll('_', ' ')}</span><h1>{runLabel(selectedRun)}</h1><code>{selectedRun.spec.id}</code></div>
                <div className="remote-run-summary-actions">
                  {!desktopSession ? <button className="ui-button ui-button--primary" type="button" onClick={() => { void startDesktop() }} disabled={desktopBusy}><MonitorPlay size={15} /> Start GUI desktop</button> : null}
                  {selectedRun.spec.executor_target.kind === 'server_linux' && ['running', 'waiting_approval', 'waiting_input'].includes(selectedRun.state) ? <button className="ui-button ui-button--secondary" type="button" onClick={() => setTerminalOpen((value) => !value)}><SquareTerminal size={15} /> {terminalOpen ? 'Hide terminal' : 'Open terminal'}</button> : null}
                  {!['completed', 'failed', 'canceled', 'expired'].includes(selectedRun.state) ? <button className="ui-button ui-button--danger" type="button" onClick={() => { void client.cancelRun(selectedRun.spec.id).then(loadRuns).catch((cause) => setError(messageOf(cause))) }}><CircleStop size={15} /> Cancel run</button> : null}
                </div>
              </section>
              <RemoteThreadMessages
                client={client}
                threadId={selectedRun.spec.thread_id}
                reloadKey={messageReloadKey}
                replyContext={selectedRun}
                onRunCreated={(run) => {
                  setRuns((current) => [run, ...current.filter((item) => item.spec.id !== run.spec.id)])
                  setSelectedRunId(run.spec.id)
                  setMessageReloadKey((value) => value + 1)
                }}
              />
              <RunInterventionPanel client={client} runId={selectedRun.spec.id} refreshKey={interventionReloadKey} onResolved={loadRuns} />
              {desktopSession ? <RemoteDesktopViewer client={client} runId={selectedRun.spec.id} session={desktopSession} onStop={stopDesktop} onSessionChanged={loadSessions} /> : null}
              {terminalOpen ? <RemoteTerminal client={client} runId={selectedRun.spec.id} onClose={() => setTerminalOpen(false)} /> : null}
              <RunArtifactPanel client={client} runId={selectedRun.spec.id} reloadKey={reloadKey} />
              <section className="remote-event-panel">
                <header className="remote-section-header"><div><h2>Live events</h2><p>Durable events received through the reconnecting stream.</p></div><span>{events.length}</span></header>
                <ol>{events.slice().reverse().map((event) => <li key={event.event_id}><time>{new Date(event.created_at).toLocaleTimeString()}</time><strong>{event.kind.replaceAll('_', ' ')}</strong><code>#{event.sequence}</code></li>)}</ol>
              </section>
            </>
          ) : <div className="remote-empty remote-detail-empty"><Server size={28} /><span>Select a run to inspect it.</span></div>}
        </main>
      </div>
    </div>
  )
}
