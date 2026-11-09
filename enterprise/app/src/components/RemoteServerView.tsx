import { Bell, CircleStop, Fingerprint, KeyRound, LogOut, MonitorPlay, RefreshCw, Server, SquareTerminal, WifiOff } from 'lucide-react'
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState, type FormEvent, type InputHTMLAttributes, type ReactElement, type ReactNode } from 'react'

import { useTranslation } from 'react-i18next'
import { Link, Navigate, useSearchParams } from 'react-router-dom'
import { tr } from '../i18n'
import type { DesktopSession, RunEvent, RunRecord } from '../runtime/contracts'
import { remoteDeviceId, remoteRuntimeClient, remoteTokenProvider, useRemoteRuntimeStore } from '../stores/remoteRuntimeStore'
import { detectServerKind, type ServerKind } from '../runtime/serverConnection'
import ServerProfiles from './ServerProfiles'
import ManagementMfaGate from './ManagementMfaGate'
import SignInDialog from './SignInDialog'
import LoginField from './LoginField'
import { loginFieldHelp } from './loginFieldHelp'
import { useBootstrapAvailability } from './useBootstrapAvailability'
import { enableWebPush } from '../runtime/webPush'
import { nativePasskeyAvailable } from '../runtime/nativePasskey'
import { webauthnAvailableForOrigin } from '../runtime/webauthn'
import { oidcEnabled } from '../runtime/oidc'
import { organisationId } from '../runtime/organisationSso'
import RemoteDesktopViewer from './RemoteDesktopViewer'
import RemoteTerminal from './RemoteTerminal'
import RunArtifactPanel from './RunArtifactPanel'
import RunInterventionPanel from './RunInterventionPanel'
import RemoteRunComposer from './RemoteRunComposer'
import RemoteEnterpriseWorkflows from './RemoteEnterpriseWorkflows'
import RemoteThreadMessages from './RemoteThreadMessages'
import RemoteScheduleManager from './RemoteScheduleManager'
import RemoteTaskManager from './RemoteTaskManager'
import RemoteProviderProfileManager from './RemoteProviderProfileManager'
import RemoteSecuritySettings from './RemoteSecuritySettings'
import RemoteGovernancePanel from './RemoteGovernancePanel'
import RemoteContentAccessNotice from './RemoteContentAccessNotice'
import RemoteContentBrowser from './RemoteContentBrowser'
import RemoteDeviceSettings from './RemoteDeviceSettings'
import RemoteOrganizationManager from './RemoteOrganizationManager'
import RemoteProjectVersionManager from './RemoteProjectVersionManager'
import RemoteMetadataManager from './RemoteMetadataManager'
import ServerMcpBindingManager from './ServerMcpBindingManager'

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
  return tr('Run {{id}}', { id: run.spec.id.slice(0, 8) })
}

function activeSession(sessions: DesktopSession[]): DesktopSession | undefined {
  return sessions.find((session) => !['ended', 'failed'].includes(session.state))
}

function invitationFromLocation(): string {
  if (typeof window === 'undefined') return ''
  return new URLSearchParams(window.location.search).get('invitation')?.trim() ?? ''
}

