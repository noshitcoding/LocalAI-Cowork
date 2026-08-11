import {
  ArrowLeft,
  Bell,
  CircleStop,
  Camera,
  CloudOff,
  Fingerprint,
  KeyRound,
  ListRestart,
  LockKeyhole,
  LogOut,
  MonitorPlay,
  RefreshCw,
  Server,
  SquareTerminal,
  Upload,
} from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState, type ChangeEvent, type FormEvent } from 'react'

import RemoteDesktopViewer from '../components/RemoteDesktopViewer'
import RemoteTerminal from '../components/RemoteTerminal'
import RunArtifactPanel from '../components/RunArtifactPanel'
import RunInterventionPanel from '../components/RunInterventionPanel'
import RemoteRunComposer from '../components/RemoteRunComposer'
import RemoteThreadMessages from '../components/RemoteThreadMessages'
import RemoteScheduleManager from '../components/RemoteScheduleManager'
import RemoteTaskManager from '../components/RemoteTaskManager'
import RemoteProviderProfileManager from '../components/RemoteProviderProfileManager'
import RemoteOrganizationManager from '../components/RemoteOrganizationManager'
import RemoteProjectVersionManager from '../components/RemoteProjectVersionManager'
import RemoteSecuritySettings from '../components/RemoteSecuritySettings'
import RemoteGovernancePanel from '../components/RemoteGovernancePanel'
import RemoteDeviceSettings from '../components/RemoteDeviceSettings'
import type { DesktopSession, RunEvent, RunRecord } from '../runtime/contracts'
import { nativePasskeyAvailable } from '../runtime/nativePasskey'
import { webauthnAvailableForOrigin } from '../runtime/webauthn'
import { oidcEnabled } from '../runtime/oidc'
import { remoteDeviceId, remoteRuntimeClient, useRemoteRuntimeStore } from '../stores/remoteRuntimeStore'
import {
  EMPTY_MOBILE_OFFLINE_STATE,
  createOfflineThreadMessageOperation,
  flushMobileOutbox,
  loadMobileOfflineState,
  saveMobileOfflineState,
  type MobileOfflineState,
} from './mobileOfflineStore'
import { hasMobilePin, setMobilePin, unlockWithBiometrics, verifyMobilePin } from './mobileSecure'
import { androidPushBuildConfigured, consumeAndroidPushEvents, enableAndroidPush } from './mobilePush'
import './mobile.css'

type LockState = 'checking' | 'locked' | 'pin_setup' | 'unlocked'

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function runLabel(run: RunRecord): string {
  const input = run.spec.input as Record<string, unknown> | null
  const label = input && (input.prompt ?? input.message ?? input.task)
  return typeof label === 'string' && label.trim() ? label.trim() : `Run ${run.spec.id.slice(0, 8)}`
}

function terminalState(state: RunRecord['state']): boolean {
  return ['completed', 'failed', 'canceled', 'expired'].includes(state)
}

