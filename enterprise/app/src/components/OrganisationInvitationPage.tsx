import { useEffect, useMemo, useRef, useState, type FormEvent } from 'react'
import { Link, useLocation } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { RemoteAuthClient } from '../runtime/authClient'
import { ManagementClient } from '../runtime/managementClient'
import { normalizeServerUrl } from '../runtime/runtimeClient'
import type { OrganisationInvitationPreview } from '../runtime/enterpriseContracts'
import { clearInvitation, pendingInvitation, rememberInvitation } from '../runtime/organisationInvitation'
import { useRemoteRuntimeStore } from '../stores/remoteRuntimeStore'
import RemoteServerView from './RemoteServerView'
import ManagementMfaGate from './ManagementMfaGate'
import './EnterpriseManagement.css'

const web = import.meta.env.MODE === 'web' || import.meta.env.VITE_COWORK_WEB === 'true'
type Selection = { serverUrl: string; token: string }

export default function OrganisationInvitationPage() {
  const { i18n } = useTranslation()
  const t = (en: string, de: string) => i18n.language.startsWith('de') ? de : en
  const account = useRemoteRuntimeStore()
  const location = useLocation()
  const [serverUrl, setServerUrl] = useState(web ? window.location.origin : account.serverUrl)
  const [token, setToken] = useState(() => new URLSearchParams(window.location.hash.slice(1)).get('invitation') ?? pendingInvitation(serverUrl))
  const [selection, setSelection] = useState<Selection | null>(() => token && serverUrl ? { serverUrl, token } : null)
  const [loaded, setLoaded] = useState<{ selection: Selection; preview: OrganisationInvitationPreview } | null>(null)
  const preview = loaded?.selection === selection ? loaded.preview : null
  const [error, setError] = useState('')
  useEffect(() => {
    const url = new URL(window.location.href)
    const fragment = new URLSearchParams(url.hash.slice(1)); fragment.delete('invitation'); url.hash = fragment.toString()
    window.history.replaceState(window.history.state, '', `${url.pathname}${url.search}${url.hash}`)
    if (account.serverUrl && account.status === 'signed_out') void account.restore()
    // Capture and remove the fragment once; the token stays out of request URLs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])
  useEffect(() => {
    const capture = () => {
      const url = new URL(window.location.href)
      const fragment = new URLSearchParams(url.hash.slice(1))
      const incoming = fragment.get('invitation')
      if (!incoming) return
      fragment.delete('invitation'); url.hash = fragment.toString()
      window.history.replaceState(window.history.state, '', `${url.pathname}${url.search}${url.hash}`)
      clearInvitation(); setToken(incoming); setLoaded(null); setError('')
      setSelection(serverUrl ? { serverUrl, token: incoming } : null)
    }
    // Another link can navigate only the fragment without remounting this page.
    capture()
    window.addEventListener('hashchange', capture)
    return () => window.removeEventListener('hashchange', capture)
  }, [serverUrl, location.key])
  useEffect(() => {
    if (!selection) return
    const abort = new AbortController(); setError('')
    void new RemoteAuthClient(selection.serverUrl).inspectOrganisationInvitation(selection.token, abort.signal)
      .then(value => {
        if (!abort.signal.aborted) {
          setLoaded({ selection, preview: value })
          rememberInvitation(selection.serverUrl, selection.token, value.expires_at)
        }
      })
      .catch(cause => { if (!abort.signal.aborted) { setLoaded(null); setError(cause instanceof Error ? cause.message : String(cause)); clearInvitation() } })
    return () => abort.abort()
  }, [selection])
  useEffect(() => {
    if (selection && preview?.account_exists && account.status === 'signed_out' && (account.serverUrl !== selection.serverUrl || account.email !== preview.email)) account.setConnection(selection.serverUrl, preview.email)
  }, [selection, preview, account])
  const review = (event: FormEvent) => {
    event.preventDefault()
    try { setSelection({ serverUrl: normalizeServerUrl(serverUrl), token: token.trim() }); setError('') }
    catch (cause) { setError(String(cause)) }
  }
  return <main className="enterprise-management">
    <h1>{t('Join an organisation', 'Einer Organisation beitreten')}</h1>
    {!preview && <form onSubmit={review}>
      <label>{t('Server address', 'Serveradresse')}<input type="url" required readOnly={web} value={serverUrl} onChange={event => { setServerUrl(event.target.value); setToken(''); setSelection(null); setLoaded(null); clearInvitation() }} /></label>
      <label>{t('Invitation token', 'Einladungscode')}<input type="password" autoComplete="off" required minLength={32} maxLength={256} value={token} onChange={event => setToken(event.target.value)} /></label>
      <button type="submit">{t('Review invitation', 'Einladung prüfen')}</button>
    </form>}
    {error && <p role="alert">{error}</p>}
    {preview && selection && <InvitationAcceptance key={`${selection.serverUrl}:${preview.id}`} selection={selection} preview={preview} />}
    <Link to="/management" onClick={() => clearInvitation()}>{t('Back to management', 'Zur Verwaltung')}</Link>
  </main>
}

function InvitationAcceptance({ selection, preview }: { selection: Selection; preview: OrganisationInvitationPreview }) {
  const { i18n } = useTranslation()
  const t = (en: string, de: string) => i18n.language.startsWith('de') ? de : en
  const account = useRemoteRuntimeStore()
  const client = useMemo(() => new ManagementClient(selection.serverUrl), [selection.serverUrl])
  const [displayName, setDisplayName] = useState('')
  const [password, setPassword] = useState('')
  const [busy, setBusy] = useState(false)
  const [joined, setJoined] = useState(false)
  const [error, setError] = useState('')
  const alive = useRef(false)
  useEffect(() => { alive.current = true; return () => { alive.current = false } }, [])
  const role = { member: t('Member', 'Mitglied'), admin: t('Organisation administrator', 'Organisationsadministrator'), auditor: t('Audit reader', 'Audit-Leser') }[preview.role]
  const accept = async () => {
    if (busy) return
    setBusy(true); setError('')
    try {
      if (preview.account_exists) {
        const result = await client.acceptInvitation(selection.token)
        if (result.organisation_id !== preview.organisation_id) throw new Error('Invitation scope changed')
      } else {
        await account.registerOrganisationInvitation(selection.serverUrl, preview.email, displayName, password, selection.token)
      }
      clearInvitation()
      if (alive.current) { setJoined(true); setPassword('') }
    } catch (cause) { if (alive.current) setError(String(cause)) }
    finally { if (alive.current) setBusy(false) }
  }
  const matching = account.status === 'authenticated' && account.serverUrl === selection.serverUrl && account.email.toLowerCase() === preview.email.toLowerCase()
  const button = <div>{matching ? <ManagementMfaGate key={`${account.serverUrl}:${account.userId}`}><button type="button" disabled={busy} onClick={() => { void accept() }}>{t('Join organisation', 'Organisation beitreten')}</button></ManagementMfaGate> : <p>{t('Sign in with the invited email address. Switch accounts if another account is connected.', 'Melde dich mit der eingeladenen E-Mail-Adresse an. Wechsle das Konto, falls ein anderes verbunden ist.')}</p>}</div>
  return <section aria-label={t('Invitation details', 'Einladungsdetails')}>
    <h2>{preview.organisation_name}</h2><p>{selection.serverUrl}</p><p>{preview.email} · {role}</p>
    <p>{t('Expires', 'Gültig bis')}: {new Date(preview.expires_at).toLocaleString(i18n.language)}</p>
    <p>{t('Instance and project access must be assigned separately. Existing organisation roles are preserved.', 'Instanz- und Projektrechte werden gesondert zugewiesen. Bestehende Organisationsrollen bleiben erhalten.')}</p>
    {joined ? <><p role="status">{t('You have joined this organisation.', 'Du bist dieser Organisation beigetreten.')}</p><Link to={`/management?organisation=${preview.organisation_id}`}>{t('Open organisation', 'Organisation öffnen')}</Link></> : preview.account_exists
      ? account.status === 'authenticated'
        ? <><p>{account.email} · {account.serverUrl}</p>{button}<button type="button" disabled={busy} onClick={() => { void account.logout() }}>{t('Sign out / switch account', 'Abmelden / Konto wechseln')}</button></>
        : <RemoteServerView headingLevel={2} />
      : <form onSubmit={event => { event.preventDefault(); void accept() }}><label>{t('Display name', 'Anzeigename')}<input required maxLength={200} autoComplete="name" value={displayName} onChange={event => setDisplayName(event.target.value)} /></label><label>{t('New password', 'Neues Passwort')}<input type="password" required minLength={12} maxLength={1024} autoComplete="new-password" value={password} onChange={event => setPassword(event.target.value)} /></label><button disabled={busy}>{t('Create account and join', 'Konto erstellen und beitreten')}</button></form>}
    {error && <p role="alert">{error}</p>}
  </section>
}