function PasswordSecondFactorDialog() {
  const account = useRemoteRuntimeStore()
  const { i18n } = useTranslation()
  const de = i18n.language.startsWith('de')
  const [secondFactor, setSecondFactor] = useState('')
  if (!account.pendingPasswordLogin) return null
  const label = tr('Authenticator or recovery code')
  return <SignInDialog labelledBy="remote-signin-factor-title" busy={account.status === 'authenticating'} onCancel={() => { setSecondFactor(''); void account.cancelPasswordLogin() }}>
          <section className="management-mfa-card">
            <header className="management-mfa-header"><div className="management-mfa-mark"><KeyRound size={24} aria-hidden="true" /></div><h1 id="remote-signin-factor-title">{de ? 'Zwei-Faktor-Authentifizierung' : 'Two-factor authentication'}</h1><p>{de ? 'Dein Passwort wurde bestätigt. Gib jetzt den Code aus deiner Authenticator-App oder einen Wiederherstellungscode ein.' : 'Your password was confirmed. Enter the code from your authenticator app or an unused recovery code.'}</p></header>
            <div className="management-mfa-account"><strong>{account.pendingPasswordLogin.email}</strong><span>{account.pendingPasswordLogin.serverUrl}</span></div>
            <form className="management-mfa-form" onSubmit={event => { event.preventDefault(); const value = secondFactor.trim(); if (value && account.status !== 'authenticating') { setSecondFactor(''); void account.completePasswordLogin(value).catch(() => {}) } }}>
              <LoginField label={label} help={de ? 'Gib den aktuellen Code aus deiner Authenticator-App oder einen unbenutzten Wiederherstellungscode ein.' : 'Enter the current code from your authenticator app or an unused recovery code.'} helpButtonLabel={de ? `Hilfe zu ${label}` : `Help for ${label}`}><input value={secondFactor} onChange={event => setSecondFactor(event.target.value)} autoComplete="one-time-code" autoFocus data-signin-focus maxLength={128} required disabled={account.status === 'authenticating'} /></LoginField>
              {account.error && <p className="management-mfa-error" role="alert">{account.error}</p>}
              <button className="ui-button ui-button--primary ui-button--lg" type="submit" disabled={account.status === 'authenticating' || !secondFactor.trim()}>{account.status === 'authenticating' ? tr('Working…') : de ? 'Anmeldung bestätigen' : 'Confirm sign-in'}</button>
              <button className="ui-button ui-button--secondary ui-button--lg" type="button" disabled={account.status === 'authenticating'} onClick={() => { setSecondFactor(''); void account.cancelPasswordLogin() }}>{de ? 'Abbrechen' : 'Cancel'}</button>
            </form>
          </section>
        </SignInDialog>
}

export default function RemoteServerView({ headingLevel = 2, managementContent }: { headingLevel?: 1 | 2; managementContent?: ReactNode }) {
  const { i18n } = useTranslation()
  const account = useRemoteRuntimeStore()
  const [orgParams]=useSearchParams()
  const selectedOrg=organisationId(orgParams.get('organisation'))
  const [discovery, setDiscovery] = useState<{ serverUrl: string; userId: string | null; kind: ServerKind } | null>(null)
  const kind = discovery?.serverUrl === account.serverUrl && discovery?.userId === account.userId ? discovery.kind : null
  const [error, setError] = useState('')
  const [retry, setRetry] = useState(0)
  useEffect(() => {
    setDiscovery(null); setError('')
    if (account.status !== 'authenticated') return
    const abort = new AbortController()
    void detectServerKind(account.serverUrl, remoteTokenProvider(), abort.signal)
      .then(value => { if (!abort.signal.aborted) setDiscovery({ serverUrl: account.serverUrl, userId: account.userId, kind: value }) })
      .catch(cause => { if (!abort.signal.aborted) setError(messageOf(cause)) })
    return () => abort.abort()
  }, [account.status, account.serverUrl, account.userId, retry])
  if (account.status === 'authenticated' && kind === 'workspace' && managementContent) return <Navigate to="/server" replace />
  if (account.status !== 'authenticated') return <RemoteServerContent key={`${account.serverUrl}:${account.email}:${account.userId}`} headingLevel={headingLevel} />
  if (kind === 'workspace') return <ManagementMfaGate key={`${account.serverUrl}:${account.userId}:${selectedOrg??''}`}><RemoteServerContent key={`${account.serverUrl}:${account.email}:${account.userId}`} headingLevel={headingLevel} /></ManagementMfaGate>
  if (kind === 'management') return <ManagementMfaGate key={`${account.serverUrl}:${account.userId}`} securityInHeader>{managementContent ?? <Navigate to={selectedOrg?`/management?organisation=${selectedOrg}`:"/management"} replace />}</ManagementMfaGate>
  const de = i18n.language.startsWith('de')
  return <div className="remote-server-view remote-server-login"><div className="remote-login-card">
    <p role={error ? 'alert' : 'status'}>{error || (de ? 'Serververbindung wird geprüft…' : 'Checking server connection…')}</p>
    {error && <button type="button" onClick={() => setRetry(value => value + 1)}>{de ? 'Erneut versuchen' : 'Retry'}</button>}
    <button type="button" onClick={() => void account.logout()}>{tr('Sign out')}</button>
  </div></div>
}

