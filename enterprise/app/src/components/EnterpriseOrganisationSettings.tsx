import { Archive, Building2 } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import type { ManagementClient, Organisation } from '../runtime/managementClient'
import { z } from 'zod'

const settingsSchema = z.object({ id: z.string(), name: z.string(), revision: z.number().int(), archived_at: z.string().nullable(), archived_by: z.string().nullable() })
export default function EnterpriseOrganisationSettings({ client, org, enabled, canArchive, canManage, onChanged }: { client: ManagementClient; org: Organisation; enabled: boolean; canArchive: boolean; canManage: boolean; onChanged: (archived?: boolean) => void }) {
  const { i18n } = useTranslation(), de = i18n.language.startsWith('de'), t = (en: string, german: string) => de ? german : en
  const [settings, setSettings] = useState<z.infer<typeof settingsSchema> | null>(null), [name, setName] = useState(org.name), [confirmation, setConfirmation] = useState(''), [busy, setBusy] = useState(false), [error, setError] = useState(''), [showArchive, setShowArchive] = useState(false)
  const generation = useRef(0)
  useEffect(() => { const scope = generation; scope.current++; return () => { scope.current++ } }, [client, org.id])
  useEffect(() => {
    if (!enabled) return
    const abort = new AbortController()
    void client.request(`/organisations/${org.id}/settings`, settingsSchema, undefined, 'GET', abort.signal).then(value => { if (!abort.signal.aborted) { setSettings(value); setName(value.name) } }).catch(reason => { if (!abort.signal.aborted) setError(String(reason)) })
    return () => abort.abort()
  }, [client, org.id, enabled])
  async function save(archive = false) {
    if (!settings || busy || !canManage || archive && confirmation !== settings.name) return
    const current = generation.current
    setBusy(true); setError('')
    try {
      const value = await client.request(`/organisations/${org.id}/${archive ? 'archive' : 'settings'}`, settingsSchema, archive ? { confirmation_name: confirmation, expected_revision: settings.revision } : { name: name.trim(), expected_revision: settings.revision }, archive ? 'POST' : 'PUT')
      if (generation.current !== current) return
      setSettings(value); setConfirmation(''); onChanged(archive)
    } catch (reason) { if (generation.current === current) setError(reason instanceof Error ? reason.message : String(reason)) } finally { if (generation.current === current) setBusy(false) }
  }
  return <section aria-label={t('Organisation settings', 'Organisationseinstellungen')}><header><div><h2><Building2 size={18} aria-hidden="true" /> {org.name}</h2><p className="enterprise-muted">{t('Organisation ID', 'Organisations-ID')}: <code>{org.id}</code></p></div><span className="enterprise-status">{t('Active organisation', 'Aktive Organisation')}</span></header><p>{t('This organisation groups members, server instances and common rules. Removing a member does not delete their server data.', 'Diese Organisation bündelt Mitglieder, Serverinstanzen und gemeinsame Regeln. Das Entfernen eines Mitglieds löscht keine Serverdaten.')}</p>{error && <p role="alert">{error}</p>}{enabled ? <form onSubmit={event => { event.preventDefault(); void save() }}><label>{t('Organisation name', 'Organisationsname')}<input required maxLength={120} value={name} disabled={!canManage || !settings || busy} onChange={event => setName(event.target.value)} /></label>{canManage && <button disabled={busy || !settings || !name.trim() || name.trim() === settings.name}>{t('Save name', 'Namen speichern')}</button>}</form> : <p className="enterprise-notice">{t('This server version does not support renaming or archiving organisations.', 'Diese Serverversion unterstützt das Umbenennen oder Archivieren von Organisationen noch nicht.')}</p>}{canManage && canArchive && <details open={showArchive} onToggle={event => setShowArchive(event.currentTarget.open)}><summary>{t('Archive organisation', 'Organisation archivieren')}</summary><div className="enterprise-notice"><Archive size={17} aria-hidden="true" /><p>{t('Archiving removes the organisation from active management and prevents new access. Audit history is retained. First delete its active instances and settle outstanding infrastructure operations. The server checks all blockers before archiving.', 'Archivieren entfernt die Organisation aus der aktiven Verwaltung und verhindert neue Zugriffe. Die Audit-Historie bleibt erhalten. Lösche vorher ihre aktiven Instanzen und kläre ausstehende Infrastrukturvorgänge. Der Server prüft vor dem Archivieren alle Blocker.')}</p></div><form onSubmit={event => { event.preventDefault(); void save(true) }}><label>{t('Type the organisation name to confirm', 'Zur Bestätigung den Organisationsnamen eingeben')}<input autoComplete="off" value={confirmation} onChange={event => setConfirmation(event.target.value)} /></label><button className="enterprise-danger" disabled={busy || !settings || confirmation !== settings.name}>{t('Archive this organisation', 'Diese Organisation archivieren')}</button></form></details>}</section>
}
