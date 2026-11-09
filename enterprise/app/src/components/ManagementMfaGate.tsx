import { ArrowRight, Check, Copy, Fingerprint, KeyRound, LoaderCircle, LogOut, ShieldCheck } from 'lucide-react'
import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react'
import { useTranslation } from 'react-i18next'
import { useSearchParams } from 'react-router-dom'
import { organisationId } from '../runtime/organisationSso'
import { remoteRuntimeClient, useRemoteRuntimeStore } from '../stores/remoteRuntimeStore'
import type { AuthenticationAssurance } from '../runtime/enterpriseContracts'
import type { TotpSetup } from '../runtime/contracts'
import RemoteSecuritySettings from './RemoteSecuritySettings'
import AuthenticatorSetup from './AuthenticatorSetup'
import SignInDialog from './SignInDialog'
import { ManagementSecurityContext } from './managementSecurityContext'
import './EnterpriseManagement.css'
import './ManagementMfaGate.css'

export default function ManagementMfaGate({ children, securityInHeader = false }: { children: ReactNode; securityInHeader?: boolean }) {
  const [params]=useSearchParams(),org=organisationId(params.get("organisation"))
  const account = useRemoteRuntimeStore()
  return <ManagementMfaGateContent key={JSON.stringify([account.serverUrl, account.userId, org])} org={org || undefined} securityInHeader={securityInHeader}>{children}</ManagementMfaGateContent>
}