export default function MobileApp() {
  const account = useRemoteRuntimeStore()
  const [lockState, setLockState] = useState<LockState>('checking')
  const [pin, setPin] = useState('')
  const [pinConfirmation, setPinConfirmation] = useState('')
  const [serverUrl, setServerUrl] = useState(account.serverUrl)
  const [email, setEmail] = useState(account.email)
  const [password, setPassword] = useState('')
  const [accountPasswordConfirmation, setAccountPasswordConfirmation] = useState('')
  const [secondFactor, setSecondFactor] = useState('')
  const [authMode, setAuthMode] = useState<'login' | 'bootstrap' | 'invitation'>('login')
  const [displayName, setDisplayName] = useState('')
  const [bootstrapToken, setBootstrapToken] = useState('')
  const [invitationToken, setInvitationToken] = useState('')
  const [online, setOnline] = useState(navigator.onLine)
  const [offline, setOffline] = useState<MobileOfflineState>(EMPTY_MOBILE_OFFLINE_STATE)
  const [selectedRunId, setSelectedRunId] = useState<string | null>(null)
  const [sessions, setSessions] = useState<DesktopSession[]>([])
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [artifactReloadKey, setArtifactReloadKey] = useState(0)
  const [pushStatus, setPushStatus] = useState<'idle' | 'enabling' | 'enabled'>('idle')
  const [terminalOpen, setTerminalOpen] = useState(false)
  const [interventionReloadKey, setInterventionReloadKey] = useState(0)
  const [messageReloadKey, setMessageReloadKey] = useState(0)
  const [ssoEnabled, setSsoEnabled] = useState(false)
  const [offlineReply, setOfflineReply] = useState('')
  const offlineRef = useRef(offline)
  const fileInput = useRef<HTMLInputElement>(null)
  const cameraInput = useRef<HTMLInputElement>(null)

  const client = useMemo(
    () => account.status === 'authenticated' ? remoteRuntimeClient() : null,
    [account.status],
  )
  const selectedRun = offline.runs.find((run) => run.spec.id === selectedRunId)
  const events = selectedRunId ? offline.events[selectedRunId] ?? [] : []
  const activeSession = sessions.find((session) => !['ended', 'failed'].includes(session.state))

  useEffect(() => {
    let canceled = false
    void (async () => {
      try {
        if (await unlockWithBiometrics() === 'unlocked') {
          if (!canceled) setLockState('unlocked')
          return
        }
      } catch {
        // Biometric cancellation intentionally falls back to the configured app PIN.
      }
      const hasPin = await hasMobilePin()
      if (!canceled) setLockState(hasPin ? 'locked' : 'pin_setup')
    })()
    return () => { canceled = true }
  }, [])
  useEffect(() => {
    let canceled = false
    const timer = window.setTimeout(() => {
      void oidcEnabled(serverUrl).then((enabled) => { if (!canceled) setSsoEnabled(enabled) })
    }, 250)
    return () => { canceled = true; window.clearTimeout(timer) }
  }, [serverUrl])

  useEffect(() => {
    if (lockState !== 'unlocked') return
    void loadMobileOfflineState()
      .then((state) => {
        offlineRef.current = state
        setOffline(state)
        setSelectedRunId(state.runs[0]?.spec.id ?? null)
      })
      .catch((cause) => setError(messageOf(cause)))
    if (account.serverUrl && account.status === 'signed_out') void account.restore()
    const updateOnline = () => setOnline(navigator.onLine)
    window.addEventListener('online', updateOnline)
    window.addEventListener('offline', updateOnline)
    return () => {
      window.removeEventListener('online', updateOnline)
      window.removeEventListener('offline', updateOnline)
    }
    // Account restoration is coalesced in the store and should run only when unlocked.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [lockState])

  const persist = useCallback((next: MobileOfflineState) => {
    offlineRef.current = next
    setOffline(next)
    void saveMobileOfflineState(next).catch((cause) => setError(messageOf(cause)))
  }, [])

  const refreshRuns = useCallback(async () => {
    if (!client || !online) return
    setBusy(true)
    try {
      const runs = await client.listRuns(100)
      const next = { ...offlineRef.current, runs }
      persist(next)
      setSelectedRunId((current) => current && runs.some((run) => run.spec.id === current)
        ? current
        : runs[0]?.spec.id ?? null)
    } catch (cause) {
      setError(messageOf(cause))
    } finally {
      setBusy(false)
    }
  }, [client, online, persist])

  const flushOutbox = useCallback(async () => {
    const current = offlineRef.current
    if (!client || !online || current.outbox.length === 0) return
    const { remaining, createdRuns } = await flushMobileOutbox(client, current.outbox)
    persist({
      ...offlineRef.current,
      runs: [
        ...createdRuns,
        ...offlineRef.current.runs.filter((run) => !createdRuns.some((created) => created.spec.id === run.spec.id)),
      ],
      outbox: remaining,
    })
  }, [client, online, persist])

  useEffect(() => {
    if (!client || !online) return
    void flushOutbox().then(refreshRuns)
  }, [client, flushOutbox, online, refreshRuns])

  useEffect(() => {
    setSessions([])
    setTerminalOpen(false)
    if (!client || !online || !selectedRunId) return
    void client.listDesktopSessions(selectedRunId).then(setSessions).catch(() => undefined)
    const after = Math.max(0, ...(offlineRef.current.events[selectedRunId] ?? []).map((event) => event.sequence))
    return client.subscribeRunEvents(selectedRunId, after, (event) => {
      setOffline((current) => {
        const prior = current.events[selectedRunId] ?? []
        const next = {
          ...current,
          events: {
            ...current.events,
            [selectedRunId]: [...prior.filter((item) => item.event_id !== event.event_id), event]
              .sort((left, right) => left.sequence - right.sequence)
              .slice(-300),
          },
        }
        void saveMobileOfflineState(next)
        offlineRef.current = next
        return next
      })
      if (event.kind === 'desktop_session_changed') {
        void client.listDesktopSessions(selectedRunId).then(setSessions).catch(() => undefined)
      }
      if (['approval_requested', 'approval_resolved', 'input_requested', 'input_received'].includes(event.kind)) setInterventionReloadKey((value) => value + 1)
      if (event.kind === 'completed') setMessageReloadKey((value) => value + 1)
      if (['state_changed', 'completed', 'failed'].includes(event.kind)) void refreshRuns()
    }, () => setOnline(false))
  }, [client, online, refreshRuns, selectedRunId])

  useEffect(() => {
    if (!client || !online || !androidPushBuildConfigured()) return
    void consumeAndroidPushEvents().then((pending) => {
      const newest = pending.sort((left, right) => right.receivedAt - left.receivedAt)[0]
      if (newest) setSelectedRunId(newest.runId)
      if (pending.length > 0) void refreshRuns()
    }).catch(() => undefined)
  }, [client, online, refreshRuns])

  const submitPin = async (event: FormEvent) => {
    event.preventDefault()
    setError(null)
    try {
      if (lockState === 'pin_setup') {
        if (pin !== pinConfirmation) throw new Error('PIN confirmation does not match')
        await setMobilePin(pin)
      } else if (!await verifyMobilePin(pin)) {
        throw new Error('Incorrect PIN')
      }
      setPin('')
      setPinConfirmation('')
      setLockState('unlocked')
    } catch (cause) {
      setPin('')
      setPinConfirmation('')
      setError(messageOf(cause))
    }
  }

  const retryBiometric = async () => {
    try {
      if (await unlockWithBiometrics() === 'unlocked') setLockState('unlocked')
    } catch (cause) {
      setError(messageOf(cause))
    }
  }

  const login = async (event: FormEvent) => {
    event.preventDefault()
    setBusy(true)
    setError(null)
    try {
      if (authMode !== 'login' && password !== accountPasswordConfirmation) {
        throw new Error('Password confirmation does not match')
      }
      if (authMode === 'bootstrap') {
        await account.bootstrap(serverUrl, email, displayName, password, bootstrapToken)
      } else if (authMode === 'invitation') {
        await account.acceptInvitation(serverUrl, email, displayName, password, invitationToken)
      } else {
        await account.login(serverUrl, email, password, secondFactor)
      }
      setPassword('')
      setAccountPasswordConfirmation('')
      setSecondFactor('')
      setBootstrapToken('')
      setInvitationToken('')
    } catch (cause) {
      setPassword('')
      setAccountPasswordConfirmation('')
      setSecondFactor('')
      setError(messageOf(cause))
    } finally {
      setBusy(false)
    }
  }

  const loginPasskey = async () => {
    setBusy(true)
    setError(null)
    try { await account.loginPasskey(serverUrl, email) }
    catch (cause) { setError(messageOf(cause)) }
    finally { setBusy(false) }
  }

  const loginOidc = async () => {
    setBusy(true)
    setError(null)
    try { await account.loginOidc(serverUrl) }
    catch (cause) { setError(messageOf(cause)) }
    finally { setBusy(false) }
  }

  const cancelRun = async () => {
    if (!selectedRun) return
    if (client && online) {
      try {
        await client.cancelRun(selectedRun.spec.id)
        await refreshRuns()
        return
      } catch (cause) {
        setError(messageOf(cause))
      }
    }
    const exists = offline.outbox.some((item) => item.kind === 'cancel_run' && item.runId === selectedRun.spec.id)
    if (!exists) {
      persist({
        ...offline,
        outbox: [...offline.outbox, {
          id: crypto.randomUUID(),
          kind: 'cancel_run',
          runId: selectedRun.spec.id,
          createdAt: new Date().toISOString(),
          attempts: 0,
        }],
      })
    }
  }

  const queueOfflineReply = (event: FormEvent) => {
    event.preventDefault()
    if (!selectedRun || !offlineReply.trim()) return
    const operation = createOfflineThreadMessageOperation(selectedRun, offlineReply)
    persist({ ...offlineRef.current, outbox: [...offlineRef.current.outbox, operation] })
    setOfflineReply('')
  }

  const discardOutboxOperation = (operationId: string) => {
    persist({
      ...offlineRef.current,
      outbox: offlineRef.current.outbox.filter((operation) => operation.id !== operationId),
    })
  }

  const uploadAttachment = async (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0]
    event.target.value = ''
    if (!file || !selectedRun || !client) return
    if (!online) {
      setError('Attachments require a connection and are never added to the offline cache.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      await client.uploadAttachment(selectedRun.spec.id, file)
      setArtifactReloadKey((value) => value + 1)
    } catch (cause) {
      setError(messageOf(cause))
    } finally {
      setBusy(false)
    }
  }

  const enableNotifications = async () => {
    if (!client) return
    setPushStatus('enabling')
    setError(null)
    try {
      const result = await enableAndroidPush(client, remoteDeviceId())
      if (result !== 'enabled') throw new Error(
        result === 'server_disabled'
          ? 'FCM is not configured on this Open Cowork server.'
          : 'This APK was built without Firebase configuration.',
      )
      setPushStatus('enabled')
    } catch (cause) {
      setPushStatus('idle')
      setError(messageOf(cause))
    }
  }

  if (lockState !== 'unlocked') {
    return (
      <main className="mobile-lock-screen">
        <LockKeyhole size={42} />
        <h1>{lockState === 'pin_setup' ? 'Protect Open Cowork' : 'Open Cowork is locked'}</h1>
        <p>{lockState === 'pin_setup' ? 'Create a 6–12 digit app PIN for devices without biometrics.' : 'Use biometrics, device credentials, or your Open Cowork app PIN.'}</p>
        {lockState === 'checking' ? <span className="mobile-spinner">Checking device security…</span> : (
          <form onSubmit={submitPin}>
            <input type="password" inputMode="numeric" pattern="[0-9]{6,12}" minLength={6} maxLength={12} value={pin} onChange={(event) => setPin(event.target.value)} placeholder="App PIN" autoFocus required />
            {lockState === 'pin_setup' ? <input type="password" inputMode="numeric" pattern="[0-9]{6,12}" minLength={6} maxLength={12} value={pinConfirmation} onChange={(event) => setPinConfirmation(event.target.value)} placeholder="Confirm PIN" required /> : null}
            <button type="submit">{lockState === 'pin_setup' ? 'Create PIN' : 'Unlock'}</button>
          </form>
        )}
        {lockState === 'locked' ? <button className="mobile-link-button" type="button" onClick={() => { void retryBiometric() }}><Fingerprint size={18} /> Use biometrics / device PIN</button> : null}
        {error ? <div className="mobile-error" role="alert">{error}</div> : null}
      </main>
    )
  }

  if (account.status !== 'authenticated' || !client) {
    return (
      <main className="mobile-login-screen">
        <div className="mobile-brand"><Server size={32} /><h1>Open Cowork</h1></div>
        <p>{authMode === 'bootstrap' ? 'Set up the first server administrator.' : authMode === 'invitation' ? 'Create the account invited to this server.' : 'Connect this phone to one canonical Open Cowork server.'}</p>
        <form onSubmit={login}>
          <label>Server URL<input type="url" value={serverUrl} onChange={(event) => setServerUrl(event.target.value)} placeholder="https://cowork.example.com" required /></label>
          <label>Email<input type="email" value={email} onChange={(event) => setEmail(event.target.value)} autoComplete="username" required /></label>
          {authMode !== 'login' ? <label>Display name<input value={displayName} onChange={(event) => setDisplayName(event.target.value)} autoComplete="name" required /></label> : null}
          <label>Password<input type="password" value={password} onChange={(event) => setPassword(event.target.value)} autoComplete={authMode === 'login' ? 'current-password' : 'new-password'} minLength={authMode === 'login' ? undefined : 12} required /></label>
          {authMode !== 'login' ? <label>Confirm password<input type="password" value={accountPasswordConfirmation} onChange={(event) => setAccountPasswordConfirmation(event.target.value)} autoComplete="new-password" minLength={12} required /></label> : null}
          {authMode === 'login' ? <label>Authenticator / recovery code<input value={secondFactor} onChange={(event) => setSecondFactor(event.target.value)} inputMode="numeric" autoComplete="one-time-code" placeholder="Only when enabled" /></label> : null}
          {authMode === 'bootstrap' ? <label>Bootstrap token<input type="password" value={bootstrapToken} onChange={(event) => setBootstrapToken(event.target.value)} autoComplete="off" minLength={16} required /></label> : null}
          {authMode === 'invitation' ? <label>Invitation token<input value={invitationToken} onChange={(event) => setInvitationToken(event.target.value)} autoComplete="off" minLength={32} required /></label> : null}
          <button type="submit" disabled={busy || account.status === 'restoring'}>{account.status === 'restoring' ? 'Restoring…' : authMode === 'bootstrap' ? 'Create administrator' : authMode === 'invitation' ? 'Create account' : 'Connect'}</button>
          {authMode === 'login' && (webauthnAvailableForOrigin(serverUrl) || nativePasskeyAvailable()) ? <button type="button" disabled={busy || !serverUrl || !email} onClick={() => { void loginPasskey() }}><Fingerprint size={18} /> Sign in with passkey</button> : null}
          {authMode === 'login' && ssoEnabled ? <button type="button" disabled={busy || !serverUrl} onClick={() => { void loginOidc() }}><KeyRound size={18} /> Sign in with SSO</button> : null}
          <div className="mobile-auth-modes"><button type="button" aria-pressed={authMode === 'login'} onClick={() => setAuthMode('login')}>Sign in</button><button type="button" aria-pressed={authMode === 'invitation'} onClick={() => setAuthMode('invitation')}>Invitation</button><button type="button" aria-pressed={authMode === 'bootstrap'} onClick={() => setAuthMode('bootstrap')}>First admin</button></div>
        </form>
        {offline.runs.length > 0 ? <small>{offline.runs.length} cached runs remain available after sign-out.</small> : null}
        {(error || account.error) ? <div className="mobile-error" role="alert">{error ?? account.error}</div> : null}
      </main>
    )
  }

  if (selectedRun) {
    return (
      <main className="mobile-shell">
        <header className="mobile-header">
          <button type="button" aria-label="Back to runs" onClick={() => setSelectedRunId(null)}><ArrowLeft /></button>
          <div><strong>{runLabel(selectedRun)}</strong><small>{online ? 'Live server run' : 'Offline cached run'}</small></div>
          <button type="button" aria-label="Refresh" disabled={!online || busy} onClick={() => { void refreshRuns() }}><RefreshCw /></button>
        </header>
        {!online ? <div className="mobile-offline-banner"><CloudOff size={16} /> Offline — changes join the encrypted outbox.</div> : null}
        <section className="mobile-run-card">
          <span className={`mobile-state state-${selectedRun.state}`}>{selectedRun.state.replaceAll('_', ' ')}</span>
          <code>{selectedRun.spec.id}</code>
          <p>{selectedRun.spec.executor_target.kind.replaceAll('_', ' ')}</p>
          <div className="mobile-actions">
            {!terminalState(selectedRun.state) ? <button type="button" className="danger" onClick={() => { void cancelRun() }}><CircleStop size={16} /> Cancel</button> : null}
            {online && !activeSession && ['running', 'waiting_approval', 'waiting_input'].includes(selectedRun.state) ? <button type="button" onClick={() => { void client.startDesktopSession(selectedRun.spec.id, { width: 1280, height: 720 }).then(() => client.listDesktopSessions(selectedRun.spec.id)).then(setSessions).catch((cause) => setError(messageOf(cause))) }}><MonitorPlay size={16} /> Desktop</button> : null}
            {online && selectedRun.spec.executor_target.kind === 'server_linux' && ['running', 'waiting_approval', 'waiting_input'].includes(selectedRun.state) ? <button type="button" onClick={() => setTerminalOpen((value) => !value)}><SquareTerminal size={16} /> Terminal</button> : null}
            <button type="button" disabled={!online || busy} onClick={() => fileInput.current?.click()}><Upload size={16} /> File</button>
            <button type="button" disabled={!online || busy} onClick={() => cameraInput.current?.click()}><Camera size={16} /> Photo</button>
            <input ref={fileInput} className="mobile-hidden-input" type="file" onChange={(event) => { void uploadAttachment(event) }} />
            <input ref={cameraInput} className="mobile-hidden-input" type="file" accept="image/*" capture="environment" onChange={(event) => { void uploadAttachment(event) }} />
          </div>
        </section>
        <RemoteThreadMessages
          client={online ? client : null}
          threadId={selectedRun.spec.thread_id}
          reloadKey={messageReloadKey}
          initialMessages={offline.messages[selectedRun.spec.thread_id]}
          replyContext={online ? selectedRun : undefined}
          onLoaded={(messages) => persist({
            ...offlineRef.current,
            messages: { ...offlineRef.current.messages, [selectedRun.spec.thread_id]: messages },
          })}
          onRunCreated={(run) => {
            persist({
              ...offlineRef.current,
              runs: [run, ...offlineRef.current.runs.filter((item) => item.spec.id !== run.spec.id)],
            })
            setSelectedRunId(run.spec.id)
            setMessageReloadKey((value) => value + 1)
          }}
        />
        {!online ? (
          <section className="mobile-offline-reply">
            <h2>Queue a reply</h2>
            <p>It will start a new durable run on this thread after connectivity returns.</p>
            <form onSubmit={queueOfflineReply}>
              <textarea aria-label="Offline reply" value={offlineReply} onChange={(event) => setOfflineReply(event.target.value)} rows={4} placeholder="Continue this work when I am online again…" required />
              <button type="submit" disabled={!offlineReply.trim()}><ListRestart size={16} /> Add to outbox</button>
            </form>
            <small>{offline.outbox.filter((operation) => operation.kind === 'thread_message' && operation.threadId === selectedRun.spec.thread_id).length} queued for this thread</small>
          </section>
        ) : null}
        {online ? <RunInterventionPanel client={client} runId={selectedRun.spec.id} refreshKey={interventionReloadKey} onResolved={refreshRuns} /> : null}
        {activeSession && online ? <RemoteDesktopViewer client={client} runId={selectedRun.spec.id} session={activeSession} onSessionChanged={() => client.listDesktopSessions(selectedRun.spec.id).then(setSessions)} /> : null}
        {terminalOpen && online ? <RemoteTerminal client={client} runId={selectedRun.spec.id} onClose={() => setTerminalOpen(false)} /> : null}
        {online ? <RunArtifactPanel client={client} runId={selectedRun.spec.id} reloadKey={artifactReloadKey} /> : null}
        <section className="mobile-events">
          <h2>Run activity</h2>
          {events.length === 0 ? <p>No cached events.</p> : <ol>{events.slice().reverse().map((event: RunEvent) => <li key={event.event_id}><time>{new Date(event.created_at).toLocaleTimeString()}</time><strong>{event.kind.replaceAll('_', ' ')}</strong><small>#{event.sequence}</small></li>)}</ol>}
        </section>
        {error ? <div className="mobile-error" role="alert">{error}</div> : null}
      </main>
    )
  }

  return (
    <main className="mobile-shell">
      <header className="mobile-header">
        <div><strong>Open Cowork</strong><small>{account.email}</small></div>
        <button type="button" aria-label="Sign out" onClick={() => { void account.logout() }}><LogOut /></button>
      </header>
      {!online ? <div className="mobile-offline-banner"><CloudOff size={16} /> Offline cache</div> : null}
      {online && androidPushBuildConfigured() && pushStatus !== 'enabled' ? <button className="mobile-push-banner" type="button" disabled={pushStatus === 'enabling'} onClick={() => { void enableNotifications() }}><Bell size={16} /> {pushStatus === 'enabling' ? 'Enabling notifications…' : 'Enable private run notifications'}</button> : null}
      {offline.outbox.length > 0 ? <div className="mobile-outbox"><ListRestart size={16} /> {offline.outbox.length} queued action{offline.outbox.length === 1 ? '' : 's'}</div> : null}
      {offline.outbox.length > 0 ? <section className="mobile-outbox-list"><strong>Encrypted outbox</strong>{offline.outbox.map((operation) => <article key={operation.id}><span><b>{operation.kind === 'cancel_run' ? 'Cancel run' : 'Thread reply'}</b><small>{operation.kind === 'cancel_run' ? operation.runId : operation.request.content && typeof operation.request.content === 'object' && 'text' in operation.request.content ? String(operation.request.content.text) : operation.threadId}</small>{operation.lastError ? <em>{operation.lastError}</em> : null}</span><button type="button" aria-label={`Discard queued ${operation.kind.replaceAll('_', ' ')}`} onClick={() => discardOutboxOperation(operation.id)}>Discard</button></article>)}</section> : null}
      <section className="mobile-run-list">
        <div className="mobile-section-heading"><h1>Runs</h1><button type="button" disabled={!online || busy} onClick={() => { void refreshRuns() }}><RefreshCw size={16} /> Sync</button></div>
        {online ? <RemoteSecuritySettings compact client={client} /> : null}
        {online ? <RemoteGovernancePanel compact client={client} currentUserId={account.userId ?? ''} /> : null}
        {online ? <RemoteDeviceSettings compact client={client} /> : null}
        {online ? <RemoteOrganizationManager compact client={client} currentUserId={account.userId ?? ''} /> : null}
        {online ? <RemoteProjectVersionManager compact client={client} /> : null}
        {online ? <RemoteProviderProfileManager compact client={client} /> : null}
        {online ? <RemoteTaskManager compact client={client} onRunCreated={(run) => { const next = { ...offlineRef.current, runs: [run, ...offlineRef.current.runs.filter((item) => item.spec.id !== run.spec.id)] }; persist(next); setSelectedRunId(run.spec.id) }} /> : null}
        {online ? <RemoteScheduleManager compact client={client} /> : null}
        {online ? <RemoteRunComposer compact client={client} onCreated={(run) => { const next = { ...offlineRef.current, runs: [run, ...offlineRef.current.runs.filter((item) => item.spec.id !== run.spec.id)] }; persist(next); setSelectedRunId(run.spec.id) }} /> : null}
        {offline.runs.length === 0 ? <div className="mobile-empty"><Server size={28} /><p>No cached runs yet.</p></div> : offline.runs.map((run) => <button type="button" key={run.spec.id} onClick={() => setSelectedRunId(run.spec.id)}><span><strong>{runLabel(run)}</strong><small>{new Date(run.spec.created_at).toLocaleString()}</small></span><em className={`mobile-state state-${run.state}`}>{run.state.replaceAll('_', ' ')}</em></button>)}
      </section>
      {error ? <div className="mobile-error" role="alert">{error}</div> : null}
    </main>
  )
}
