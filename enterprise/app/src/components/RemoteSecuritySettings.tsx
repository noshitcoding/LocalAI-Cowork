import { Copy, Fingerprint, KeyRound, RefreshCw, ShieldCheck, ShieldOff, Trash2, X } from 'lucide-react'
import { useCallback, useEffect, useLayoutEffect, useState, type FormEvent } from 'react'
import { useTranslation } from 'react-i18next'

import type { AuthSessionRecord, PasskeyRecord, TotpSetup, TotpStatus } from '../runtime/contracts'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import { oidcEnabled } from '../runtime/oidc'
import { organisationId } from '../runtime/organisationSso'
import { useRemoteRuntimeStore } from '../stores/remoteRuntimeStore'
import AuthenticatorSetup from './AuthenticatorSetup'
import './RemoteSecuritySettings.css'

type Props = { client: RemoteRuntimeClient; compact?: boolean; requiredMfa?: boolean; providerVerified?: boolean; providerExpiresAt?: string; initiallyOpen?: boolean; onClose?: () => void; onCloseBlockedChange?: (blocked: boolean) => void; headingId?: string }
function messageOf(error: unknown): string { return error instanceof Error ? error.message : String(error) }

export default function RemoteSecuritySettings({ client, compact = false, requiredMfa = false, providerVerified = false, providerExpiresAt, initiallyOpen = false, onClose, onCloseBlockedChange, headingId }: Props) {
  const {i18n}=useTranslation(),t=(en:string,de:string)=>i18n.language.startsWith('de')?de:en
  const [open, setOpen] = useState(initiallyOpen)
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
  const closeBlocked = busy || recoveryCodes.length > 0
  useLayoutEffect(() => { onCloseBlockedChange?.(closeBlocked) }, [closeBlocked, onCloseBlockedChange])
  const [ssoEnabled, setSsoEnabled] = useState(false)
  const [ssoOrg,setSsoOrg]=useState(new URLSearchParams(window.location.search).get('organisation')??'')
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

  }, [load, open])
  useEffect(()=>{
    if(!open)return
    let canceled=false
    setSsoEnabled(false)
    if(!ssoOrg || organisationId(ssoOrg))void oidcEnabled(useRemoteRuntimeStore.getState().serverUrl,ssoOrg||undefined).then(value=>{if(!canceled)setSsoEnabled(value)})
    return ()=>{canceled=true}
  },[open,ssoOrg])

  const startSetup = async () => {
    setBusy(true)
    try { setSetup(await client.setupTotp()); setCode(''); setRecoveryCodes([]); setError(null) }
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
    try { await linkOidc(ssoOrg||undefined); setError(null) }
    catch (cause) { setError(messageOf(cause)) }
    finally { setBusy(false) }
  }

  if (!open) return <button className={compact ? '' : 'ui-button ui-button--secondary ui-button--sm'} type="button" onClick={() => setOpen(true)}><KeyRound size={14} /> Security</button>
  return (
    <section className={`remote-security-settings${compact ? ' compact' : ''}`}>
      <header><div><ShieldCheck size={16} /><strong id={headingId}>{t('Account security','Kontosicherheit')}</strong></div><button type="button" disabled={closeBlocked} aria-label={t("Close security settings","Sicherheitseinstellungen schließen")} onClick={() => { if (!closeBlocked) { setOpen(false); onClose?.() } }}><X size={15} /></button></header>
      {providerVerified && <p>{t("Approved SSO verification is active for this session","Freigegebene SSO-Bestätigung ist für diese Sitzung aktiv")}{providerExpiresAt ? ` ${t("until","bis")} ${new Date(providerExpiresAt).toLocaleString(i18n.language)}` : ''}. {t("Local factor enrolment remains separate.","Die Einrichtung lokaler Faktoren bleibt getrennt.")}</p>}
      {!status ? <p>Loading account security…</p> : status.enabled ? <>
        <div className="remote-security-status"><ShieldCheck size={18} /><div><strong>{t('TOTP is enabled','TOTP ist aktiviert')}</strong><small>{status.unused_recovery_codes} {t("unused recovery codes","ungenutzte Wiederherstellungscodes")}</small></div></div>
        <label>{t('Authenticator or recovery code','Authenticator- oder Wiederherstellungscode')}<input value={code} onChange={(event) => setCode(event.target.value)} autoComplete="one-time-code" /></label>
        <button type="button" disabled={busy || !code.trim()} onClick={() => { void regenerate() }}><RefreshCw size={14} /> Replace recovery codes</button>
        <label>{t('Password','Passwort')}<input type="password" value={password} onChange={(event) => setPassword(event.target.value)} autoComplete="current-password" /></label>
        <button className="danger" type="button" disabled={busy || !password || !code.trim() || (requiredMfa && passkeys.length===0)} onClick={() => { void disable() }}><ShieldOff size={14} /> Disable TOTP</button>
      </> : setup ? <form className="remote-security-totp-setup" onSubmit={enable}>
        <AuthenticatorSetup uri={setup.otpauth_uri} secret={setup.secret} expiresAt={setup.expires_at} busy={busy} onRestart={() => { void startSetup() }} />
        <label className="remote-security-totp-code">{t('Six-digit code','Sechsstelliger Code')}<input value={code} onChange={(event) => setCode(event.target.value)} inputMode="numeric" pattern="[0-9]{6}" maxLength={6} placeholder="000000" autoComplete="one-time-code" required /></label>
        <button className="ui-button ui-button--primary ui-button--lg" type="submit" disabled={busy || !/^\d{6}$/.test(code)}><ShieldCheck size={14} />{busy ? t('Confirming…', 'Wird bestätigt…') : t('Verify and enable', 'Bestätigen und aktivieren')}</button>
      </form> : <button type="button" disabled={busy} onClick={() => { void startSetup() }}><ShieldCheck size={14} /> Set up authenticator</button>}
      {requiredMfa && <p>{t("Required organisation or administrator access needs current local or approved SSO verification. Retain at least one enrolled local factor.","Organisations- oder Administratorzugriff mit MFA-Pflicht benötigt eine aktuelle lokale oder freigegebene SSO-Bestätigung. Behalte mindestens einen eingerichteten lokalen Faktor.")}</p>}
      <div className="remote-security-passkeys">
        <div className="remote-security-status"><Fingerprint size={18} /><div><strong>{t('Passkeys','Passkeys')}</strong><small>{t('Passwordless sign-in bound to this server domain','Passwortlose Anmeldung, an diese Serverdomain gebunden')}</small></div></div>
        {passkeys.length > 0 ? <ul>{passkeys.map((passkey) => <li key={passkey.id}><span><strong>{passkey.label}</strong><small>{passkey.last_used_at ? `${t("Last used","Zuletzt verwendet")} ${new Date(passkey.last_used_at).toLocaleString(i18n.language)}` : `${t("Added","Hinzugefügt")} ${new Date(passkey.created_at).toLocaleString(i18n.language)}`}</small></span><button type="button" aria-label={`${t("Remove passkey","Passkey entfernen")} ${passkey.label}`} disabled={busy || !password || (status?.enabled === true && !code.trim())} onClick={() => { void removePasskey(passkey.id) }}><Trash2 size={14} /></button></li>)}</ul> : <p>{t('No passkeys registered.','Keine Passkeys registriert.')}</p>}
        {passkeysAvailable ? <form onSubmit={registerPasskey}><label>{t('New passkey label','Name des neuen Passkeys')}<input value={passkeyLabel} onChange={(event) => setPasskeyLabel(event.target.value)} maxLength={100} placeholder={t("Work laptop","Arbeitslaptop")} required /></label><button type="submit" disabled={busy || !passkeyLabel.trim()}><Fingerprint size={14} /> Add passkey</button></form> : <p>{t('Open the web app on the server domain to add a passkey.','Öffne die Web-App auf der Serverdomain, um einen Passkey hinzuzufügen.')}</p>}
        {passkeys.length > 0 ? <p>{t('Enter your password','Gib dein Passwort')}{status?.enabled ? t(' and authenticator or recovery code',' und den Authenticator- oder Wiederherstellungscode') : ''} {t('above before removing a passkey.','oben ein, bevor du einen Passkey entfernst.')}</p> : null}
      </div>
      <div className="remote-security-passkeys">
        <div className="remote-security-status"><KeyRound size={18} /><div><strong>{t('Signed-in devices','Angemeldete Geräte')}</strong><small>{t('Review and revoke server sessions for this account','Sitzungen dieses Kontos prüfen und widerrufen')}</small></div></div>
        {sessions.length > 0 ? <ul>{sessions.map((session) => <li key={session.id}><span><strong>{session.current ? t('This session','Diese Sitzung') : `${t('Device','Gerät')} ${session.device_id.slice(0,8)}`}</strong><small>{session.active ? `${t("Last used","Zuletzt verwendet")} ${new Date(session.last_used_at).toLocaleString(i18n.language)}` : `${t('Revoked','Widerrufen')} ${session.revoked_at ? new Date(session.revoked_at).toLocaleString(i18n.language) : t('or expired','oder abgelaufen')}`}</small></span>{session.active && !session.current ? <button type="button" aria-label={`${t("Revoke device","Gerät widerrufen")} ${session.device_id.slice(0,8)}`} disabled={busy} onClick={() => { void revokeSession(session.id) }}><Trash2 size={14} /></button> : null}</li>)}</ul> : <p>{t('No recent server sessions.','Keine aktuellen Serversitzungen.')}</p>}
      </div>
      <label>{t('Organisation ID for SSO (optional)','Organisations-ID für SSO (optional)')}<input value={ssoOrg} onChange={e=>setSsoOrg(e.target.value.trim())} /></label>
      {ssoEnabled ? <div className="remote-security-passkeys"><div className="remote-security-status"><KeyRound size={18} /><div><strong>{t('Single sign-on','Single Sign-on')}</strong><small>{t('Link this account to the configured OpenID Connect provider','Dieses Konto mit dem eingerichteten OpenID-Connect-Provider verbinden')}</small></div></div><button type="button" disabled={busy} onClick={() => { void linkSso() }}><KeyRound size={14} /> Link SSO identity</button></div> : null}
      {recoveryCodes.length > 0 ? <div className="remote-recovery-codes"><strong>{t('Save these one-time recovery codes now','Diese einmaligen Wiederherstellungscodes jetzt sichern')}</strong><p>{t('They are shown only once. Store them outside this device.','Sie werden nur einmal angezeigt. Bewahre sie außerhalb dieses Geräts auf.')}</p><code>{recoveryCodes.join('\n')}</code><button type="button" onClick={() => { void copy(recoveryCodes.join('\n')) }}><Copy size={14} /> Copy all</button><button type="button" disabled={busy} onClick={() => setRecoveryCodes([])}>{t('I saved these codes','Codes gesichert')}</button></div> : null}
      {error ? <div className="remote-inline-error" role="alert">{error}</div> : null}
    </section>
  )
}
