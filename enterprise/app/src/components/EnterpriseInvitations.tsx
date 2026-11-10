import { useEffect, useRef, useState, type FormEvent } from 'react'
import { useTranslation } from 'react-i18next'
import type { ManagementClient } from '../runtime/managementClient'
import type { CreateOrganisationInvitation, OrganisationInvitation, OrganisationInvitationSecret } from '../runtime/enterpriseContracts'
import { invitationLink } from '../runtime/organisationInvitation'

export default function EnterpriseInvitations({ client, orgId, serverUrl }: { client: ManagementClient; orgId: string; serverUrl: string }) {
  const { i18n } = useTranslation()
  const t = (en: string, de: string) => i18n.language.startsWith('de') ? de : en
  const [email, setEmail] = useState('')
  const [role, setRole] = useState<CreateOrganisationInvitation['role']>('member')
  const [days, setDays] = useState(7)
  const [rows, setRows] = useState<OrganisationInvitation[]>([])
  const [secret, setSecret] = useState<OrganisationInvitationSecret | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [loading, setLoading] = useState(true)
  const [copied, setCopied] = useState(false)
  const [reload, setReload] = useState(0)
  const alive = useRef(false)
  useEffect(() => { alive.current = true; return () => { alive.current = false } }, [])
  useEffect(() => {
    const abort = new AbortController(); setLoading(true)
    void client.invitations(orgId, abort.signal)
      .then(values => { if (!abort.signal.aborted) { setRows(values); setError('') } })
      .catch(cause => { if (!abort.signal.aborted) setError(String(cause)) })
      .finally(() => { if (!abort.signal.aborted) setLoading(false) })
    return () => abort.abort()
  }, [client, orgId, reload])
  const roleNames = { member: t('Member', 'Mitglied'), admin: t('Organisation administrator', 'Organisationsadministrator'), auditor: t('Audit reader', 'Audit-Leser') }
  const states = { pending: t('Pending', 'Offen'), accepted: t('Accepted', 'Angenommen'), revoked: t('Revoked', 'Widerrufen'), expired: t('Expired', 'Abgelaufen'), authority_revoked: t('Inviter no longer authorised', 'Einladender nicht mehr berechtigt') }
  const create = async (event: FormEvent) => {
    event.preventDefault(); if (busy || loading) return
    setBusy(true); setSecret(null); setError(''); setCopied(false)
    try {
      const value = await client.createInvitation(orgId, { email: email.trim().toLowerCase(), role, expires_at: new Date(Date.now() + days * 86400000).toISOString() })
      if (alive.current && value.organisation_id === orgId) { setSecret(value); setEmail(''); setReload(n => n + 1) }
    } catch (cause) { if (alive.current) setError(String(cause)) }
    finally { if (alive.current) setBusy(false) }
  }
  const revoke = async (id: string) => {
    if (busy) return
    setBusy(true); setError('')
    try { await client.revokeInvitation(orgId, id); if (alive.current) { if (secret?.id === id) setSecret(null); setReload(n => n + 1) } }
    catch (cause) { if (alive.current) setError(String(cause)) }
    finally { if (alive.current) setBusy(false) }
  }
  const link = secret?.organisation_id === orgId ? invitationLink(serverUrl, secret.token) : ''
  return <section className="enterprise-settings-stack" aria-label={t('Organisation invitations', 'Organisationseinladungen')}>
    <h2>{t('Invite to this organisation', 'In diese Organisation einladen')}</h2>
    <p>{t('Create a private link and send it to the named person. Existing accounts must sign in. Invitations do not grant platform administration or access to every instance.', 'Erstelle einen privaten Link und übermittle ihn der genannten Person. Bestehende Konten müssen sich anmelden. Einladungen erteilen keine Plattformadministration und keinen Zugriff auf alle Instanzen.')}</p>
    <form onSubmit={event => { void create(event) }}>
      <label>{t('Invited email', 'Eingeladene E-Mail-Adresse')}<input type="email" required value={email} onChange={event => setEmail(event.target.value)} /></label>
      <label>{t('Invited role', 'Eingeladene Rolle')}<select value={role} onChange={event => setRole(event.target.value as typeof role)}>{(['member', 'admin', 'auditor'] as const).map(value => <option key={value} value={value}>{roleNames[value]}</option>)}</select></label>
      <label>{t('Valid for days', 'Gültig für Tage')}<input type="number" min={1} max={30} required value={days} onChange={event => setDays(Number(event.target.value))} /></label>
      <button disabled={busy || loading || !email}>{t('Create invitation', 'Einladung erstellen')}</button>
    </form>
    {link && secret && <div className="enterprise-confirm enterprise-settings-stack"><p>{t('Share this link only with', 'Diesen Link nur übermitteln an')} {secret.email}. {t('It is shown only now.', 'Er wird nur jetzt angezeigt.')}</p><label>{t('Private invitation link', 'Privater Einladungslink')}<input readOnly value={link} onFocus={event => event.target.select()} /></label><div className="enterprise-actions"><button type="button" onClick={() => { void navigator.clipboard.writeText(link).then(() => setCopied(true)).catch(() => setError(t('Select and copy the link manually.', 'Bitte den Link markieren und manuell kopieren.'))) }}>{copied ? t('Copied', 'Kopiert') : t('Copy link', 'Link kopieren')}</button><button type="button" onClick={() => setSecret(null)}>{t('Hide link', 'Link ausblenden')}</button></div></div>}
    <button type="button" disabled={busy || loading} onClick={() => setReload(n => n + 1)}>{t('Refresh invitations', 'Einladungen aktualisieren')}</button>
    {loading && <p role="status">{t('Loading invitations…', 'Einladungen werden geladen…')}</p>}
    <div className="enterprise-records"><table><thead><tr><th>E-Mail</th><th>{t('Role', 'Rolle')}</th><th>{t('Expires', 'Ablauf')}</th><th>{t('Status', 'Status')}</th><th>{t('Action', 'Aktion')}</th></tr></thead><tbody>{rows.map(row => <tr key={row.id}><td>{row.email}</td><td>{roleNames[row.role]}</td><td>{new Date(row.expires_at).toLocaleString(i18n.language)}</td><td>{states[row.state]}</td><td>{['pending', 'authority_revoked'].includes(row.state) && <button type="button" disabled={busy} aria-label={`${t('Revoke invitation for', 'Einladung widerrufen für')} ${row.email}`} onClick={() => { void revoke(row.id) }}>{t('Revoke', 'Widerrufen')}</button>}</td></tr>)}</tbody></table></div>
    {error && <p role="alert">{error}</p>}
  </section>
}