function ManagementMfaGateContent({ children, org, securityInHeader }: { children: ReactNode; org?: string; securityInHeader: boolean }) {
  const { i18n } = useTranslation()
  const t = (en: string, de: string) => i18n.language.startsWith('de') ? de : en
  const account = useRemoteRuntimeStore()
  const [client] = useState(remoteRuntimeClient)
  const [status, setStatus] = useState<AuthenticationAssurance | null>(null)
  const [setup, setSetup] = useState<TotpSetup | null>(null)
  const [code, setCode] = useState('')
  const [recovery, setRecovery] = useState<string[]>([])
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [copiedRecovery, setCopiedRecovery] = useState(false)
  const alive = useRef(false)
  const generation = useRef(0)
  const mutationInFlight = useRef(false)
  const recoveryPending = useRef(false)
  const returningToPassword = useRef(false)
  const invalidatePending = useCallback(() => { generation.current++ }, [])
  const load = useCallback(async () => {
    if (mutationInFlight.current || recoveryPending.current) return
    const current = ++generation.current
    try {
      const value = await client.authenticationAssurance(org||undefined)
      if (alive.current && generation.current === current) { setStatus(value); setError('') }
    } catch (cause) {
      if (alive.current && generation.current === current) { setStatus(null); setError(cause instanceof Error ? cause.message : String(cause)) }
    }
  }, [client,org])
  useEffect(() => {
    alive.current = true; void load()
    const focus = () => { void load() }
    const timer = window.setInterval(focus, 15000)
    window.addEventListener('focus', focus)
    return () => { alive.current = false; invalidatePending(); window.clearInterval(timer); window.removeEventListener('focus', focus) }
  }, [load, invalidatePending])
  const primaryConfirmed = account.hasRecentPrimaryLogin()
  const needsPrimaryLogin = !!status?.required && !status.session_verified && !primaryConfirmed && !recovery.length
  useEffect(() => {
    if (!needsPrimaryLogin || returningToPassword.current) return
    returningToPassword.current = true
    // A restored restricted session is not a password confirmation in this
    // interaction. Revoke/clear it and return to the ordinary sign-in form.
    invalidatePending()
    void account.logout().catch(() => undefined)
  }, [account, needsPrimaryLogin, invalidatePending])
  const run = async (work: (isCurrent: () => boolean) => Promise<void>) => {
    if (mutationInFlight.current) return
    mutationInFlight.current = true
    const current = ++generation.current
    const isCurrent = () => alive.current && generation.current === current
    setBusy(true); setError('')
    try { await work(isCurrent) } catch (cause) { if (isCurrent()) setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { mutationInFlight.current = false; if (isCurrent()) { generation.current++; setBusy(false) } }
  }
  const start = () => run(async isCurrent => { const value = await client.setupTotp(); if (isCurrent()) { setSetup(value); setCode('') } })
  const verify = () => run(async isCurrent => {
    if (setup) {
      const result = await client.enableTotp(code.trim())
      if (isCurrent()) { recoveryPending.current = result.recovery_codes.length > 0; setRecovery(result.recovery_codes); setSetup(null); setCode('') }
    } else {
      const result = await client.verifySecondFactor(code.trim(),org||undefined)
      if (isCurrent()) { setStatus(result); setCode('') }
    }
  })
  const copy = (value: string) => run(async isCurrent => { await navigator.clipboard.writeText(value); if (isCurrent()) setCopiedRecovery(true) })
  if (!recovery.length && status && (!status.required || status.session_verified)) {
    if (securityInHeader) return <ManagementSecurityContext.Provider value={{ client, requiredMfa: status.required, providerVerified: status.provider_verified, providerExpiresAt: status.provider_verification_expires_at }}>{children}</ManagementSecurityContext.Provider>
    return <><div className="enterprise-management"><RemoteSecuritySettings client={client} compact requiredMfa={status.required} providerVerified={status.provider_verified} providerExpiresAt={status.provider_verification_expires_at} /></div>{children}</>
  }
  if (needsPrimaryLogin) return <div className="remote-server-view remote-server-login"><p role="status">{t('Sign in again to continue.', 'Melde dich erneut an, um fortzufahren.')}</p></div>
  if (!status && !primaryConfirmed && !recovery.length) return <div className="remote-server-view remote-server-login"><div className="remote-login-card">
    <p role={error ? 'alert' : 'status'}>{error || t('Checking sign-in verification…', 'Anmeldung wird geprüft…')}</p>
    {error && <button type="button" onClick={() => { void load() }}>{t('Retry', 'Erneut prüfen')}</button>}
    <button type="button" onClick={() => { void account.logout() }}>{t('Sign out', 'Abmelden')}</button>
  </div></div>
  return <SignInDialog labelledBy="management-mfa-heading" busy={busy} onCancel={() => { void account.logout() }}>
    <section className="management-mfa-card" aria-labelledby="management-mfa-heading" aria-busy={busy}>
      <header className="management-mfa-header">
        <div className="management-mfa-mark"><ShieldCheck size={24} strokeWidth={1.6} aria-hidden="true" /></div>
        <h1 id="management-mfa-heading">{recovery.length ? t('Save your recovery codes', 'Wiederherstellungscodes sichern') : setup ? t('Set up your authenticator', 'Authenticator einrichten') : t('Confirm your sign-in', 'Anmeldung bestätigen')}</h1>
        <p>{recovery.length ? t('Your authenticator is ready. Keep a way back into your account.', 'Dein Authenticator ist bereit. Sichere dir den Zugang zu deinem Konto.') : setup ? t('Connect your app in three short steps.', 'Verbinde deine App in drei kurzen Schritten.') : t('One more step to open your workspace.', 'Noch ein Schritt bis zu deinem Arbeitsbereich.')}</p>
      </header>
      <div className="management-mfa-account"><strong>{account.email}</strong><span>{account.serverUrl}</span></div>
      {status?.required && !recovery.length && <p className="management-mfa-policy">{t('This access requires an additional sign-in check.', 'Für diesen Zugriff ist eine zusätzliche Bestätigung erforderlich.')}</p>}
      {error && <p className="management-mfa-error" role="alert">{error}</p>}
      {recovery.length > 0 ? <div className="management-mfa-recovery">
        <p>{t('Each code works once. Save them separately from this device before continuing. They are shown only now.', 'Jeder Code funktioniert einmal. Sichere sie getrennt von diesem Gerät, bevor du fortfährst. Sie werden nur jetzt angezeigt.')}</p>
        <pre aria-label={t('Recovery codes', 'Wiederherstellungscodes')}>{recovery.join('\n')}</pre>
        <button className="ui-button ui-button--secondary ui-button--lg" type="button" disabled={busy} onClick={() => { void copy(recovery.join('\n')) }}>{copiedRecovery ? <Check size={16} aria-hidden="true" /> : <Copy size={16} aria-hidden="true" />}{copiedRecovery ? t('Codes copied', 'Codes kopiert') : t('Copy recovery codes', 'Wiederherstellungscodes kopieren')}</button>
        <button className="ui-button ui-button--primary ui-button--lg" type="button" disabled={busy} onClick={() => { recoveryPending.current = false; setRecovery([]); setStatus(null); void load() }}>{t('I saved the codes — continue', 'Codes gesichert – weiter')}<ArrowRight size={16} aria-hidden="true" /></button>
      </div> : !status ? <div className="management-mfa-loading">
        {!error && <p role="status"><LoaderCircle className="management-mfa-spinner" size={18} aria-hidden="true" />{t('Checking sign-in verification…', 'Anmeldung wird geprüft…')}</p>}
        {error && <button className="ui-button ui-button--primary ui-button--lg" type="button" onClick={() => { void load() }}>{t('Retry', 'Erneut prüfen')}</button>}
      </div>
        : status.reauthentication_required ? <p className="management-mfa-instruction">{t('Sign out and sign in again before setting up your authenticator. For your security, setup needs a sign-in from the last ten minutes.', 'Melde dich ab und erneut an, um deinen Authenticator einzurichten. Zur Sicherheit darf die Anmeldung dafür höchstens zehn Minuten zurückliegen.')}</p>
          : setup ? <form className="management-mfa-form" onSubmit={event => { event.preventDefault(); void verify() }}>
            <AuthenticatorSetup uri={setup.otpauth_uri} secret={setup.secret} expiresAt={setup.expires_at} busy={busy} onRestart={() => { void start() }} />
            <label>{t('Six-digit code', 'Sechsstelliger Code')}<input className="management-mfa-code" autoComplete="one-time-code" data-signin-focus inputMode="numeric" required pattern="[0-9]{6}" maxLength={6} placeholder="000000" value={code} onChange={event => setCode(event.target.value)} /></label>
            <button className="ui-button ui-button--primary ui-button--lg" disabled={busy || !/^\d{6}$/.test(code)}>{busy && <LoaderCircle className="management-mfa-spinner" size={16} aria-hidden="true" />}{t('Verify and enable', 'Bestätigen und aktivieren')}<ArrowRight size={16} aria-hidden="true" /></button>
          </form> : status.totp_enabled ? <form className="management-mfa-form" onSubmit={event => { event.preventDefault(); void verify() }}>
            <p className="management-mfa-instruction">{t('Open your authenticator app and enter the current code. You can also use an unused recovery code.', 'Öffne deine Authenticator-App und gib den aktuellen Code ein. Du kannst auch einen unbenutzten Wiederherstellungscode verwenden.')}</p>
            <label>{t('Authenticator or recovery code', 'Authenticator- oder Wiederherstellungscode')}<input autoComplete="one-time-code" data-signin-focus maxLength={128} required autoFocus value={code} onChange={event => setCode(event.target.value)} /></label>
            <button className="ui-button ui-button--primary ui-button--lg" disabled={busy || !code.trim()}>{busy && <LoaderCircle className="management-mfa-spinner" size={16} aria-hidden="true" />}{t('Verify this session', 'Diese Sitzung bestätigen')}<ArrowRight size={16} aria-hidden="true" /></button>
          </form> : status.enrollment_allowed ? <div className="management-mfa-enroll">
            <p className="management-mfa-instruction">{t('Use Microsoft Authenticator or another authenticator app. Scan a QR code, then confirm the code from your app.', 'Nutze Microsoft Authenticator oder eine andere Authenticator-App. Scanne einen QR-Code und bestätige anschließend den Code aus deiner App.')}</p>
            <button className="ui-button ui-button--primary ui-button--lg" type="button" disabled={busy} onClick={() => { void start() }}>{busy ? <LoaderCircle className="management-mfa-spinner" size={16} aria-hidden="true" /> : <KeyRound size={16} aria-hidden="true" />}{t('Set up authenticator', 'Authenticator einrichten')}<ArrowRight size={16} aria-hidden="true" /></button>
          </div> : null}
      {!setup && !recovery.length && (status?.passkey_registered || status?.oidc_reauthentication_available) && <div className="management-mfa-alternatives">
        {status.passkey_registered && <button className="ui-button ui-button--secondary ui-button--lg" type="button" disabled={busy} onClick={() => { void run(() => account.loginPasskey(account.serverUrl, account.email)) }}><Fingerprint size={16} aria-hidden="true" />{t('Sign in with a verified passkey', 'Mit einem bestätigten Passkey anmelden')}</button>}
        {status.oidc_reauthentication_available && <button className="ui-button ui-button--secondary ui-button--lg" type="button" disabled={busy} onClick={() => { void run(() => account.loginOidc(account.serverUrl, status.oidc_organisation_id??org??undefined)) }}><KeyRound size={16} aria-hidden="true" />{t('Verify by signing in with approved SSO', 'Mit freigegebenem SSO erneut anmelden')}</button>}
      </div>}
      <footer className="management-mfa-footer"><button className="ui-button ui-button--ghost ui-button--sm" type="button" disabled={busy} onClick={() => { void account.logout() }}><LogOut size={15} aria-hidden="true" />{t('Sign out', 'Abmelden')}</button></footer>
    </section>
  </SignInDialog>
}
