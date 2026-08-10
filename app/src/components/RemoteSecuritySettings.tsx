import { Copy, Fingerprint, KeyRound, RefreshCw, ShieldCheck, ShieldOff, Trash2, X } from 'lucide-react'
import { useCallback, useEffect, useState, type FormEvent } from 'react'

import type { AuthSessionRecord, PasskeyRecord, TotpSetup, TotpStatus } from '../runtime/contracts'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import { oidcEnabled } from '../runtime/oidc'
import { useRemoteRuntimeStore } from '../stores/remoteRuntimeStore'

type Props = { client: RemoteRuntimeClient; compact?: boolean }
function messageOf(error: unknown): string { return error instanceof Error ? error.message : String(error) }

export default function RemoteSecuritySettings({ client, compact = false }: Props) {
  const [open, setOpen] = useState(false)
  const [status, setStatus] = useState<TotpStatus | null>(null)
  const [setup, setSetup] = useState<TotpSetup | null>(null)
  const [passkeys, setPasskeys] = useState<PasskeyRecord[]>([])
  const [sessions, setSessions] = useState<AuthSessionRecord[]>([])
  const [passkeyLabel, setPasskeyLabel] = useState('')
  const [code, setCode] = useState('')
  const [password, setPassword] = useState('')
  const [recoveryCodes, setRecoveryCodes] = useState<string[]>([])
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [ssoEnabled, setSsoEnabled] = useState(false)
  const linkOidc = useRemoteRuntimeStore((state) => state.linkOidc)
  const passkeysAvailable = client.passkeysAvailableInContext()

  const load = useCallback(async () => {
    try {
      const [totp, registeredPasskeys, registeredSessions] = await Promise.all([
        client.totpStatus(), client.listPasskeys(), client.listAuthSessions(),
      ])
      setStatus(totp)
      setPasskeys(registeredPasskeys)
      setSessions(registeredSessions)
      setError(null)
    } catch (cause) { setError(messageOf(cause)) }
  }, [client])
  useEffect(() => {
    if (!open) return
    void load()
    void oidcEnabled(useRemoteRuntimeStore.getState().serverUrl).then(setSsoEnabled)
  }, [load, open])

  const startSetup = async () => {
    setBusy(true)
    try { setSetup(await client.setupTotp()); setRecoveryCodes([]); setError(null) }
    catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }
  const enable = async (event: FormEvent) => {
    event.preventDefault(); setBusy(true)
    try {
      const result = await client.enableTotp(code)
      setRecoveryCodes(result.recovery_codes); setSetup(null); setCode(''); await load()
    } catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }
  const regenerate = async () => {
    setBusy(true)
    try { const result = await client.regenerateRecoveryCodes(code); setRecoveryCodes(result.recovery_codes); setCode(''); await load() }
    catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }
  const disable = async () => {
    setBusy(true)
    try { await client.disableTotp(password, code); setPassword(''); setCode(''); setRecoveryCodes([]); await load() }
    catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }
  const registerPasskey = async (event: FormEvent) => {
    event.preventDefault(); setBusy(true)
    try { await client.registerPasskey(passkeyLabel.trim()); setPasskeyLabel(''); await load() }
    catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }
  const removePasskey = async (passkeyId: string) => {
    setBusy(true)
    try { await client.removePasskey(passkeyId, password, code); setPassword(''); setCode(''); await load() }
    catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }
  const revokeSession = async (sessionId: string) => {
    setBusy(true)
    try { await client.revokeAuthSession(sessionId); await load() }
    catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }
  const copy = async (value: string) => { await navigator.clipboard.writeText(value) }
  const linkSso = async () => {
    setBusy(true)
    try { await linkOidc(); setError(null) }
    catch (cause) { setError(messageOf(cause)) }
    finally { setBusy(false) }
  }

  if (!open) return <button className={compact ? '' : 'ui-button ui-button--secondary ui-button--sm'} type="button" onClick={() => setOpen(true)}><KeyRound size={14} /> Security</button>
  return (
    <section className={`remote-security-settings${compact ? ' compact' : ''}`}>
      <header><div><ShieldCheck size={16} /><strong>Account security</strong></div><button type="button" aria-label="Close security settings" onClick={() => setOpen(false)}><X size={15} /></button></header>
      {!status ? <p>Loading account security…</p> : status.enabled ? <>
        <div className="remote-security-status"><ShieldCheck size={18} /><div><strong>TOTP is enabled</strong><small>{status.unused_recovery_codes} unused recovery codes</small></div></div>
        <label>Authenticator or recovery code<input value={code} onChange={(event) => setCode(event.target.value)} autoComplete="one-time-code" /></label>
        <button type="button" disabled={busy || !code.trim()} onClick={() => { void regenerate() }}><RefreshCw size={14} /> Replace recovery codes</button>
        <label>Password<input type="password" value={password} onChange={(event) => setPassword(event.target.value)} autoComplete="current-password" /></label>
        <button className="danger" type="button" disabled={busy || !password || !code.trim()} onClick={() => { void disable() }}><ShieldOff size={14} /> Disable TOTP</button>
      </> : setup ? <form onSubmit={enable}>
        <p>Add this secret to an RFC 6238 authenticator. The pending setup expires {new Date(setup.expires_at).toLocaleString()}.</p>
        <div className="remote-security-secret"><code>{setup.secret}</code><button type="button" aria-label="Copy TOTP secret" onClick={() => { void copy(setup.secret) }}><Copy size={14} /></button></div>
        <details><summary>Authenticator URI</summary><code>{setup.otpauth_uri}</code></details>
        <label>Six-digit code<input value={code} onChange={(event) => setCode(event.target.value)} inputMode="numeric" pattern="[0-9]{6}" autoComplete="one-time-code" required /></label>
        <button type="submit" disabled={busy || !/^\d{6}$/.test(code)}><ShieldCheck size={14} /> Verify and enable</button>
      </form> : <button type="button" disabled={busy} onClick={() => { void startSetup() }}><ShieldCheck size={14} /> Set up authenticator</button>}
      <div className="remote-security-passkeys">
        <div className="remote-security-status"><Fingerprint size={18} /><div><strong>Passkeys</strong><small>Passwordless sign-in bound to this server domain</small></div></div>
        {passkeys.length > 0 ? <ul>{passkeys.map((passkey) => <li key={passkey.id}><span><strong>{passkey.label}</strong><small>{passkey.last_used_at ? `Last used ${new Date(passkey.last_used_at).toLocaleString()}` : `Added ${new Date(passkey.created_at).toLocaleString()}`}</small></span><button type="button" aria-label={`Remove passkey ${passkey.label}`} disabled={busy || !password || (status?.enabled === true && !code.trim())} onClick={() => { void removePasskey(passkey.id) }}><Trash2 size={14} /></button></li>)}</ul> : <p>No passkeys registered.</p>}
        {passkeysAvailable ? <form onSubmit={registerPasskey}><label>New passkey label<input value={passkeyLabel} onChange={(event) => setPasskeyLabel(event.target.value)} maxLength={100} placeholder="Work laptop" required /></label><button type="submit" disabled={busy || !passkeyLabel.trim()}><Fingerprint size={14} /> Add passkey</button></form> : <p>Open the web app on the server domain to add a passkey.</p>}
        {passkeys.length > 0 ? <p>Enter your password{status?.enabled ? ' and authenticator or recovery code' : ''} above before removing a passkey.</p> : null}
      </div>
      <div className="remote-security-passkeys">
        <div className="remote-security-status"><KeyRound size={18} /><div><strong>Signed-in devices</strong><small>Review and revoke server sessions for this account</small></div></div>
        {sessions.length > 0 ? <ul>{sessions.map((session) => <li key={session.id}><span><strong>{session.current ? 'This session' : `Device ${session.device_id.slice(0, 8)}`}</strong><small>{session.active ? `Last used ${new Date(session.last_used_at).toLocaleString()}` : `Revoked ${session.revoked_at ? new Date(session.revoked_at).toLocaleString() : 'or expired'}`}</small></span>{session.active && !session.current ? <button type="button" aria-label={`Revoke device ${session.device_id.slice(0, 8)}`} disabled={busy} onClick={() => { void revokeSession(session.id) }}><Trash2 size={14} /></button> : null}</li>)}</ul> : <p>No recent server sessions.</p>}
      </div>
      {ssoEnabled ? <div className="remote-security-passkeys"><div className="remote-security-status"><KeyRound size={18} /><div><strong>Single sign-on</strong><small>Link this account to the configured OpenID Connect provider</small></div></div><button type="button" disabled={busy} onClick={() => { void linkSso() }}><KeyRound size={14} /> Link SSO identity</button></div> : null}
      {recoveryCodes.length > 0 ? <div className="remote-recovery-codes"><strong>Save these one-time recovery codes now</strong><p>They are shown only once. Store them outside this device.</p><code>{recoveryCodes.join('\n')}</code><button type="button" onClick={() => { void copy(recoveryCodes.join('\n')) }}><Copy size={14} /> Copy all</button></div> : null}
      {error ? <div className="remote-inline-error" role="alert">{error}</div> : null}
    </section>
  )
}