function RemoteServerContent({ headingLevel }: { headingLevel: 1 | 2 }) {
  const { i18n } = useTranslation()
  const Heading = headingLevel === 1 ? 'h1' : 'h2'
  const account = useRemoteRuntimeStore()
  const [orgParams]=useSearchParams()
  const orgInput=orgParams.get('organisation')??''
  const selectedOrg=organisationId(orgInput)
  const [serverUrl, setServerUrl] = useState(account.serverUrl)
  const bootstrapAvailability = useBootstrapAvailability(serverUrl, account.status !== 'authenticated')
  const bootstrapAvailable = bootstrapAvailability === true
  const bootstrapComplete = bootstrapAvailability === false
  const [email, setEmail] = useState(account.email)
  const [password, setPassword] = useState('')
  const [passwordConfirmation, setPasswordConfirmation] = useState('')
  const initialInvitation = useMemo(() => invitationFromLocation(), [])
  const initialFleet = useMemo(() => IS_WEB_APP ? new URLSearchParams(window.location.hash.slice(1)).get('fleet') : null, [])
  const fleetStarted = useRef(false)
  const [authMode, setAuthMode] = useState<'login' | 'bootstrap' | 'invitation'>(initialInvitation ? 'invitation' : 'login')
  const [displayName, setDisplayName] = useState('')
  const [bootstrapToken, setBootstrapToken] = useState('')
  const [invitationToken, setInvitationToken] = useState(initialInvitation)
  const [runs, setRuns] = useState<RunRecord[]>([])
  const [selectedRunId, setSelectedRunId] = useState<string | null>(null)
  const [events, setEvents] = useState<RunEvent[]>([])
  const [sessions, setSessions] = useState<{ runId: string; items: DesktopSession[] } | null>(null)
  const sessionLoadGenerationRef = useRef(0)
  const selectedRunIdRef = useRef(selectedRunId)
  const [loadingRuns, setLoadingRuns] = useState(false)
  const [desktopBusy, setDesktopBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [streamError, setStreamError] = useState<string | null>(null)
  const [reloadKey, setReloadKey] = useState(0)
  const [pushEnabled, setPushEnabled] = useState(false)
  const [terminalOpen, setTerminalOpen] = useState(false)
  const [interventionReloadKey, setInterventionReloadKey] = useState(0)
  const [messageReloadKey, setMessageReloadKey] = useState(0)
  const [ssoProbe,setSsoProbe]=useState<{serverUrl:string;orgInput:string;enabled:boolean}|null>(null)
  const ssoChecked=ssoProbe?.serverUrl===serverUrl && ssoProbe.orgInput===orgInput
  const ssoEnabled=authMode === 'login' && bootstrapComplete && (!orgInput || !!selectedOrg) && ssoChecked && ssoProbe.enabled
  const [supportReadOnly, setSupportReadOnly] = useState(false)

  useEffect(() => {
    if (!bootstrapAvailable && authMode === 'bootstrap') {
      setAuthMode('login')
      setPassword(''); setPasswordConfirmation(''); setBootstrapToken('')
    }
  }, [bootstrapAvailable, authMode])

  useEffect(() => {
    selectedRunIdRef.current = selectedRunId
  }, [selectedRunId])

  const client = useMemo(
    () => account.status === 'authenticated' ? remoteRuntimeClient() : null,
    [account.status],
  )
  const selectedRun = runs.find((run) => run.spec.id === selectedRunId)
  const desktopSession = sessions?.runId === selectedRunId
    ? activeSession(sessions.items)
    : undefined

  useEffect(() => {
    if (initialFleet) {
      if (fleetStarted.current) return
      fleetStarted.current = true
      const params = new URLSearchParams(window.location.hash.slice(1))
      const fleetEmail = params.get('email') ?? ''
      window.history.replaceState(window.history.state, '', window.location.pathname + window.location.search)
      void account.loginFleet(window.location.origin, fleetEmail, initialFleet).catch(cause => setError(messageOf(cause)))
      return
    }
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
    if (!bootstrapComplete || authMode !== 'login' || (orgInput && !selectedOrg)) return
    let canceled = false
    const timer = window.setTimeout(() => {
      void oidcEnabled(serverUrl,selectedOrg).then((enabled) => { if (!canceled) setSsoProbe({serverUrl,orgInput,enabled}) })
    }, 250)
    return () => { canceled = true; window.clearTimeout(timer) }
  }, [serverUrl,orgInput,selectedOrg,bootstrapComplete,authMode])

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

  const loadSessions = useCallback(async (runId: string) => {
    if (!client) return
    const generation = ++sessionLoadGenerationRef.current
    try {
      const items = await client.listDesktopSessions(runId)
      if (generation !== sessionLoadGenerationRef.current || selectedRunIdRef.current !== runId) return
      setSessions({ runId, items })
    } catch (cause) {
      if (generation !== sessionLoadGenerationRef.current || selectedRunIdRef.current !== runId) return
      setError(messageOf(cause))
    }
  }, [client])

  useEffect(() => {
    if (!client) return
    void loadRuns()
  }, [client, loadRuns])

  useEffect(() => {
    sessionLoadGenerationRef.current += 1
    setEvents([])
    setSessions(null)
    setTerminalOpen(false)
    setStreamError(null)
    if (!client || !selectedRunId) return
    let active = true
    void loadSessions(selectedRunId)
    const unsubscribe = client.subscribeRunEvents(selectedRunId, 0, (event) => {
      if (!active) return
      setEvents((current) => [...current.filter((item) => item.event_id !== event.event_id), event].slice(-200))
      if (event.kind === 'artifact_created') setReloadKey((value) => value + 1)
      if (event.kind === 'completed') setMessageReloadKey((value) => value + 1)
      if (['approval_requested', 'approval_resolved', 'input_requested', 'input_received'].includes(event.kind)) setInterventionReloadKey((value) => value + 1)
      if (event.kind === 'desktop_session_changed') void loadSessions(selectedRunId)
      if (event.kind === 'state_changed' || event.kind === 'completed' || event.kind === 'failed') {
        void client.getRun(selectedRunId).then((updated) => {
          setRuns((current) => current.map((run) => run.spec.id === updated.spec.id ? updated : run))
        }).catch(() => undefined)
      }
    }, (eventError) => { if (active) setStreamError(eventError.message) }, () => { if (active) setStreamError(null) })
    return () => {
      active = false
      sessionLoadGenerationRef.current += 1
      unsubscribe()
    }
  }, [client, loadSessions, selectedRunId])

  const login = async (event: FormEvent) => {
    event.preventDefault()
    setError(null)
    try {
      if (authMode !== 'login' && password !== passwordConfirmation) {
        throw new Error(tr('The password confirmation does not match'))
      }
      if (authMode === 'bootstrap') {
        if (!bootstrapAvailable) throw new Error(i18n.language.startsWith('de') ? 'Die Ersteinrichtung ist für diesen Server nicht verfügbar.' : 'First-administrator setup is not available on this server.')
        await account.bootstrap(serverUrl, email, displayName, password, bootstrapToken)
      } else if (authMode === 'invitation') {
        await account.acceptInvitation(serverUrl, email, displayName, password, invitationToken)
      } else {
        await account.login(serverUrl, email, password)
      }
      setPassword('')
      setPasswordConfirmation('')
      setBootstrapToken('')
      setInvitationToken('')
    } catch (cause) {
      setPassword('')
      setPasswordConfirmation('')
      setError(messageOf(cause))
    }
  }

  const loginPasskey = async () => {
    setError(null)
    try { await account.loginPasskey(serverUrl, email) }
    catch (cause) { setError(messageOf(cause)) }
  }

  const loginOidc = async () => {
    if (!ssoEnabled) return
    setError(null)
    try { await account.loginOidc(serverUrl,selectedOrg) }
    catch (cause) { setError(messageOf(cause)) }
  }

  const startDesktop = async () => {
    if (!client || !selectedRunId) return
    const runId = selectedRunId
    setDesktopBusy(true)
    setError(null)
    try {
      await client.startDesktopSession(runId, { width: 1440, height: 900 })
      await loadSessions(runId)
    } catch (cause) {
      setError(messageOf(cause))
    } finally {
      setDesktopBusy(false)
    }
  }

  const stopDesktop = async () => {
    if (!client || !selectedRunId || !desktopSession) return
    const runId = selectedRunId
    const sessionId = desktopSession.id
    setDesktopBusy(true)
    try {
      await client.stopDesktopSession(runId, sessionId)
      await loadSessions(runId)
    } catch (cause) {
      setError(messageOf(cause))
    } finally {
      setDesktopBusy(false)
    }
  }

  if (account.status !== 'authenticated' || !client) {
    const de = i18n.language.startsWith('de')
    const help = loginFieldHelp[de ? 'de' : 'en']
    const field = (name: keyof typeof loginFieldHelp.en, label: string, input: ReactElement<InputHTMLAttributes<HTMLInputElement>>) => (
      <LoginField key={name} label={label} help={help[name]} helpButtonLabel={de ? `Hilfe zu ${label}` : `Help for ${label}`}>{input}</LoginField>
    )
    return (
      <div className="remote-server-view remote-server-login">
        <form className="remote-login-card" onSubmit={login}>
          <div className="remote-login-mark"><Server size={28} /></div>
          <Heading>{authMode === 'bootstrap' ? tr('Set up the first administrator') : authMode === 'invitation' ? tr('Accept an Open Cowork invitation') : tr('Sign in')}</Heading>
          <p>{authMode === 'bootstrap' ? tr('Use the one-time bootstrap token configured on the server.') : authMode === 'invitation' ? tr('Create your account with the private token sent by an administrator.') : tr('Sign in to manage durable runs, desktops, browser sessions, and encrypted artifacts.')}</p>
          {!IS_WEB_APP && <ServerProfiles serverUrl={serverUrl} email={email} onSelect={(url, profileEmail) => {
            setPassword(''); setBootstrapToken(''); setInvitationToken(''); setAuthMode('login'); setError(null)
            setServerUrl(url); setEmail(profileEmail)
            // An empty entry is only a draft; valid selections invalidate the old session.
            if (url) account.setConnection(url, profileEmail)
          }} />}
          {field('server', IS_WEB_APP ? tr('Workspace server') : tr('Server URL'), <input type="url" value={serverUrl} onChange={(event) => setServerUrl(event.target.value)} placeholder="https://cowork.example.com" readOnly={IS_WEB_APP} required />)}
          {field('email', tr('Email'), <input type="email" value={email} onChange={(event) => setEmail(event.target.value)} autoComplete="username" required />)}
          {authMode === 'login' && orgInput && !bootstrapAvailable && <p role="status">{!selectedOrg ? (de ? 'Dieser Organisations-Anmeldelink ist ungültig. Bitte deinen Administrator um einen neuen Link oder melde dich mit deinem bestehenden Konto an.' : 'This organisation sign-in link is invalid. Ask your administrator for a new link, or sign in with your existing account.') : !ssoChecked ? (de ? 'SSO der Organisation wird geprüft…' : 'Checking organisation SSO…') : !ssoEnabled ? (de ? 'Dein Administrator hat SSO für diese Organisation noch nicht freigeschaltet. Melde dich mit deinem bestehenden Konto an.' : 'Your administrator has not enabled SSO for this organisation. Sign in with your existing account.') : null}</p>}
          {authMode === 'bootstrap' && <p>{de ? 'Erstelle zuerst dein Administratorkonto und richte anschließend die Zwei-Faktor-Authentifizierung ein. Danach kannst du SSO in den Organisationseinstellungen konfigurieren.' : 'Create your administrator account first, then complete two-factor authentication. You can configure SSO in the organisation settings afterwards.'}</p>}
          {authMode !== 'login' ? field('displayName', tr('Display name'), <input value={displayName} onChange={(event) => setDisplayName(event.target.value)} autoComplete="name" maxLength={200} required />) : null}
          {field('password', tr('Password'), <input type="password" value={password} onChange={(event) => setPassword(event.target.value)} autoComplete={authMode === 'login' ? 'current-password' : 'new-password'} minLength={authMode === 'login' ? undefined : 12} required />)}
          {authMode !== 'login' ? field('confirmPassword', tr('Confirm password'), <input type="password" value={passwordConfirmation} onChange={(event) => setPasswordConfirmation(event.target.value)} autoComplete="new-password" minLength={12} required />) : null}
          {authMode === 'bootstrap' ? field('bootstrap', tr('Bootstrap token'), <input type="password" value={bootstrapToken} onChange={(event) => setBootstrapToken(event.target.value)} autoComplete="off" minLength={16} required />) : null}
          {authMode === 'invitation' ? field('invitation', tr('Invitation token'), <input value={invitationToken} onChange={(event) => setInvitationToken(event.target.value)} autoComplete="off" minLength={32} required />) : null}
          {!account.pendingPasswordLogin && (error || account.error) ? <div className="remote-inline-error" role="alert">{error ?? account.error}</div> : null}
          <button className="ui-button ui-button--primary ui-button--lg" type="submit" disabled={account.status === 'authenticating' || account.status === 'restoring'}>
            {account.status === 'restoring' ? tr('Restoring session…') : account.status === 'authenticating' ? tr('Working…') : authMode === 'bootstrap' ? tr('Create administrator') : authMode === 'invitation' ? tr('Create account') : tr('Sign in')}
          </button>
          {authMode === 'login' && (webauthnAvailableForOrigin(serverUrl) || nativePasskeyAvailable()) ? <button className="ui-button ui-button--secondary ui-button--lg" type="button" disabled={account.status === 'authenticating' || !serverUrl || !email} onClick={() => { void loginPasskey() }}><Fingerprint size={16} /> {tr('Sign in with passkey')}</button> : null}
          {authMode === 'login' && ssoEnabled ? <button className="ui-button ui-button--secondary ui-button--lg" type="button" disabled={account.status === 'authenticating' || !serverUrl} onClick={() => { void loginOidc() }}><KeyRound size={16} /> {tr('Sign in with SSO')}</button> : null}
          <div className="remote-login-modes"><button type="button" aria-pressed={authMode === 'login'} onClick={() => setAuthMode('login')}>{tr('Sign in')}</button><button type="button" aria-pressed={authMode === 'invitation'} onClick={() => setAuthMode('invitation')}>{tr('Use invitation')}</button>{bootstrapAvailable && <button type="button" aria-pressed={authMode === 'bootstrap'} onClick={() => setAuthMode('bootstrap')}>{tr('First admin')}</button>}</div>
          {account.status === 'restoring' ? <button className="ui-button ui-button--ghost" type="button" onClick={() => account.clearError()}>{tr('Use another account')}</button> : null}
          <Link to="/join">{i18n.language.startsWith('de') ? 'Organisationseinladung öffnen' : 'Open organisation invitation'}</Link>
          <small>{IS_WEB_APP
            ? tr('This web app is bound to its canonical origin. The refresh session stays in a secure HttpOnly cookie.')
            : tr('HTTPS is required except for a loopback development server. Tokens are never stored in localStorage.')}</small>
        </form>
        {account.pendingPasswordLogin && <PasswordSecondFactorDialog key={account.pendingPasswordLogin.expiresAt} />}
      </div>
    )
  }

  return (
    <div className="remote-server-view">
      <header className="remote-server-header">
        {headingLevel === 1 && !selectedRun && <h1>{tr('Server')}</h1>}
        <div><Server size={20} /><span><strong>{account.serverUrl}</strong><small>{account.email}</small></span></div>
        <nav className="remote-server-actions" aria-label={tr('Server actions')}>
          {!supportReadOnly && <RemoteRunComposer client={client} onCreated={(run) => { setRuns((current) => [run, ...current.filter((item) => item.spec.id !== run.spec.id)]); setSelectedRunId(run.spec.id) }} />}
          <details className="remote-server-tools" onKeyDown={event => { if (event.key === 'Escape' && !event.defaultPrevented) { event.currentTarget.open = false; event.currentTarget.querySelector('summary')?.focus() } }}>
            <summary>{i18n.language.startsWith('de') ? 'Verwaltung & Werkzeuge' : 'Manage & tools'}</summary>
            <div className="remote-server-tools-panel">
              <button className="remote-server-tools-close ui-button ui-button--secondary ui-button--sm" type="button" onClick={event=>{const menu=event.currentTarget.closest('details');if(menu){menu.open=false;menu.querySelector('summary')?.focus()}}}>{i18n.language.startsWith('de')?'Werkzeugmenü schließen':'Close tools menu'}</button>
          {!supportReadOnly && <RemoteOrganizationManager client={client} currentUserId={account.userId ?? ''} />}
          <RemoteContentBrowser client={client} />
          {!supportReadOnly && <><RemoteProjectVersionManager client={client} />
          <RemoteMetadataManager client={client} />
          <ServerMcpBindingManager client={client} />

          <RemoteTaskManager client={client} onRunCreated={(run) => { setRuns((current) => [run, ...current.filter((item) => item.spec.id !== run.spec.id)]); setSelectedRunId(run.spec.id) }} />
          <RemoteScheduleManager client={client} />
          <RemoteProviderProfileManager client={client} />
          <RemoteSecuritySettings client={client} /></>}
          <RemoteGovernancePanel client={client} currentUserId={account.userId ?? ''} />
          <RemoteEnterpriseWorkflows client={client} currentUserId={account.userId ?? ''} />
          {!supportReadOnly && <RemoteDeviceSettings client={client} />}
          {!IS_WEB_APP ? (
            <Suspense fallback={null}>
              <LocalSyncConflicts serverUrl={account.serverUrl} />
            </Suspense>
          ) : null}
            </div>
          </details>
          <button className="ui-button ui-button--secondary ui-button--sm" type="button" disabled={pushEnabled} onClick={() => { void enableWebPush(client, remoteDeviceId()).then((result) => { if (result !== 'enabled') throw new Error(`WebPush ${result.replaceAll('_', ' ')}`); setPushEnabled(true) }).catch((cause) => setError(messageOf(cause))) }}><Bell size={14} /> {pushEnabled ? tr('Notifications on') : tr('Enable notifications')}</button>
          <button className="ui-button ui-button--secondary ui-button--sm" type="button" onClick={() => { void loadRuns() }} disabled={loadingRuns}><RefreshCw size={14} /> {tr('Refresh')}</button>
          <button className="ui-button ui-button--ghost ui-button--sm" type="button" onClick={() => { void account.logout() }}><LogOut size={14} /> {tr('Sign out')}</button>
        </nav>
      </header>
      <RemoteContentAccessNotice client={client} onReadOnly={setSupportReadOnly} />
      {error ? <div className="remote-inline-error remote-page-error" role="alert">{error}</div> : null}
      {streamError ? <div className="remote-inline-error remote-page-error" role="status">{tr('Connection interrupted. Retrying automatically.')} {streamError}</div> : null}
      <div className="remote-runs-layout">
        <aside className="remote-run-list" aria-label={tr('Server runs')}>
          <header><h2>{tr('Runs')}</h2><span>{runs.length}</span></header>
          {loadingRuns && runs.length === 0 ? <p className="remote-muted">{tr('Loading runs…')}</p> : null}
          {!loadingRuns && runs.length === 0 ? <div className="remote-empty"><WifiOff size={20} /><span>{tr('No accessible server runs.')}</span></div> : null}
          {runs.map((run) => (
            <button type="button" key={run.spec.id} className={selectedRunId === run.spec.id ? 'selected' : ''} onClick={() => setSelectedRunId(run.spec.id)}>
              <span><strong>{runLabel(run)}</strong><small>{new Date(run.spec.created_at).toLocaleString()}</small></span>
              <em className={`remote-run-state state-${run.state}`}>{run.state.replaceAll('_', ' ')}</em>
            </button>
          ))}
        </aside>

        <div className="remote-run-detail">
          {selectedRun ? (
            <>
              <section className="remote-run-summary">
                <div><span className="remote-kicker">{selectedRun.spec.executor_target.kind.replaceAll('_', ' ')}</span><Heading>{runLabel(selectedRun)}</Heading><code>{selectedRun.spec.id}</code></div>
                <div className="remote-run-summary-actions">
                  {!supportReadOnly && !desktopSession && ['running', 'waiting_approval', 'waiting_input'].includes(selectedRun.state) ? <button className="ui-button ui-button--primary" type="button" onClick={() => { void startDesktop() }} disabled={desktopBusy}><MonitorPlay size={15} /> {tr('Start GUI desktop')}</button> : null}
                  {!supportReadOnly && selectedRun.spec.executor_target.kind === 'server_linux' && ['running', 'waiting_approval', 'waiting_input'].includes(selectedRun.state) ? <button className="ui-button ui-button--secondary" type="button" onClick={() => setTerminalOpen((value) => !value)}><SquareTerminal size={15} /> {terminalOpen ? tr('Hide terminal') : tr('Open terminal')}</button> : null}
                  {!supportReadOnly && !['completed', 'failed', 'canceled', 'expired'].includes(selectedRun.state) ? <button className="ui-button ui-button--danger" type="button" onClick={() => { void client.cancelRun(selectedRun.spec.id).then(loadRuns).catch((cause) => setError(messageOf(cause))) }}><CircleStop size={15} /> {tr('Cancel run')}</button> : null}
                </div>
              </section>
              {selectedRun.state==='waiting_for_snapshot'&&<p className="remote-snapshot-recovery" role="status">{i18n.language.startsWith('de')?'Dieser ältere Lauf wartet auf ausdrücklich freigegebene Projektdateien. Brich den wartenden Lauf ab und wähle anschließend „Thread fortsetzen“. Dort kannst du Dateien auswählen, ihren Snapshot hochladen oder ausdrücklich ohne Projektdateien starten.':'This older run is waiting for explicitly approved project files. Cancel the waiting run, then choose “Continue thread” to select and upload a snapshot or explicitly run without project files.'}</p>}
              <RemoteThreadMessages
                client={client}
                threadId={selectedRun.spec.thread_id}
                reloadKey={messageReloadKey}
                replyContext={supportReadOnly ? undefined : selectedRun}
                onRunCreated={(run) => {
                  setRuns((current) => [run, ...current.filter((item) => item.spec.id !== run.spec.id)])
                  setSelectedRunId(run.spec.id)
                  setMessageReloadKey((value) => value + 1)
                }}
              />
              {!supportReadOnly && <RunInterventionPanel client={client} runId={selectedRun.spec.id} refreshKey={interventionReloadKey} onResolved={loadRuns} />}
              {!supportReadOnly && desktopSession ? <RemoteDesktopViewer client={client} runId={selectedRun.spec.id} session={desktopSession} onStop={stopDesktop} onSessionChanged={() => loadSessions(selectedRun.spec.id)} /> : null}
              {!supportReadOnly && terminalOpen ? <RemoteTerminal client={client} runId={selectedRun.spec.id} onClose={() => setTerminalOpen(false)} /> : null}
              <RunArtifactPanel client={client} runId={selectedRun.spec.id} reloadKey={reloadKey} />
              <section className="remote-event-panel">
                <header className="remote-section-header"><div><h2>{tr('Live events')}</h2><p>{tr('Durable events received through the reconnecting stream.')}</p></div><span>{events.length}</span></header>
                <ol>{events.slice().reverse().map((event) => <li key={event.event_id}><time>{new Date(event.created_at).toLocaleTimeString()}</time><strong>{event.kind.replaceAll('_', ' ')}</strong><code>#{event.sequence}</code></li>)}</ol>
              </section>
            </>
          ) : <div className="remote-empty remote-detail-empty"><Server size={28} /><span>{tr('Select a run to inspect it.')}</span></div>}
        </div>
      </div>
    </div>
  )
}
